/* wp8f 控制台前端 —— **导弹模拟页签**
 * 零构建 classic script：共享全局作用域，依赖 app.js 先加载（`$` / `report` / `setMsg` / `t`）。
 */
/* ---------- 导弹模拟（离线 HTML，子模块 gui/static/vendor/wt-missile-simulator） ----------
 * 首次切到该页签才加载（Three.js + 导弹数据 ~800KB，不拖慢启动）；子模块没初始化时
 * /static/... 会 404，此时给出 git submodule 初始化提示而不是空白页。 */
const MS_URL = "/static/vendor/wt-missile-simulator/index.html";
let msState = "idle";     // idle | loading | ok | missing
let msGen = 0;            // 加载代次：切走 / 重新加载都 +1，用来作废"在途"的那次加载
function msPlaceholder(html) {
  const holder = $("#ms-holder");
  if (holder) holder.innerHTML = `<div class="empty">${html}</div>`;
}
function msPlaceholderTip(text) {
  return `<span class="ico">🚀</span><span class="tip">${text}</span>`;
}
/** 切出页签：卸载 iframe（浏览器销毁浏览上下文 → 渲染循环/计时器全停），
 *  下次切回来由 `ensureMissile()` 重新加载。 */
function closeMissile() {
  const holder = $("#ms-holder");
  if (!holder) return;
  msGen++;                 // 在途加载作废：fetch 回来时不会再往这里塞 iframe
  if (msState === "idle") return;
  holder.innerHTML = `<div class="empty">${msPlaceholderTip(t("ms.not_loaded"))}</div>`;
  msState = "idle";
  setMsg(t("ms.closed"));
}
async function ensureMissile(force) {
  const holder = $("#ms-holder");
  if (!holder) return;
  if (msState === "ok" && !force) return;
  if (msState === "loading") return;
  const gen = ++msGen;
  msState = "loading";
  try {
    const r = await fetch(MS_URL, { cache: "no-store" });
    // 加载期间切走了（closeMissile 已作废本次加载）→ 放弃，别把 iframe 塞回隐藏页签
    if (gen !== msGen) return;
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    holder.innerHTML = `<iframe id="ms-frame" title="${esc(t("nav.missile"))}" src="${MS_URL}"></iframe>`;
    const fr = document.getElementById("ms-frame");
    fr.addEventListener("load", () => {
      if (gen !== msGen) return;         // 期间被切走：保持 idle，别改状态
      msState = "ok";
      setMsg(t("ms.loaded"));
    });
    setMsg(t("ms.loading"));
  } catch (e) {
    if (gen !== msGen) return;
    msState = "missing";
    const tip = esc(t("ms.missing")).replace(/&lt;br&gt;/g, "<br>");   // 文案里的 <br> 是排版
    msPlaceholder(msPlaceholderTip(tip));
    report(t("ms.not_ready"), false);
  }
}
const msReload = $("#ms-reload");
if (msReload) {
  msReload.addEventListener("click", () => {
    msState = "idle";
    msPlaceholder(msPlaceholderTip(t("ms.reloading")));
    ensureMissile(true);
  });}
const msHelp = $("#ms-help");
if (msHelp) {
  msHelp.addEventListener("click", () => {
    notify(t("ms.controls"), true, "ms-help");
  });
}
