/* wp8f 控制台前端（零构建链：原生 JS）—— 主逻辑 + 共享工具
 *
 * 这里只放"所有页签都要用"的东西，各页签的逻辑在自己的文件里：
 *   app.js（本文件）：启动、页签路由、状态提示、图表容器/主题工具、共享常量
 *   hud-colors.js  ：颜色口径 `#RRGGBBAA` 的解析/格式化（纯字符串，不碰 DOM）
 *   config-form.js ：配置器：配置列表 / 表单生成 / 联动夹紧 / 自动保存 / 启动 HUD
 *   fm-charts.js   ：飞行模型页签：机型、燃油与重量、6 张曲线图、机型信息与对比
 *   replay.js      ：回放页签：录制文件列表、3D 轨迹、xoy 底面地图、遥测、转换导出
 *   missile.js     ：导弹模拟页签：离线 iframe 的加载 / 卸载 / 子模块缺失提示
 *
 * **没有打包器**：index.html 按顺序放多个 classic `<script>`，它们共享同一个全局作用域，
 * 所以 `const` / `let` / `function` 跨文件直接可见。代价是加载顺序有约束：
 *   1. `app.js` 必须第一个（`$` / `api` / `esc` / `report` / `notify` / 图表工具都在这里）；
 *   2. 各页签文件只用自己的 `const`，跨文件只调**函数**（函数声明会提升到脚本开头）；
 *   3. 启动动作推迟到 `DOMContentLoaded`，那时所有脚本都已执行。
 */
const $ = (sel) => document.querySelector(sel);
const api = async (url, opts) => {
  const r = await fetch(url, opts);
  if (!r.ok) {
    const body = await r.text();
    throw new Error(`${r.status}: ${body}`);
  }
  return r.json();
};

/* HTML 转义：所有进入 innerHTML 的外部数据（文件名/机型名/配置值/错误消息）必须过这里 */
const esc = (s) =>
  String(s ?? "").replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

/* ---------- i18n：一次 fetch + Object.freeze ----------
 * 文案唯一来源 = `resource/i18n/<lang>.json`（Rust 侧读同一份），`/api/i18n[/<lang>]` 返回
 * "当前语言覆盖 en"的合并表 → 前端回退链末端只剩键名。查表 = 一次属性访问，不在每帧路径上。
 * 语言入口 = 左栏下拉（`auto` = 跟随系统），改动写回配置的 `language` 键。 */
let I18N = Object.freeze({});      // 当前语言（已与 en 合并）
let I18N_LANG = "";                // 生效语言 code（"" = 还没拿到表）
let I18N_MISSING = [];             // en 有、当前语言没有的键（开发期漏翻可见性）
let LANG_LIST = [];                // [{code,name}]，由后端给出（单一来源：resource/i18n）
const LANG_KEY = "wp8f.lang";      // GUI 侧记住用户的选择（""/"auto" = 跟随系统）

/** 查表：当前语言 →（后端已合并 en）→ 键名本身。`vars` 替换 `{name}` 这类占位符。 */
function t(key, vars) {
  let s = I18N[key];
  if (s === undefined) s = key;
  if (vars) s = String(s).replace(/\{(\w+)\}/g, (m, k) => (vars[k] !== undefined ? vars[k] : m));
  return s;
}

/** 取一次文案表。`lang` 为空 = 让后端按配置/系统解析。 */
async function fetchI18n(lang) {
  const url = lang ? `/api/i18n/${encodeURIComponent(lang)}` : "/api/i18n";
  const d = await api(url);
  I18N = Object.freeze(Object.assign({}, d.strings || {}));
  I18N_LANG = d.lang || "";
  I18N_MISSING = d.missing || [];
  if (Array.isArray(d.languages) && d.languages.length) LANG_LIST = d.languages;
  if (I18N_LANG) document.documentElement.lang = I18N_LANG;
  return d;
}

/** 把 `data-i18n` / `data-i18n-title` / `data-i18n-placeholder` 的静态文案刷一遍。
 *  表没拿到时**不动 DOM**：宁可留 HTML 里的原文，也不要让界面变成一堆键名。 */
function applyI18n() {
  if (!I18N_LANG) return;
  document.title = t("app.title");
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    let vars = null;
    if (el.dataset.i18nVars) { try { vars = JSON.parse(el.dataset.i18nVars); } catch (e) { vars = null; } }
    el.textContent = t(el.dataset.i18n, vars);
  });
  document.querySelectorAll("[data-i18n-title]").forEach((el) => { el.title = t(el.dataset.i18nTitle); });
  document.querySelectorAll("[data-i18n-placeholder]").forEach((el) => { el.placeholder = t(el.dataset.i18nPlaceholder); });
}

/** localStorage 里的语言偏好（""/"auto" = 跟随系统）。 */
function uiLangPref() {
  try { return localStorage.getItem(LANG_KEY) || "auto"; } catch (e) { return "auto"; }
}

/** 左栏语言下拉：第一项是"跟随系统"，其余是 `resource/i18n/*.json` 里的语言。 */
function fillLangSelect() {
  const sel = $("#lang-select");
  if (!sel) return;
  const items = [{ v: "auto", label: t("app.lang_auto") }].concat(
    LANG_LIST.map((l) => ({ v: l.code, label: l.name })));
  sel.replaceChildren(...items.map((o) => {
    const el = document.createElement("option");
    el.value = o.v;
    el.textContent = o.label;
    return el;
  }));
  sel.value = uiLangPref();
}

/** 切语言：重新取表（后端同时把自己的当前语言切过去）→ 刷新静态文案 → 重渲染各页签。
 *  `writeConfig` = 同时写回当前配置的 `language` 键（用户在下拉里改时才有意义）。
 *
 *  ⚠️ localStorage 里存的是后端解析出来的 code（`d.lang`），不是传进去的原值：配置里可能写着
 *  旧代码（`zh-CN`）或 `auto`，存原值会让左栏下拉选不中任何一项（下拉的 value 是新代码）。 */
async function setUiLang(v, writeConfig = false) {
  let d;
  try {
    d = await fetchI18n(v === "auto" ? "" : v);
  } catch (e) {
    report(t("cfg.load_list_failed", { err: e }), false);
    return;
  }
  try { localStorage.setItem(LANG_KEY, v === "auto" ? "auto" : (d.lang || v)); } catch (e) { /* 隐私模式忽略 */ }
  applyI18n();
  fillLangSelect();
  // 配置器整块是 JS 生成的（config-form.js 的 renderForm 按 t() 取词），必须重渲染
  if (typeof renderForm === "function" && cfgJson && cfgJson.value) renderForm(cfgJson.value);
  // 飞行模型页签同理（机型信息卡片 + 图表轴名/图例）；用可选全局调用，不引入硬依赖
  if (typeof fmOnLangChange === "function") fmOnLangChange();
  // 回放页签（3D 图轴名 / series 名 / 遥测 / 底面提示 / 下拉状态）
  if (typeof rpOnLangChange === "function") rpOnLangChange();
  // 写回配置：写**生效的 code**（旧代码/别名由后端归一，写回去的是新代码）
  if (writeConfig && typeof setConfigValue === "function") setConfigValue("language", v === "auto" ? "auto" : (d.lang || v));
}

/* FM 的 6 张图：ID 清单只此一份（加/删一张图只改这里，再也不会有"某张图永远空白"）。
 * 注意必须在 activateTab 之前定义 —— 它是 const，启动时恢复页签会立刻用到。 */
const FM_CHART_IDS = ["fm-chart-cl", "fm-chart-power", "fm-chart-3d",
                      "fm-chart-tasalt", "fm-chart-thrust-tas", "fm-chart-thrust-ias"];
const fmEachChart = (fn) => FM_CHART_IDS.forEach((id) => fn(document.getElementById(id), id));

/* ---------- Tab 切换 ---------- */
// 底部操作条（预览配置/开始）只在配置器页签显示
function setActionbarVisible(show) {
  const bar = document.querySelector(".actionbar");
  if (bar) bar.style.display = show ? "flex" : "none";
}

const LAST_TAB_KEY = "wp8f.lastTab";
let activeTabName = "";   // 切出「导弹模拟」时要卸载 iframe（见 closeMissile）
function activateTab(name) {
  const btn = document.querySelector(`#tabs .tab[data-tab="${name}"]`);
  if (!btn) return;
  const prev = activeTabName;
  document.querySelectorAll("#tabs .tab").forEach((b) => b.classList.remove("active"));
  document.querySelectorAll(".panel").forEach((p) => p.classList.remove("active"));
  btn.classList.add("active");
  $(`#tab-${name}`).classList.add("active");
  setActionbarVisible(name === "config");
  // 导弹模拟是整页 iframe（Three.js 渲染循环 + 导弹数据）：切出即卸载，
  // 否则它会一直在后台跑满 GPU/CPU（页签隐藏 ≠ 停止）。
  if (prev === "missile" && name !== "missile") closeMissile();
  if (name === "fm") {
    fmEachChart((node) => {
      if (node && !node.querySelector("canvas")) chartPlaceholder(node, t("common.loading"), true);
    });
    loadAircraft();
  }
  if (name === "replay") loadReplayFiles();
  if (name === "missile") ensureMissile();
  resizeTabCharts(name);   // 补上隐藏期间被跳过的 resize（FM / 回放图）
  activeTabName = name;
  try { localStorage.setItem(LAST_TAB_KEY, name); } catch (e) { /* 隐私模式忽略 */ }
}

document.querySelectorAll("#tabs .tab").forEach((btn) => {
  btn.addEventListener("click", () => activateTab(btn.dataset.tab));
});


/* ---------- 恢复上次页签（localStorage；首次进入=配置器） ---------- */
/* 注意：必须在 activateTab 定义之后调用，故放在文件末尾的 `DOMContentLoaded` 启动块里
 *（那时 config-form / fm-charts / replay / missile 四个文件也已执行，见文件头说明）。 */

/* ---------- 健康检查 + 托盘状态（启动块里调用：文案要等 i18n 就绪后才报） ---------- */
// 托盘可用性：`null` = 还没问到，`false` = 不可用（此时禁止「开始后隐藏窗口」，并右下角告警）
let trayAvailable = null;

async function checkService() {
  try {
    await api("/api/health");
    setMsg(t("msg.connected"));
  } catch (e) {
    report(t("msg.disconnected"), false);
  }
  try {
    const s = await api("/api/system");
    trayAvailable = !!s.tray_available;
    if (!trayAvailable) notify(t("msg.no_tray"), false, "tray");
  } catch (e) { /* 系统信息拿不到：托盘状态保持未知，不影响其它功能 */ }
}

/* ---------- 配置器：**DOM 引用与状态提示**（表单本身在 config-form.js） ----------
 * 这几个引用留在 app.js：`setMsg` 写 `#config-msg`、`setCfgActive` 写 `#config-current`，
 * 而 `setMsg` / `report` / `notify` 是所有页签共用的状态出口（toast 就在 `#notify`）。
 * `cfgSelect` / `cfgJson` 由 config-form.js 使用（同一条全局作用域，见文件头说明）。 */
const cfgSelect = $("#config-select");
const cfgJson = $("#config-json");
const cfgMsg = $("#config-msg");
const cfgCurrent = $("#config-current");
let cfgActive = null;

function setCfgActive(name) {
  cfgActive = name;
  // 底部状态区左侧显示当前配置名（元素缺失也不报错）
  if (cfgCurrent) cfgCurrent.textContent = name ? t("msg.current_config", { name }) : "";
}

function setMsg(text, ok = true) {
  if (!cfgMsg) return;
  cfgMsg.textContent = text;
  cfgMsg.className = `msg ${ok ? "ok" : "err"}`;
}

/** 统一的状态反馈：状态栏（只在配置器页签可见）+ 右下角 toast（所有页签可见）。
 *  状态栏挂在底部操作条上，而操作条只在配置器页签显示 —— 所以"只 setMsg"的提示
 *  在飞行模型/回放页签上一个字都看不到（「请先选择机型」「请先加入录制文件」都中招）。
 *  所有用户可见的反馈都走这里；`key` 非空时 toast 是替换式的，不会刷屏。 */
function report(text, ok = true, key = null, short = null) {
  setMsg(text, ok);
  notify(short || text, ok, key);
}


/** 右下角 toast 通知（#notify）：ok=绿 / err=红，3~4 秒自动消失，最多叠 4 条。
 *  key 非空时替换同 key 的既有条目（防自动保存类高频提示刷屏）。
 *  textContent 写入 —— 不走 innerHTML，天然免注入。 */
function notify(msg, ok = true, key = null) {
  const box = $("#notify");
  if (!box) return;
  const arm = (el) => {
    if (el._t) clearTimeout(el._t); // 重置消失倒计时
    el._t = setTimeout(() => { if (el.parentNode) el.parentNode.removeChild(el); }, ok ? 3000 : 4000);
  };
  if (key) {
    const prev = box.querySelector(`[data-key="${key}"]`);
    if (prev) {
      prev.className = `notify ${ok ? "ok" : "err"}`;
      prev.textContent = msg;
      arm(prev);
      return;
    }
  }
  while (box.children.length >= 4) box.removeChild(box.firstElementChild);
  const el = document.createElement("div");
  el.className = `notify ${ok ? "ok" : "err"}`;
  if (key) el.dataset.key = key;
  el.textContent = msg;
  box.appendChild(el);
  arm(el);
}
/* 曲线/轨迹的系列配色（原本 FM_PALETTE 与 RP_COLORS 逐字节相同、另有一处硬编码
 * "#2f6fed" 兜底 → 三份定义会漂移）。按下标取模，第 8 个对象回到第 1 个颜色。 */
const SERIES_COLORS = ["#2f6fed", "#c0392b", "#2e8b57", "#8e44ad", "#e07b00", "#0e7490", "#7a5c3e"];
const seriesColor = (i) => SERIES_COLORS[((i % SERIES_COLORS.length) + SERIES_COLORS.length) % SERIES_COLORS.length];
/* 3D 盒子的共同外观：两处只差 boxWidth/boxDepth */
const GRID3D_BASE = {
  boxHeight: 70,
  light: { main: { intensity: 1.2, shadow: true }, ambient: { intensity: 0.35 } },
  viewControl: { autoRotate: false, rotateSensitivity: 1.5, zoomSensitivity: 1.3 },
};

const AXIS_STYLE = {
  nameTextStyle: { color: "#8c8c86" },
  axisLine: { lineStyle: { color: "#b9b9b4" } },
  splitLine: { lineStyle: { color: "#e6e6e0" } },
};

let glProbeResult = null;
function echartsGlOk() {
  if (glProbeResult !== null) return glProbeResult;
  glProbeResult = false;
  try {
    const div = document.createElement("div");
    div.style.cssText = "width:8px;height:8px;position:absolute;left:-9999px";
    document.body.appendChild(div);
    const c = echarts.init(div);
    c.setOption({
      xAxis3D: { type: "value" }, yAxis3D: { type: "value" }, zAxis3D: { type: "value" },
      grid3D: {},
      series: [{ type: "surface", data: [[0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 1]] }],
    });
    glProbeResult = true;
    c.dispose();
    document.body.removeChild(div);
  } catch (e) {
    glProbeResult = false;
  }
  return glProbeResult;
}

/** 在图表容器里显示占位（加载中 / 空）—— ECharts 容器内放 HTML 即可，setOption 后自动被覆盖 */
function chartPlaceholder(el, text, loading = false) {
  if (!el) return;
  // 关键：容器里若有 ECharts 实例，必须先 dispose —— 否则写 innerHTML 会删掉 canvas，
  // 实例变成"已销毁但引用还在"，后续 setOption 静默失效（曾导致清空后图表再也画不出来）
  if (window.echarts) {
    const inst = echarts.getInstanceByDom(el);
    if (inst) inst.dispose();
  }
  el.innerHTML = "";
  const box = document.createElement("div");
  box.className = "empty";
  if (loading) {
    const sp = document.createElement("div");
    sp.className = "spinner";
    box.appendChild(sp);
  } else {
    const ico = document.createElement("span");
    ico.className = "ico";
    ico.textContent = "◌";
    box.appendChild(ico);
  }
  const tip = document.createElement("span");
  tip.className = "tip";
  tip.textContent = text;
  box.appendChild(tip);
  el.appendChild(box);
}

/** 取可用的图表实例：被 dispose 过就重新 init（占位函数会 dispose 旧实例） */
function liveChart(ref, id) {
  const el = document.getElementById(id);
  if (!el || !window.echarts) return ref;
  if (ref && !ref.isDisposed()) return ref;
  return echarts.init(el);
}

/* ---------- 图表统一主题：暖灰底、青绿首色、浅网格、11px 轴字 ---------- */
const CHART_COLORS = ["#2e7d6f", "#c1873b", "#4a6fa5", "#9c5b7a", "#5b8a72", "#8a8a80"];
function chartTheme() {
  return {
    color: CHART_COLORS,
    textStyle: { fontFamily: "Sarasa Mono SC, Microsoft YaHei, sans-serif", fontSize: 11, color: "#4a4a46" },
    grid: { left: 56, right: 18, top: 34, bottom: 40, containLabel: false },
    legend: { top: 4, itemWidth: 12, itemHeight: 8, textStyle: { fontSize: 11, color: "#4a4a46" } },
    tooltip: {
      backgroundColor: "rgba(255,255,255,.97)",
      borderColor: "#e6e6df",
      borderWidth: 1,
      textStyle: { color: "#1c1c1c", fontSize: 11.5 },
      extraCssText: "box-shadow:0 6px 24px rgba(28,28,28,.12);border-radius:8px;",
    },
    categoryAxis: {
      axisLine: { lineStyle: { color: "#d8d8d1" } },
      axisTick: { show: false },
      axisLabel: { color: "#8f8f88", fontSize: 11 },
      splitLine: { show: false },
    },
    valueAxis: {
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: { color: "#8f8f88", fontSize: 11 },
      splitLine: { lineStyle: { color: "#eeece5" } },
    },
  };
}
// 把主题合并进 option（不覆盖各图自己的配置）
function withTheme(option) {
  const th = chartTheme();
  return Object.assign({}, th, option, {
    textStyle: Object.assign({}, th.textStyle, option.textStyle || {}),
  });
}

/* ---------- 统一 resize（单监听，避免重复注册） ---------- */
/** 容器是否可见：隐藏页签（display:none）里 clientWidth/Height = 0，
 *  此时 resize() 会让 zrender 把画布量成 0×0（FM 图每次加载会重新 init 自愈，
 *  回放图没有这条路）→ 不可见就跳过，切回页签时再补一次。 */
function chartVisible(el) {
  return !!el && el.clientWidth > 0 && el.clientHeight > 0;
}

/** 切回页签时给该页签的图表补一次 resize：窗口可能是在别的页签上被改尺寸的
 *（resize 被跳过），实例还是旧尺寸、甚至是 0×0 初始化出来的。 */
function resizeTabCharts(name) {
  if (!window.echarts) return;
  if (name === "fm") {
    fmEachChart((node) => {
      if (!chartVisible(node)) return;
      const c = echarts.getInstanceByDom(node);
      if (c && !c.isDisposed()) c.resize();
    });
  } else if (name === "replay") {
    const node = document.getElementById("rp-chart");
    if (chartVisible(node) && rp.chart && !rp.chart.isDisposed()) rp.chart.resize();
  }
}

window.addEventListener("resize", () => {
  // vendor 里没有 echarts 时，裸调 echarts.* 会在每次改窗口大小抛 ReferenceError
  if (!window.echarts) return;
  fmEachChart((node) => {
    if (!chartVisible(node)) return;
    const c = echarts.getInstanceByDom(node);
    if (c && !c.isDisposed()) c.resize();
  });
  const rpNode = document.getElementById("rp-chart");
  if (chartVisible(rpNode) && rp.chart && !rp.chart.isDisposed()) rp.chart.resize();
});

/* ---------- 启动 ----------
 * 放在 `DOMContentLoaded` 里（不能写在脚本末尾直接跑）：启动要调 `loadConfigList`、
 * `activateTab` → `loadAircraft` / `loadReplayFiles` 这些后面才加载的函数，直接跑会 ReferenceError。
 *
 * i18n 必须排在最前：配置器整块是 `renderForm()` 按 `t()` 生成的，晚一步就会先用 HTML 里的
 * 中文渲染再被替换（闪一下 + 白费一次渲染）。 */
document.addEventListener("DOMContentLoaded", async () => {
  try {
    const pref = uiLangPref();
    await fetchI18n(pref === "auto" ? "" : pref);
    applyI18n();
    fillLangSelect();
    const sel = $("#lang-select");
    if (sel) sel.addEventListener("change", (e) => setUiLang(e.target.value, true));
  } catch (e) {
    // 文案表拿不到（缺 resource/i18n 或接口异常）：保持 HTML 原文、界面照旧可用，
    // 只在控制台留一条线索 —— 不要把界面变成一堆键名。
    console.error("i18n 加载失败，界面保持 HTML 原文：", e);
  }
  // 字体清单（`resource/fonts/`）也要在第一次 renderForm 之前拿到：字体下拉是
  // renderForm 直接拼进标题行的，晚一步就得整表重渲染。
  await loadFonts();
  // C/D：HUD 语言清单（`resource/lang/`）与语音包清单（`resource/voice/` 的子目录）同理 ——
  // 它们也是 renderForm 直接拼进「告警」卡片的两个下拉，必须在第一次渲染前就绪。
  await loadHudLangs();
  await loadVoicePacks();
  checkService();
  loadConfigList();
  // 恢复上次停留的页签（localStorage；首次进入 = 配置器）
  try {
    const lastTab = localStorage.getItem(LAST_TAB_KEY);
    if (lastTab && lastTab !== "config") activateTab(lastTab);
  } catch (e) { /* localStorage 不可用时保持默认页签 */ }
});
