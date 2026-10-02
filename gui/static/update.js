/* wp8f 控制台前端 —— 飞行模型数据库更新面板（P3 / P6）
 *
 * 设计：
 *  - **自己挂载**：面板插在 `#tab-fm` 的最前面（index.html 里只多一行 `<script>`）；
 *  - **只读接口的 JSON**：`/api/update/*` 一律 HTTP 200 + `{ok, error?, status}`，
 *    失败是业务结果（网络/校验/磁盘/权限/HUD 在跑…分类在前端的 `error.kind` 上选文案）；
 *  - **文案全部走 i18n**：`update.*` 键（zh-CN 为原文，15 种语言同键集）；
 *  - **「更新到下载新版本」只在下载完成后可选**（`staging_ready`）：没下完就是禁用；
 *  - **没有「停止 HUD」这个动作**：HUD 在跑时后端直接拒绝并回
 *    `kind = "hud_running"`，界面把原因弹出来，由用户自己关 —— 更新器不杀进程；
 *  - **二次确认**：点「更新到下载新版本」先弹页内确认（讲清"会丢掉哪些顶层目录 / HUD 要重启"），
 *    确认后才发 `POST /api/update/apply`（后端此时**再校验一遍**下载目录，失败就原样保留）。
 *  - 轮询只在飞行模型页签可见时进行（隐藏页签不做无用请求），间隔 1.5s（下载中）。
 */
const UPDATE_POLL_MS = 1500;
const up = { timer: null, status: null, busy: false, logOpen: false };

/** i18n 查表（表没就绪时回退到键名，见 app.js 的 t()） */
const upT = (k, vars) => t(k, vars);

/** 分类错误 → 一句人话（`error.kind` 是后端给的机器名）。
 *  分隔符用 ` — ` 而不是全角冒号：这一行走的是 15 种语言，标点不该是中文专用的。 */
function upErrText(error) {
  if (!error) return "";
  const key = {
    network: "update.err.network",
    verify: "update.err.verify",
    disk: "update.err.disk",
    permission: "update.err.permission",
    io: "update.err.io",
    protocol: "update.err.protocol",
    cancelled: "update.err.cancelled",
    // HUD 正在运行 → 拒绝更新（后端不会替用户结束 wp8f.exe）
    hud_running: "update.err.hud_running",
  }[error.kind] || "update.err.io";
  return `${upT(key)} — ${error.message || ""}`;
}

/** 更新器接口：POST 无 body（JSON 头必须带，服务端 guard 要求 application/json） */
async function upApi(path, body) {
  const r = await fetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body || {}),
  });
  return r.json();
}

/** 建面板（只建一次） */
function updateMount() {
  const tab = document.getElementById("tab-fm");
  if (!tab || document.getElementById("fm-update")) return;
  const box = document.createElement("div");
  box.id = "fm-update";
  box.className = "card up-card";
  box.innerHTML = `
    <div class="up-head">
      <span class="up-title" data-up="title"></span>
      <span class="up-phase" id="up-phase"></span>
    </div>
    <div class="up-ver">
      <span><i data-up="current"></i> <b id="up-cur">—</b></span>
      <span><i data-up="latest"></i> <b id="up-latest">—</b></span>
      <span class="up-note" id="up-note"></span>
    </div>
    <div class="up-bar" id="up-bar" hidden><div class="up-bar-fill" id="up-fill"></div></div>
    <div class="up-prog" id="up-prog"></div>
    <div class="up-msg" id="up-msg"></div>
    <div class="up-btns">
      <button class="tbtn boxed" id="up-check" data-up="check"></button>
      <button class="tbtn boxed" id="up-start" data-up="start"></button>
      <button class="tbtn boxed" id="up-cancel" data-up="cancel"></button>
      <button class="tbtn boxed" id="up-clear" data-up="discard"></button>
      <button class="tbtn boxed" id="up-apply" data-up="switch"></button>
      <button class="tbtn boxed" id="up-log-toggle" data-up="log"></button>
    </div>
    <div class="up-confirm" id="up-confirm" hidden>
      <div class="up-confirm-text" id="up-confirm-text"></div>
      <div class="up-btns">
        <button class="tbtn boxed" id="up-confirm-yes" data-up="confirm_yes"></button>
        <button class="tbtn boxed" id="up-confirm-no" data-up="confirm_no"></button>
      </div>
    </div>
    <pre class="up-log" id="up-log" hidden></pre>
    <div class="up-legal" id="up-legal"></div>
  `;
  tab.insertBefore(box, tab.firstChild);

  document.getElementById("up-check").addEventListener("click", () => upAct("check"));
  document.getElementById("up-start").addEventListener("click", () => upAct("start"));
  document.getElementById("up-cancel").addEventListener("click", () => upAct("cancel"));
  document.getElementById("up-clear").addEventListener("click", () => upClear());
  document.getElementById("up-apply").addEventListener("click", () => upAskConfirm());
  document.getElementById("up-confirm-yes").addEventListener("click", () => upApply());
  document.getElementById("up-confirm-no").addEventListener("click", () => upHideConfirm());
  document.getElementById("up-log-toggle").addEventListener("click", () => {
    up.logOpen = !up.logOpen;
    const pre = document.getElementById("up-log");
    pre.hidden = !up.logOpen;
    upRenderStatic();
  });
  upRenderStatic();
}

/** 静态文案（语言切换后由轮询重刷） */
function upRenderStatic() {
  document.querySelectorAll("#fm-update [data-up]").forEach((el) => {
    el.textContent = upT("update." + el.dataset.up);
  });
  const pre = document.getElementById("up-log");
  if (pre) pre.hidden = !up.logOpen;
  const legal = document.getElementById("up-legal");
  if (legal) legal.textContent = upT("update.legal");
}

/** 按钮可用性 + 进度条 + 错误行 */
function upRender(st) {
  up.status = st;
  // 静态文案每次都刷：语言切换（P4 的下拉）后不必等刷新，面板自己跟上
  upRenderStatic();
  const t2 = document.getElementById("up-phase");
  const cur = document.getElementById("up-cur");
  const latest = document.getElementById("up-latest");
  const note = document.getElementById("up-note");
  const prog = document.getElementById("up-prog");
  const msg = document.getElementById("up-msg");
  const bar = document.getElementById("up-bar");
  const fill = document.getElementById("up-fill");
  if (!t2) return;

  const phaseText = {
    idle: "update.phase.idle",
    checking: "update.phase.checking",
    downloading: "update.phase.downloading",
    finalizing: "update.phase.finalizing",
    ready: "update.phase.ready",
    switching: "update.phase.switching",
    done: "update.phase.done",
    failed: "update.phase.failed",
  }[st.phase] || "update.phase.idle";
  t2.textContent = upT(phaseText);
  t2.className = "up-phase " + (st.phase === "failed" ? "err" : st.phase === "done" || st.phase === "ready" ? "ok" : "");

  cur.textContent = st.local_version || upT("update.unknown");
  latest.textContent = st.remote_version || upT("update.unknown");

  // 一行状态说明：有新版 / 已是最新 / 暂存可用 / 清单来源
  let noteText = "";
  if (st.update_available && st.remote_version) {
    noteText = upT("update.available", { from: st.local_version || upT("update.unknown"), to: st.remote_version });
  } else if (st.local_version && st.remote_version) {
    noteText = upT("update.up_to_date");
  } else if (st.remote_version && !st.local_version) {
    noteText = upT("update.first_time");
  }
  if (st.manifest && st.manifest.note && !st.manifest.authoritative && st.progress.files_total > 0) {
    noteText += (noteText ? " · " : "") + upT("update.manifest_local");
  }
  if (st.staging_ready && st.phase !== "done") {
    noteText += (noteText ? " · " : "") + upT("update.ready");
  }
  if (st.phase === "done") {
    noteText = upT("update.done");
  }
  note.textContent = noteText;

  // 进度
  const p = st.progress || {};
  const showBar = st.busy || p.files_total > 0;
  bar.hidden = !showBar;
  const ratio = p.files_total ? Math.min(1, (p.files_done || 0) / p.files_total) : 0;
  fill.style.width = (ratio * 100).toFixed(1) + "%";
  if (p.files_total > 0) {
    prog.textContent = upT("update.progress", {
      done: p.files_done, total: p.files_total,
      bytes: p.bytes_done_h || "0 B", bytes_total: p.bytes_total_h || "0 B",
    }) + (p.retries ? " · " + upT("update.retries", { n: p.retries }) : "")
      + (p.files_failed ? " · " + upT("update.failed_n", { n: p.files_failed }) : "");
  } else {
    prog.textContent = "";
  }

  // 错误 / 提示
  msg.className = "up-msg";
  if (st.error) {
    msg.classList.add("err");
    msg.textContent = upErrText(st.error);
  } else if (st.hud_running && st.staging_ready) {
    // 只提示，不替用户关 HUD（没有「停止 HUD」这个动作）
    msg.classList.add("warn");
    msg.textContent = upT("update.hud_running");
  } else if (st.json_supported === false) {
    msg.classList.add("warn");
    msg.textContent = upT("update.json_needed");
  } else {
    msg.textContent = "";
  }

  // 按钮
  //  - 「更新到下载新版本」**只有下载完成（staging_ready）后才可选**；没下完 / 忙 / HUD 在跑都是禁用
  //    （HUD 在跑时后端也会拒绝，这里禁用只是别让用户白点一次）
  //  - 「取消」只在忙时可点；「清除下载」只在有下载目录且不忙时可点
  //    （"取消"要清掉 data_new 与 .wp8f_update_tmp）
  const dis = !!st.busy;
  const btn = (id, on) => { const b = document.getElementById(id); if (b) b.disabled = !on; };
  const hasStaging = !!(st.staging_version || (st.progress && st.progress.files_done > 0));
  btn("up-check", !dis);
  btn("up-start", !dis);
  btn("up-cancel", st.busy);
  btn("up-clear", !dis && hasStaging);
  btn("up-apply", !dis && !!st.staging_ready && !st.hud_running);
  // 暂存不再是"可更新"状态（更新完了 / 重新下载了）就把确认框收起来
  if (!st.staging_ready || dis) upHideConfirm();

  // 日志
  const pre = document.getElementById("up-log");
  if (pre) {
    const lines = (st.log || []).slice(-80);
    pre.textContent = lines.join("\n");
    if (up.logOpen) pre.scrollTop = pre.scrollHeight;
  }
}

/** 动作：check / start / cancel */
async function upAct(what) {
  if (up.busy) return;
  up.busy = true;
  try {
    const body = what === "start" && up.status && up.status.remote_version
      ? { tag: up.status.remote_version }
      : {};
    const d = await upApi("/api/update/" + what, body);
    if (d.status) upRender(d.status);
    if (!d.ok && d.error) {
      msgShow(upErrText(d.error), false);
    } else if (what === "check") {
      const st = d.status || {};
      msgShow(st.update_available
        ? upT("update.available", { from: st.local_version || upT("update.unknown"), to: st.remote_version || "" })
        : upT("update.up_to_date"), true);
    } else if (what === "start") {
      msgShow(upT("update.phase.downloading"), true);
    } else if (what === "cancel") {
      msgShow(upT("update.err.cancelled"), true);
    }
  } catch (e) {
    msgShow(`${upT("update.err.network")} — ${e}`, false);
  } finally {
    up.busy = false;
  }
  upPoll(true);
}

/** 清除下载目录（`data_new` + `.wp8f_update_tmp`）：校验没过时的"取消"就走这里 */
async function upClear() {
  if (up.busy) return;
  up.busy = true;
  try {
    const d = await upApi("/api/update/discard", {});
    if (d.status) upRender(d.status);
    if (d.ok) msgShow(upT("update.discarded"), true);
    else msgShow(upErrText(d.error), false);
  } catch (e) {
    msgShow(upT("update.err.discard", { err: String(e) }), false);
  } finally {
    up.busy = false;
  }
  upPoll(true);
}

/** 更新前先问一次（**页内确认**，不依赖 window.confirm：
 *  WebView2 的原生对话框行为由宿主决定，界面上自己画一个才可控、也能被探针点到） */
function upAskConfirm() {
  const st = up.status || {};
  // HUD 在跑：按钮本来就禁用了，这里再兜一道（脚本/键盘也能触发 click）
  if (st.hud_running) {
    msgShow(upT("update.err.hud_running"), false);
    return;
  }
  if (!st.staging_ready) {
    msgShow(upT("update.no_staging"), false);
    return;
  }
  const dropped = (st.dropped_top_level || []).filter((n) => n !== "version");
  // 固定单槽：上一版目录名是常量 `data_old`（不再带时间戳）
  let text = upT("update.confirm_switch", { old: "data_old" });
  if (dropped.length) text += " " + upT("update.dropped", { list: dropped.join(", ") });
  text += " " + upT("update.confirm_hud");
  const box = document.getElementById("up-confirm");
  const t2 = document.getElementById("up-confirm-text");
  if (!box || !t2) return;
  t2.textContent = text;
  box.hidden = false;
  const yes = document.getElementById("up-confirm-yes");
  if (yes) yes.focus();
}

function upHideConfirm() {
  const box = document.getElementById("up-confirm");
  if (box) box.hidden = true;
}

/** 用户确认之后才真的更新（后端会**再校验一遍**下载目录，没过就原样保留） */
async function upApply() {
  upHideConfirm();
  if (up.busy) return;
  up.busy = true;
  try {
    const d = await upApi("/api/update/apply", {});
    if (d.status) upRender(d.status);
    if (d.ok) msgShow(upT("update.done"), true);
    else msgShow(upErrText(d.error), false);
  } catch (e) {
    msgShow(`${upT("update.err.network")} — ${e}`, false);
  } finally {
    up.busy = false;
  }
  if (typeof loadFmVersion === "function") loadFmVersion();
  upPoll(true);
}

/** 状态栏 + toast（复用 app.js 的统一出口） */
function msgShow(text, ok) {
  report(text, ok, "update");
}

/** 语言切换后**立刻**按新语言重刷面板（不等下一次轮询 —— 轮询只在飞行模型页签可见时跑，
 *  最长要 1.5s 才跟上；面板上的文案全部是 JS 生成的，`applyI18n()` 的 `data-i18n` 管不到它）。
 *  `app.js` 的 `setUiLang()` 经 `fmOnLangChange()` 用可选全局方式调过来。 */
function upOnLangChange() {
  if (up.status) upRender(up.status);
  else upRenderStatic();
}

/** 轮询：只在飞行模型页签可见时跑 */
async function upPoll(force) {
  const tab = document.getElementById("tab-fm");
  if (!tab) return;
  if (!force && !tab.classList.contains("active")) return;
  try {
    const r = await fetch("/api/update/status");
    const d = await r.json();
    if (d.status) upRender(d.status);
  } catch (e) { /* 控制台自己在服务这个请求，失败就是进程要退了：静默 */ }
}

document.addEventListener("DOMContentLoaded", () => {
  updateMount();
  upPoll(true);
  if (up.timer) clearInterval(up.timer);
  up.timer = setInterval(() => upPoll(false), UPDATE_POLL_MS);
});
