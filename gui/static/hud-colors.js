/* wp8f 控制台前端 —— **HUD 颜色口径**（配置与 CSS 同为 #RRGGBBAA，RGBA 顺序）
 * 零构建 classic script：共享全局作用域，依赖 app.js 先加载（只用它的 `esc`）。
 */
/* ---------- 颜色：配置与 CSS 同为 #RRGGBBAA（RGBA 顺序） ----------
 * 前 6 位是 RGB，**末两位 AA 就是透明度**（00=全透明、FF=不透明）；HUD 把 AA 当真 alpha
 * 直通写出，预览所见即 HUD 实际显示。只写 6 位 #RRGGBB 视为不透明。
 * 拆通道必须按 RGBA 读：按 ARGB 读会让 8 位色整块读错（预览色与 HUD 实际颜色对不上）。
 * 这两个函数（含 parseHudColor）是纯字符串处理，不碰 DOM —— 可直接在 Node 里测。
 */
function parseHudColor(v) {
  const s = String(v == null ? "" : v).trim().replace(/^#/, "");
  if (/^[0-9a-f]{6}$/i.test(s)) {
    return { r: parseInt(s.slice(0, 2), 16), g: parseInt(s.slice(2, 4), 16),
             b: parseInt(s.slice(4, 6), 16), a: 255, hasAlpha: false };
  }
  if (/^[0-9a-f]{8}$/i.test(s)) {
    return { r: parseInt(s.slice(0, 2), 16), g: parseInt(s.slice(2, 4), 16),
             b: parseInt(s.slice(4, 6), 16), a: parseInt(s.slice(6, 8), 16), hasAlpha: true };
  }
  return null;
}
/** 配置色 → CSS 颜色：8 位用 rgba()（a∈0..1，保留 3 位小数），6 位直接给 #RRGGBB。 */
function hudColorCss(v) {
  const c = parseHudColor(v);
  if (!c) return "#ffffff";                     // 非法值：HUD 会整体回退默认配置
  const hex = (n) => n.toString(16).toUpperCase().padStart(2, "0");
  if (!c.hasAlpha) return `#${hex(c.r)}${hex(c.g)}${hex(c.b)}`;
  const a = Math.round((c.a / 255) * 1000) / 1000;
  return `rgba(${c.r},${c.g},${c.b},${a})`;
}
/** 色块/输入框的 tooltip（色值 → rgb + 透明度），文案走 i18n（`t` 来自 app.js）。 */
function hudColorTip(v) {
  const raw = String(v == null ? "" : v).trim();
  const c = parseHudColor(v);
  if (!c) return t("cfg.bad_color", { value: raw });
  const rgb = `rgb(${c.r},${c.g},${c.b})`;
  if (!c.hasAlpha || c.a === 255) return t("color.opaque", { value: raw, rgb });
  if (c.a === 0) return t("color.transparent", { value: raw, rgb });
  return t("color.alpha", { value: raw, rgb, pct: Math.round((c.a / 255) * 100) });
}
