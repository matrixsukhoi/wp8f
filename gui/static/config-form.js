/* wp8f 控制台前端 —— 配置器页签
 * 零构建 classic script：共享全局作用域，依赖 app.js 先加载
 *（`$` / `api` / `esc` / `t` / `setMsg` / `report` / `notify` / `hudColor*`）。
 * 可见文案都走 `t("cfg.…")`，所以 CSS/探针挂的钩子是语言无关的 `data-card-id`
 *（`data-card` 保留 = 当前语言标题）。 */

/** `resource/fonts/` 下的字体清单（`GET /api/fonts`，启动时取一次）。
 *  只列目录里真实存在的字体（没有"默认/内置"这种合成项）。 */
let FONT_LIST = [];

async function loadFonts() {
  try {
    const d = await api("/api/fonts");
    FONT_LIST = Array.isArray(d) ? d : (d.fonts || []);
  } catch (e) {
    FONT_LIST = [];
    console.error("字体清单读取失败（字体下拉会是空的）：", e);
  }
}

/** 字体下拉的选项：**选项值 = `font_path` 要写的字符串**（`resource/fonts/<文件名>`，由
 *  `/api/fonts` 的 `path` 给出），选了原样写回配置 —— 没有"只写文件名、HUD 自动去目录里拼"
 *  这种隐式行为。
 *
 *  配置里的值不在目录里时（空 / 写错 / 文件被删）补一个占位项把问题摆在眼前：
 *  HUD 对这两种情况都是**报错退出**，不回退到别的字体。 */
function fontOptions(selected) {
  const cur = String(selected ?? "");
  const known = FONT_LIST.some((f) => f.path === cur || f.name === cur);
  const out = [];
  if (!known) {
    const label = cur ? t("font.not_in_dir", { name: cur }) : t("font.none");
    out.push(`<option value="${esc(cur)}" data-mono="1" selected>${esc(label)}</option>`);
  }
  for (const f of FONT_LIST) {
    const tip = f.mono ? t("font.tip.mono") : t("font.tip.not_mono");
    const sel = (f.path === cur || f.name === cur) ? " selected" : "";
    out.push(`<option value="${esc(f.path)}" data-mono="${f.mono ? 1 : 0}"` +
      ` title="${esc(f.name + " · " + tip)}"${sel}>${esc(f.name)}</option>`);
  }
  return out.join("");
}

/** HUD 标签语言（C）：`GET /api/hud-lang` —— `resource/lang/` 下**真实存在**的语言文件。
 *  **只有一个配置键** `hud_lang_path`（路径）：写哪个 json，HUD 就读哪个（core 启动时读）。
 *  与左栏的 GUI 语言下拉**互不影响**：那个写 `language`（15 种界面语言）。 */
let HUD_LANG_FILES = []; // [{file,path,name}]
let HUD_LANG_DIR = "resource/lang"; // 由后端给（与 disp/src/i18n.rs::LANG_DIR 同源）

async function loadHudLangs() {
  try {
    const d = await api("/api/hud-lang");
    HUD_LANG_FILES = Array.isArray(d.files) ? d.files : [];
    if (d.dir) HUD_LANG_DIR = d.dir;
  } catch (e) {
    HUD_LANG_FILES = [];
    console.error("HUD 语言文件清单读取失败（候选提示会是空的；手写路径仍可用）：", e);
  }
}

/** 配置里**没写** `hud_lang_path` 时 HUD 实际会读的那份文件（老配置口径：按 GUI 的
 *  `language` 映射 —— 中文系 → zh，其余 → en，与 HUD 侧 `pick()` 同一口径）。
 *  `document.documentElement.lang` 是解析过 `auto` 之后的生效语言，所以这里不用管 auto。
 *  只用于"留空时选中哪一项"，不写回配置。 */
function effectiveHudLangPath(cfgValue) {
  const cur = String(cfgValue ?? "").trim();
  if (cur) return cur;
  const g = String(document.documentElement.lang || "").trim().toLowerCase().replace(/_/g, "-");
  const code = (!g || g === "zh" || g.startsWith("zh-")) ? "zh" : "en";
  return `${HUD_LANG_DIR}/${code}.json`;
}

/** 候选的显示文字：只有文件名主干（`zh` / `en`）—— 不带路径、不带 `.json`、不加括号注释。
 *  配置里写的仍是完整路径（见 `hudLangOptions`）。 */
function hudLangOptionText(f) {
  return String(f.file || "").replace(/\.json$/i, "");
}

/** HUD 语言文件下拉的选项：**值 = `hud_lang_path` 要写的完整相对路径**（`resource/lang/zh.json`），
 *  显示 = 上面那个短标签。配置里的值不在目录里（自建路径 / 文件被删）时补一个占位项原样摆出来。 */
function hudLangOptions(cfgValue) {
  const cur = String(cfgValue ?? "").trim();
  const eff = effectiveHudLangPath(cur);
  const out = [];
  if (cur && !HUD_LANG_FILES.some((f) => f.path === cur)) {
    out.push(`<option value="${esc(cur)}" selected>${esc(cur)}</option>`);
  }
  for (const f of HUD_LANG_FILES) {
    const sel = (f.path === cur || (!cur && f.path === eff)) ? " selected" : "";
    out.push(`<option value="${esc(f.path)}"${sel}>${esc(hudLangOptionText(f))}</option>`);
  }
  return out.join("");
}

/** 语音包（D）：`GET /api/voice-packs` —— `resource/voice/` 下的**子目录**。
 *  选项值 = `voice_path` 要写的字符串（`resource/voice/<子目录名>`，由后端给 `path`）；
 *  配置里的值不在目录里时补一个占位项（HUD 会照旧启动，只是那个包里读不到 wav → 静音）。 */
let VOICE_PACKS = [];    // [{name,path,wavs,bytes}]
let VOICE_LOOSE = [];    // 散在 resource/voice/ 根上的 .wav（D 之后不会被加载）

async function loadVoicePacks() {
  try {
    const d = await api("/api/voice-packs");
    VOICE_PACKS = Array.isArray(d.packs) ? d.packs : [];
    VOICE_LOOSE = Array.isArray(d.loose) ? d.loose : [];
  } catch (e) {
    VOICE_PACKS = [];
    VOICE_LOOSE = [];
    console.error("语音包清单读取失败（语音包下拉会是空的）：", e);
  }
}

function voicePackOptions(selected) {
  const cur = String(selected ?? "");
  const known = VOICE_PACKS.some((p) => p.path === cur || p.name === cur);
  const out = [];
  if (!known) {
    out.push(`<option value="${esc(cur)}" selected>` +
      `${esc(t("cfg.opt.voice_pack.missing", { name: cur || t("cfg.opt.voice_pack.empty") }))}</option>`);
  }
  for (const p of VOICE_PACKS) {
    const sel = (p.path === cur || p.name === cur) ? " selected" : "";
    out.push(`<option value="${esc(p.path)}"${sel} title="${esc(t("cfg.tip.voice_pack", { n: p.wavs, kb: Math.round(p.bytes / 1024) }))}">` +
      `${esc(p.name)}（${p.wavs}）</option>`);
  }
  return out.join("");
}

/** 配置键的写法（与表单控件同一条路径：改 JSON → 自动保存）。
 *  语言下拉在左栏（不在配置器表单里），它要写的就是这个函数。 */
function setConfigValue(key, value) {
  try {
    const c = JSON.parse(cfgJson.value);
    const keys = key.split(".");
    let o = c;
    for (let i = 0; i < keys.length - 1; i++) o = o[keys[i]] = o[keys[i]] ?? {};
    o[keys[keys.length - 1]] = value;
    cfgJson.value = JSON.stringify(c, null, 2);
    scheduleAutoSave();
  } catch (e) { /* JSON 半成品：忽略（自动保存会兜底） */ }
}

/** localStorage 里"**上次选中的配置文件**"（P9 用户要求：选了 mylayout.json，下次打开还停在它上面）。
 *  与 `app.js` 的 `LAST_TAB_KEY` 同一风格：只记**文件名**（配置内容仍走自动保存那条路，两者不混）。
 *  `getItem` 返回 `null`（没记过 / 被回退清掉）与 `""` 一视同仁。 */
const LAST_CONFIG_KEY = "wp8f.lastConfig";

/** 磁盘上那份配置的原文（`loadConfig` / 自动保存成功后更新）。
 *
 *  存在的理由：**配置有两个写者** —— 控制台（表单/JSON 编辑）与 HUD（预览里拖完面板
 *  按 ESC 时把位置写回）。以前控制台拿着打开时的旧副本整份写回，于是"拖动 mini → 切到
 *  圆环 → 再切回 mini"会把 mini 的位置复原（用户报的正是这个）。现在窗口重新获得焦点时
 *  会比对磁盘内容，被外部改过就重载（本地有未保存改动时不抢，见 `reloadIfChangedOnDisk`）。 */
let cfgTextOnDisk = null;

function lastConfigName() {
  try { return localStorage.getItem(LAST_CONFIG_KEY) || ""; } catch (e) { return ""; }
}
function rememberConfig(name) {
  if (!name) return;
  try { localStorage.setItem(LAST_CONFIG_KEY, name); } catch (e) { /* 隐私模式忽略 */ }
}
function forgetConfig() {
  try { localStorage.removeItem(LAST_CONFIG_KEY); } catch (e) { /* 隐私模式忽略 */ }
}

async function loadConfigList() {
  try {
    const names = await api("/api/config/list");
    const opts = names.map((n) => {
      const o = document.createElement("option");
      o.value = n;
      o.textContent = n;
      return o;
    });
    cfgSelect.replaceChildren(...opts);
    cfgSelect.title = t("cfg.dropdown_n", { n: names.length, names: names.join(", ") });
    if (names.length) {
// ① 记住的那份还在列表里 → 选它；
// ② 不在了（被删 / 改名）→ 回退 default.json（没有就取第一项），
      //    并**清掉记住的值**，否则每次启动都要走一遍"回退"这条路。
      // ⚠️ 只有用户在**下拉里主动切换**时才写这个键（见文件末尾的 change 监听）——
      //    回退路径不写，探针才能断言"回退后记住的值确实被清掉了"。
      const want = lastConfigName();
      let pick;
      if (want && names.includes(want)) {
        pick = want;
      } else {
        if (want) forgetConfig();
        pick = names.includes("default.json") ? "default.json" : names[0];
      }
      await loadConfig(pick);
      setMsg(`${cfgMsg.textContent}${t("cfg.dropdown_n_short", { n: names.length })}`);
    } else {
      setMsg(t("cfg.no_configs"), false);
    }
  } catch (e) {
    report(t("cfg.load_list_failed", { err: e }), false);
  }
}

/** 配置里的 `language` 键（缺键 → `""`，表示不干预界面语言）。 */
function configLanguage(content) {
  try {
    const v = JSON.parse(content).language;
    return typeof v === "string" ? v.trim() : "";
  } catch (e) { return ""; }
}

async function loadConfig(name) {
  try {
    const d = await api(`/api/config/${encodeURIComponent(name)}`);
    setCfgActive(name);
    cfgSelect.value = name;
    if (cfgCurrent) cfgCurrent.textContent = name;
    cfgJson.value = d.content;
    cfgTextOnDisk = d.content;   // 记住"盘上那份"，用于检测被外部（HUD 拖拽）改过
    // 这份配置指定的语言与当前界面语言不同 → 跟着配置走（左栏下拉与 localStorage 一起更新）。
    // setUiLang 内部会用新语言重渲染一次表单，下面的 renderForm 是幂等的第二次。
    const lang = configLanguage(d.content);
    if (lang && lang !== uiLangPref()) await setUiLang(lang, false);
    renderForm(d.content);
    setMsg(t("cfg.loaded", { name, size: d.content.length }));
  } catch (e) {
    report(t("cfg.load_failed", { name, err: e }), false);
  }
}

cfgSelect.addEventListener("change", (e) => {
  // 用户主动切换 → 记住它（只记文件名，见 LAST_CONFIG_KEY）
  rememberConfig(e.target.value);
  loadConfig(e.target.value);
});

function renderForm(content) {
  let cfg = {};
  try { cfg = JSON.parse(content); } catch (e) { return; }
  // 支持 "datalink.enabled" 这类嵌套键取值
  const getv = (obj, key) => key.split(".").reduce((o, k) => (o == null ? undefined : o[k]), obj);
  // 分类卡片（仅卡片标题画下划线；行内开关不画线）。
  // 顺序 = 排版顺序：8 张卡片两列 4 排，短卡片与长卡片配对，整页刚好一屏 ——
  // 加卡片/加行前先跑 layout_check 的"一屏放下"。
  // 键 = 语言无关的卡片 id（挂 `data-card-id`，CSS 与布局探针用），值 = 标题的 i18n 键。
  // `data-card` 仍写标题，是为了让既有探针的 `[data-card="布局"]` 在 zh-CN 下继续可用。
  const CARDS = [
    ["panel", "cfg.card.panel"], ["alert", "cfg.card.alert"], ["layout", "cfg.card.layout"],
    ["font", "cfg.card.font"], ["record", "cfg.card.record"],
    ["datalink", "cfg.card.datalink"], ["color", "cfg.card.color"],
  ];
  const groups = Object.fromEntries(CARDS.map(([id]) => [id, []]));
  // 卡片标题右侧的小字说明（占用标题行，不额外占一行高度）
  const CARD_HINT = {
    color: () => t("cfg.hint.color_format"),
    // 环形缓冲槽数（`ring.frames` / `ring.map_samples`）放**标题行**：加一行会让整页多 30px，
    // layout_check 的"一屏放下"放不下（与颜色卡片的格式提示、字体卡片的字体下拉同一手法）。
    panel: () => `${t("cfg.lbl.ring_slots")} ` + ringNum("ring.frames") + ringNum("ring.map_samples"),
  };
  // 标题行里的窄数字输入：缺键展示 32（= Rust 侧 `RingConfig::default()`），
  // 上下限 2–256 与 `RingConfig::{frames,map_samples}_clamped()` 同口径。
  const ringNum = (key) =>
    `<input class="ring-num" type="number" data-key="${key}" min="2" max="256" step="1" ` +
    `title="${esc(t("cfg.hint.ring_slots"))}" value="${Number(getv(cfg, key) ?? 32)}">`;
  // def：配置里缺这个键时的默认勾选状态（其它开关都是 false，只有语音告警缺省 true）
  const check = (g, label, key, def = false) => {
    const v = getv(cfg, key);
    groups[g].push(`<div class="item switch"><input type="checkbox" data-key="${key}" ${(v === undefined ? def : v) ? "checked" : ""}><label>${label}</label></div>`);
  };
  // `def`：配置里缺这个键时的**展示值**（必须与该键的 serde 默认值一致，
  // 否则老配置打开配置器会显示成 min，一碰就把错误的值写进配置）。
  const slider = (g, label, key, min, max, step, def) => {
    const v = Number(getv(cfg, key) ?? (def === undefined ? min : def));
    // **点右侧的数字**才能手动输入（上下限由 range 的 min/max 决定，越界会被夹紧）；
    // 左侧标签只显示名字、不再响应点击。
    groups[g].push(`<div class="item"><label>${label}</label><span class="sld"><input type="range" data-key="${key}" min="${min}" max="${max}" step="${step}" value="${v}"><b class="sval sval-edit" data-edit="${key}" data-min="${min}" data-max="${max}" data-step="${step}" title="${esc(t("cfg.input_range", { min, max }))}">${v}</b></span></div>`);
  };
  const color = (g, label, key) => {
    // 配置里缺这个键时的默认展示值：白色不透明（新语义下 6 位 = AA 按 FF 处理）
    const v = String(getv(cfg, key) ?? "#FFFFFF");
    // 色块在输入框左边（预览 → 数值，读起来更顺）
    groups[g].push(`<div class="item"><label>${label}</label><span class="sld"><i class="swatch" style="background:${esc(hudColorCss(v))}" title="${esc(hudColorTip(v))}"></i><input type="text" class="colortext" data-key="${key}" value="${esc(v)}"></span></div>`);
  };
  const textf = (g, label, key, type = "text") => {
    const v = getv(cfg, key) ?? "";
    groups[g].push(`<div class="item"><label>${label}</label><input type="${type}" data-key="${key}" value="${esc(String(v))}"></div>`);
  };
  /** 配置里的 core 数据刷新频率（Hz，钳 5..=60）：地图刷新帧间隔滑杆的上限由它决定。 */
  const refreshHzCfg = () => {
    const v = Math.round(Number(getv(cfg, "refresh_hz") ?? 30));
    return Math.min(Math.max(Number.isFinite(v) && v > 0 ? v : 30, 5), 60);
  };
  // HUD 类型：**三选一**（0 = 关闭 / 1 = MiniHUD / 2 = 圆环 HUD）—— 只写 `hud_type` 一个键。
  // 旧配置里的字符串写法（"off"/"mini"/"circle"）与两个 `*_enabled` 老开关这里都兼容着读，
  // 选中后统一写成数字（Rust 侧的 `HudType` 也接受旧写法，所以老配置不会读失败）。
  const HUD_MODES = [
    { v: 0, key: "cfg.opt.hud_off" },
    { v: 1, key: "cfg.opt.minihud" },
    { v: 2, key: "cfg.opt.circle_hud" },
  ];
  const hudModeOf = (cfg) => {
    const raw = getv(cfg, "hud_type");
    const s = String(raw ?? "").toLowerCase();
    if (s === "off" || s === "0") return 0;
    if (s === "mini" || s === "1") return 1;
    if (s === "circle" || s === "2") return 2;
    if (Number.isFinite(Number(raw))) return Number(raw) === 1 ? 1 : Number(raw) === 0 ? 0 : 2;
    // 老配置只有两个开关时按开关推：mini 开 → 1，否则圆环或全关
    if (getv(cfg, "minihud_enabled") === true) return 1;
    if (getv(cfg, "circle_enabled") === true) return 2;
    return 0;
  };
  const hudType = () => {
    const cur = hudModeOf(cfg);
    const radios = HUD_MODES.map((m) =>
      `<label class="radio"><input type="radio" name="hud_type" data-hudtype="${m.v}"${cur === m.v ? " checked" : ""}><span>${t(m.key)}</span></label>`
    ).join("");
    groups.panel.push(`<div class="item"><label>${t("cfg.hud_type")}</label><span class="radios">${radios}</span></div>`);
  };
  // 地图记录间隔（配置键 `map_obj_record_every_frames`）：**每多少个数据帧记录一次地图对象**
  // （单位 = 数据帧；毫秒 = 数据帧数 × 1000 / refresh_hz）。最小 1、默认 8，
  // **上限 = 1 秒的数据帧数 = 刷新率**。
  // 它同时就是**飞行记录的采样间隔**（记录频率已从配置里删除：地图坐标每这么多帧才更新一次，
  // 记更快只会写下坐标相同的重复帧），所以这张滑杆一改，`.wpr` 的采样率跟着变。
  // 与 Rust 同一条口径：`wp8f_disp::HudLayoutConfig::map_obj_record_every_frames_clamped`。
  const mapObjRecordEveryMax = (refreshHz) =>
    Math.min(Math.max(Math.round(Number(refreshHz) || 30), 5), 60);
  /** 「地图记录间隔（数据帧）」滑杆的联动夹紧：上限随 `refresh_hz` 变化。
   *  `writeBack = true` 才把夹紧后的值写回 JSON 并提示（用户改动触发）；
   *  首次渲染只夹**显示值** —— 不静默改配置，改动只由用户的手来触发。 */
  const syncMapObjRecordEvery = (writeBack) => {
    const form = $("#config-form");
    const r = form && form.querySelector('input[type=range][data-key="map_obj_record_every_frames"]');
    if (!r) return;
    let hz = refreshHzCfg();
    try {
      const c = JSON.parse(cfgJson.value);
      hz = Math.min(Math.max(Math.round(Number(c.refresh_hz) || hz), 5), 60);
    } catch (e) { /* JSON 半成品：退回表单上的刷新率 */ }
    const max = mapObjRecordEveryMax(hz);
    // **先读旧值再改 max**：给 range 设 max 时浏览器会把越界的 value 直接夹掉，
    // 夹完再读就永远"没超限"，联动夹紧与写回提示都会静默失效。
    const before = Math.round(Number(r.value) || 1);
    r.max = String(max);
    const v = Math.min(max, Math.max(1, before));
    if (v !== before) {
      r.value = String(v);
      if (writeBack) {
        try {
          const c = JSON.parse(cfgJson.value);
          c.map_obj_record_every_frames = v;
          cfgJson.value = JSON.stringify(c, null, 2);
          scheduleAutoSave();
          setMsg(t("cfg.msg.map_frames_clamped", {
            hz, max, value: v, ms: Math.round((v * 1000) / hz),
          }), false);
        } catch (e) { /* 忽略：自动保存会兜底 */ }
      }
    }
    const sval = r.parentElement && r.parentElement.querySelector(".sval");
    if (sval) {
      sval.textContent = String(r.value);
      sval.dataset.max = String(max);   // 手动输入框按这个上限夹紧（见 sval-edit 处理器）
      sval.title = t("cfg.tip.map_frames", { max });
    }
  };
  // 面板（开关 + HUD 类型 + core 数据刷新频率）
  check("panel", t("cfg.lbl.flight_panel"), "flight_info_enabled");
  check("panel", t("cfg.lbl.engine_panel"), "engine_info_enabled");
  check("panel", t("cfg.lbl.map"), "map_enabled");
  check("panel", t("cfg.lbl.datalink_panel"), "datalink_panel_enabled");
  hudType();
  slider("panel", t("cfg.lbl.refresh_hz"), "refresh_hz", 5, 60, 1, 30);
  // 地图记录间隔（每多少数据帧记录一次地图对象）：最小 1 = 每帧记录，缺键时展示 8
  // （= serde 默认 `MAP_OBJ_INTERVAL_FRAME`，Rust 侧由
  // `default_map_obj_record_every_frames()` 提供），上限 = 1 秒的数据帧数
  // （= 数据刷新率，随 refresh_hz 变；越界在 sync 里夹紧）。
  slider("panel", t("cfg.lbl.map_record_frames"), "map_obj_record_every_frames", 1,
         mapObjRecordEveryMax(refreshHzCfg()), 1, 8);
  // HUD 语言文件（C）：**core 启动时读的那份 json** —— 配置键 `hud_lang_path`。
  // 值写 `resource/lang/` 下的**完整相对路径**（`resource/lang/zh.json`），下拉里**只显示 `zh`/`en`**
// 候选 = `resource/lang/` 下真实存在的 `*.json`（含用户自建的）。
  groups.panel.push(`<div class="item"><label title="${esc(t("cfg.hint.hud_lang_path"))}">${t("cfg.lbl.hud_lang_path")}</label>` +
    `<select class="cfg-sel" data-key="hud_lang_path">` +
    `${hudLangOptions(getv(cfg, "hud_lang_path"))}</select></div>`);
  // 告警（单独一张卡：后续告警开关都加在这里；颜色卡片只放颜色）
  check("alert", t("cfg.lbl.voice"), "voice_warnings_enabled", true);
  // 机动告警声（F-18 风格蜂鸣/长鸣）：与语音告警是两个独立音层，分开开关
  check("alert", t("cfg.lbl.maneuver_tone"), "maneuver_tone_enabled", true);
  slider("alert", t("cfg.lbl.blink_hz"), "warning_blink_hz", 0, 20, 1);
  // 语音包（D）：`resource/voice/` 下的**子目录**（放一个新子目录即可新增语音包）。
  groups.alert.push(`<div class="item"><label title="${esc(t("cfg.hint.voice_pack"))}">${t("cfg.lbl.voice_pack")}</label>` +
    `<select class="cfg-sel" data-key="voice_path">` +
    `${voicePackOptions(getv(cfg, "voice_path"))}</select></div>`);
  // 布局（列数与尺寸，尺寸上限 1440px）
  slider("layout", t("cfg.lbl.flight_cols"), "flight_info_col", 2, 10, 1);
  slider("layout", t("cfg.lbl.engine_cols"), "engine_info_col", 1, 10, 1);
  slider("layout", t("cfg.lbl.map_w"), "map_size_x", 100, 1440, 10);
  slider("layout", t("cfg.lbl.map_h"), "map_size_y", 100, 1440, 10);
  slider("layout", t("cfg.lbl.circle_r"), "circle_ring_radius", 20, 1440, 2);
  // 字体（字号上限 72；标签按 4 个中文字符对齐，见 style.css 的 data-card-id="font" 规则）
  // 字体下拉**挂在卡片标题行**（与 .h3-hint 同一手法）：加一行会让整页多 30px，
// layout_check 的"一屏放下"很紧（见 style.css 里颜色卡片的同款注释）。
  slider("font", t("cfg.lbl.font_size"), "font_size", 8, 72, 1);
  slider("font", t("cfg.lbl.circle_font"), "circle_font_size", 8, 72, 1);
  slider("font", t("cfg.lbl.map_font"), "map_font_size", 8, 72, 1);
  slider("font", t("cfg.lbl.mini_font"), "mini_font_size", 8, 72, 1);
// 颜色（全部颜色项 + 预览色块）
// 只有 8 项：地图与圆环用数字色，POI 十字与告警 X 用 hint_color / data_color。
  //    POI 十字用**警示色**）—— 旧配置文件里还写着这几个键也不会再被读进来。
  color("color", t("cfg.lbl.data_color"), "data_color");
  color("color", t("cfg.lbl.label_color"), "label_color");
  color("color", t("cfg.lbl.unit_color"), "unit_color");
  color("color", t("cfg.lbl.hint_color"), "hint_color");
  color("color", t("cfg.lbl.alert_color"), "alert_color");
  color("color", t("cfg.lbl.map_bg"), "map_bg_color");
  color("color", t("cfg.lbl.stroke_color"), "stroke_color");
  // 数据链
  check("datalink", t("cfg.lbl.enabled"), "datalink.enabled");
  textf("datalink", t("cfg.lbl.server"), "datalink.server");
  textf("datalink", t("cfg.lbl.port"), "datalink.port", "number");
  textf("datalink", t("cfg.lbl.key"), "datalink.key");
  textf("datalink", t("cfg.lbl.session"), "datalink.session_id", "number");
  textf("datalink", t("cfg.lbl.send_hz"), "datalink.send_hz", "number");
  // 飞行记录（原「TacView」卡片；启用了才展开子项）
  // 飞行记录（`.wpr`：地图信息 + 底图 + 逐帧 DisplayData；导出 ACMI/CSV 在回放页签）
  // **没有"记录频率"这一项**：采样间隔跟随「地图刷新帧间隔」（见上面那张滑杆）。
  check("record", t("cfg.lbl.record"), "record.enabled");
  // 记录内存池的**初始**容量（MiB，默认 8）：记录期间只往内存池追加 CSV 行、结束才写盘
  slider("record", t("cfg.lbl.pool_mb"), "record.pool_mb", 1, 64, 1, 8);
  $("#config-form").innerHTML = CARDS
    .map(([id, titleKey]) => ({ id, title: t(titleKey), rows: groups[id] }))
    .filter((c) => c.rows.length)
    .map(({ id, title, rows }) => {
      const cls = "card";
      const hint = CARD_HINT[id] ? `<span class="h3-hint">${CARD_HINT[id]()}</span>` : "";
      // 字体卡片：字体下拉放标题行（`font_path` 是配置键，走通用的 data-key 处理器）
      const hSel = id === "font"
        ? `<select class="h3-sel" data-key="font_path" title="${esc(t("cfg.lbl.font_family"))}">` +
          `${fontOptions(String(getv(cfg, "font_path") ?? ""))}</select>`
        : "";
      // 数据链 / 飞行记录：勾选"启用"后才展开子项
      if (id === "datalink" || id === "record") {
        const enKey = id === "datalink" ? "datalink.enabled" : "record.enabled";
        const open = getv(cfg, enKey) ? "grid" : "none";
        return `<div class="${cls}" data-card="${esc(title)}" data-card-id="${id}"><h3>${esc(title)}${hint}${hSel}</h3><div class="grid">${rows[0]}</div>` +
          `<div class="grid sub" data-sub="${enKey}" style="display:${open}">${rows.slice(1).join("")}</div></div>`;
      }
      return `<div class="${cls}" data-card="${esc(title)}" data-card-id="${id}"><h3>${esc(title)}${hint}${hSel}</h3><div class="grid">${rows.join("")}</div></div>`;
    })
    .join("");

  $("#config-form").querySelectorAll("input, select").forEach((inp) => {
    // 单选组（HUD 类型）自己没有 data-key：它的事件处理器在下面单独绑
    if (!inp.dataset.key || inp.type === "radio") return;
    const handler = () => {
      try {
        const c = JSON.parse(cfgJson.value);
        const keys = inp.dataset.key.split(".");
        let o = c;
        for (let i = 0; i < keys.length - 1; i++) o = o[keys[i]] = o[keys[i]] ?? {};
        let v = inp.type === "checkbox" ? inp.checked : inp.value;
        if (inp.type === "number" || inp.type === "range") v = Number(v);
        o[keys[keys.length - 1]] = v;
        cfgJson.value = JSON.stringify(c, null, 2);
        const wrap = inp.parentElement;
        const sval = wrap && wrap.querySelector ? wrap.querySelector(".sval") : null;
        if (sval) sval.textContent = inp.value;
        const sw = wrap && wrap.querySelector ? wrap.querySelector(".swatch") : null;
        // 只在颜色行存在色块；值允许不带 #，所以不再要求 startsWith("#")
        if (sw && typeof inp.value === "string") {
          sw.style.background = hudColorCss(inp.value);
          sw.title = hudColorTip(inp.value);
        }
        // 颜色框校验：非法串会让 HUD 整份配置静默回退到内置默认配色（core 只在 stderr 打一行）
        if (inp.classList && inp.classList.contains("colortext")) {
          const okColor = /^#?([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/.test(String(inp.value).trim());
          inp.classList.toggle("bad", !okColor);
          if (!okColor) {
            setMsg(t("cfg.bad_color", { value: inp.value }), false);
          }
        }
        // 字体下拉：非等宽字体**每次选择都提示**（用户刚做了动作，不会被当成噪音）
        if (inp.classList && inp.classList.contains("h3-sel")) {
          const opt = inp.selectedOptions && inp.selectedOptions[0];
          if (opt && opt.dataset.mono === "0") {
            notify(t("font.warn.not_mono", { name: inp.value }), false, "font-mono");
          }
        }
        scheduleAutoSave();
        // 启用开关 → 展开/收起子项（数据链 / 飞行记录）
        const key = inp.dataset.key || "";
        if (key === "datalink.enabled" || key === "record.enabled") {
          const sub = inp.closest(".card") && inp.closest(".card").querySelector(".sub");
          if (sub) sub.style.display = inp.checked ? "grid" : "none";
        }
        // 地图记录间隔 / 数据刷新频率变了 → 地图记录间隔的上限跟着变
        // （上限 = 1 秒的数据帧数 = 刷新率；值超上限会写回 JSON 并提示）。
        if (key === "map_obj_record_every_frames" || key === "refresh_hz") {
          syncMapObjRecordEvery(true);
        }
      } catch (e) { setMsg(t("cfg.sync_failed", { err: e }), false); }
    };
    // 每个控件只绑一个事件：同一处理器绑 input+change 会让一次编辑跑两遍
    //（JSON 序列化 ×2 + 自动保存倒计时被重置两次）。range 只有 input 会连续触发，
    // 文本/数字同理；select / 复选框用 change 才是"选定完成"的语义。
    if (inp.tagName === "SELECT" || inp.type === "checkbox") {
      inp.addEventListener("change", handler);
    } else {
      inp.addEventListener("input", handler);
    }
  });

  // 配置里的字体在本机 `resource/fonts/` 里不存在（或压根没写）→ **HUD 会报错退出**
  //（`disp/src/font.rs`：`font_path` 必填，没有"回退默认字体"这回事）。
  // 这里明说一句，别让用户以为"选了却没生效"或者"HUD 自己会找一份"。渲染时提示一次。
  const wantFont = String(getv(cfg, "font_path") ?? "");
  const fontKnown = FONT_LIST.some((f) => f.path === wantFont || f.name === wantFont);
  if (!fontKnown) {
    notify(wantFont ? t("font.warn.missing", { name: wantFont }) : t("font.warn.empty"),
           false, "font-missing");
  }

  // HUD 类型单选组：三选一（0 关闭 / 1 MiniHUD / 2 圆环 HUD），只写 `hud_type`；
  // 顺手把两个老开关从配置里删掉（Rust 侧已不看它们，留着只会让人以为还有第二处开关）。
  $("#config-form").querySelectorAll('input[type=radio][data-hudtype]').forEach((r) => {
    r.addEventListener("change", () => {
      if (!r.checked) return;
      try {
        const c = JSON.parse(cfgJson.value);
        const mode = Number(r.dataset.hudtype);
        c.hud_type = mode;
        delete c.minihud_enabled;
        delete c.circle_enabled;
        cfgJson.value = JSON.stringify(c, null, 2);
        scheduleAutoSave();
        const label = HUD_MODES.find((m) => m.v === mode);
        setMsg(t("cfg.hud_type_set", { type: t(label ? label.key : "cfg.opt.hud_off") }), true);
      } catch (e) { setMsg(t("cfg.sync_failed", { err: e }), false); }
    });
  });

  // 记录频率（`record.poll_ms`）已经删除：采样间隔跟随「地图刷新帧间隔」，
  // 由 core 用 `mapobj::period_ms(帧数, refresh_hz)` 现算，这里不再有对应控件。


  // 点右侧数字 → 手动输入数值：临时在数字位置放一个 number 输入框，
  // 回车/失焦提交（夹紧到 min–max 后写回 range，复用上面的同步逻辑），Esc 取消。
  // 左侧标签**不再**响应点击（用户要求：输入入口在滑块右边的数字上）。
  $("#config-form").querySelectorAll("b.sval-edit").forEach((sval) => {
    sval.addEventListener("click", () => {
      if (sval.dataset.editing === "1") return;
      const row = sval.closest(".item");
      const range = row && row.querySelector('input[type="range"]');
      if (!range) return;
      const min = Number(sval.dataset.min), max = Number(sval.dataset.max);
      const step = Number(sval.dataset.step) || 1;
      const box = document.createElement("input");
      box.type = "number";
      box.className = "lbl-input";
      box.min = String(min);
      box.max = String(max);
      box.step = String(step);
      box.value = range.value;
      box.title = t("cfg.input_range", { min, max });
      sval.dataset.editing = "1";
      sval.style.display = "none";
      sval.after(box);
      box.focus();
      box.select();

      let done = false;
      const finish = (commit) => {
        if (done) return;
        done = true;
        if (commit) {
          const typed = Number(box.value);
          if (Number.isFinite(typed)) {
            // range 会自动夹紧到 [min,max]，这里显式判断一下以便提示用户
            const clamped = Math.min(max, Math.max(min, typed));
            range.value = String(clamped);
            range.dispatchEvent(new Event("input", { bubbles: true }));
            if (clamped !== typed) {
              setMsg(t("cfg.clamped", { value: clamped, min, max }), false);
            }
          }
        }
        box.remove();
        sval.style.display = "";
        delete sval.dataset.editing;
      };
      box.addEventListener("keydown", (ev) => {
        if (ev.key === "Enter") { ev.preventDefault(); finish(true); }
        else if (ev.key === "Escape") { ev.preventDefault(); finish(false); }
      });
      box.addEventListener("blur", () => finish(true));
    });
  });

  // 首次渲染：把地图记录间隔的滑杆按 **1 秒的数据帧数（= 数据刷新率）** 的上限**夹显示值**
  //（配置里写了超上限的值时，滑块不会显示成越界值；不在这里静默改配置，改动只由用户触发）
  syncMapObjRecordEvery(false);
}

// 自动保存（防抖 500ms）：任何修改即自动落盘，无需手动保存
let autoSaveTimer = null;
/** 上一次已经提示过的保存警告（同一条不重复弹） */
let lastSaveWarn = "";
function scheduleAutoSave() {
  if (autoSaveTimer) clearTimeout(autoSaveTimer);
  autoSaveTimer = setTimeout(async () => {
    try {
      const r = await api("/api/config/save", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path: cfgSelect.value, content: cfgJson.value }),
      });
      setMsg(t("cfg.saved", { name: cfgSelect.value, size: cfgJson.value.length }));
      cfgTextOnDisk = cfgJson.value;   // 这次写盘的内容 = 现在盘上那份
      notify(t("cfg.saved_toast", { name: cfgSelect.value }), true, "autosave"); // key=替换式，不刷屏
      // 后端把"HUD 起来会报错退出"的项作为 warning 回给这里（保存不拦，只提示）：
      // `hud_lang_path` 这类只有后端知道存在性的项，靠这条上屏。
      const warn = (r && Array.isArray(r.warnings)) ? r.warnings.join("；") : "";
      if (warn && warn !== lastSaveWarn) {
        notify(warn, false, "cfg-warn");
        lastSaveWarn = warn;
      } else if (!warn) {
        lastSaveWarn = "";
      }
    } catch (e) {
      report(t("cfg.save_failed", { err: e }), false, "autosave");
    }
  }, 500);
}

/** 盘上的配置被**外部**改过（HUD 预览拖完面板写回位置）→ 重新载入表单。
 *
 *  只在"本地没有未保存改动"（`autoSaveTimer` 为空）且内容确实不同时才动，
 *  免得把用户正在编辑的东西冲掉、也不做无谓的重渲染。
 *  触发点：窗口重新获得焦点 / 页面重新可见 / 可见时每 2 秒轻量轮询（本地 HTTP，很便宜）。 */
async function reloadIfChangedOnDisk() {
  if (!cfgActive || autoSaveTimer) return;
  if (document.visibilityState !== "visible") return;
  try {
    const d = await api(`/api/config/${encodeURIComponent(cfgActive)}`);
    if (d.content === cfgTextOnDisk) return;
    cfgTextOnDisk = d.content;
    cfgJson.value = d.content;
    renderForm(d.content);
    notify(t("cfg.reloaded", { name: cfgActive }), true, "external-change"); // key=替换式
    setMsg(t("cfg.reloaded", { name: cfgActive }));
  } catch (e) { /* 配置被删/被占用：忽略，下一次再试 */ }
}
/** 预览：**等自己启动的 wp8f 退出之后**再重读配置。
 *
 *  HUD 在预览里把拖拽结果写回配置文件的时机是"松手 / ESC / 关窗"，而 ESC 之后进程才退出；
 *  所以流程是"先确认它起来了 → 再等它消失 → 才读盘"，避免读到写回之前的旧内容。
 *  读盘用 `loadConfig`（同时刷新表单与 JSON 编辑框）。 */
let previewWatch = null;
function watchPreviewEnd() {
  if (previewWatch) return;
  let sawRunning = false;
  previewWatch = setInterval(async () => {
    let s = null;
    try { s = await api("/api/system"); } catch (e) { return; }  // 控制台正在关窗之类：下一次再试
    if (!sawRunning) {
      if (s && s.wp8f_running === true) sawRunning = true;       // 先等它真的起来
      return;
    }
    if (s && s.wp8f_running === false) {
      clearInterval(previewWatch);
      previewWatch = null;
      if (cfgActive) await loadConfig(cfgActive);
      if (cfgActive) notify(t("cfg.reloaded", { name: cfgActive }), true, "preview-end");
    }
  }, 1000);
}

window.addEventListener("focus", () => { reloadIfChangedOnDisk(); });
document.addEventListener("visibilitychange", () => { reloadIfChangedOnDisk(); });
setInterval(reloadIfChangedOnDisk, 2000);

// JSON 编辑：点击按钮才展开
const jsonToggle = $("#json-toggle");
if (jsonToggle) {
  jsonToggle.addEventListener("click", () => {
    const ta = $("#config-json");
    if (!ta) return;
    ta.hidden = !ta.hidden;
    if (!ta.hidden) ta.focus();
  });
}
// （保存已改为自动：任何配置修改防抖 500ms 即落盘，无手动保存按钮）

async function launchWp8f(drag) {
  const console = consoleBox.checked;
  try {
    if (!cfgActive) { report(t("cfg.no_config_loaded"), false); return; }
    const url = drag ? "/api/wp8f/drag-preview" : "/api/wp8f/launch";
    const r = await api(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ config: `config/${cfgActive}`, drag, console }),
    });
    const msg = t(console ? "cfg.launch_ok_console" : (drag ? "cfg.launch_ok_drag" : "cfg.launch_ok"));
    setMsg(msg);
    notify(msg);
    // 那条 "在 wp8f 窗口按 ESC 退出配置" 的提示**只属于预览模式**（`--drag` 下 HUD 里按 ESC
    // 退出配置并保存布局）；命令行调试也没有 ESC 这回事，上一次预览留下的提示同样要收掉。
    $("#toast").classList.toggle("hidden", !drag);
    if (drag) {
      // 预览：不关窗（用户要在 HUD 上拖面板、调完按 ESC 退出）
      // 起来之后盯住它：**等 wp8f.exe 真的结束**再重读配置文件 —— HUD 是在 ESC（或松手/
      // 关窗）那一刻把拖拽结果写回同一个 json 的，早读会读到旧内容（用户报的"切回来位置复原"）。
      watchPreviewEnd();
    } else if (trayAvailable === false) {
      // 托盘不可用 → 后端会拒绝关闭窗口，这里直接说明原因，不让用户以为"卡住了"
      notify(t("cfg.tray_unavailable"), false, "tray");
    } else {
      // 「开 始」→ 关闭窗口子进程：.NET/WebView2 资源全部归还系统，只留托盘常驻
      // （留 0.8s 让提示可见；关闭 ≠ 退出进程：点托盘图标会 kill wp8f 并重开窗口）
      setTimeout(() => {
        api("/api/gui/window", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ action: "close" }),
        }).catch(() => {
          // 窗口进程随后就被关闭，请求中断属正常现象，不提示
        });
      }, 800);
    }
  } catch (e) {
    report(t("cfg.launch_failed", { err: e }), false);
  }
}
// 「开始 / 预览」都直接启动；旁边的「命令行调试」勾选框决定用哪种方式：
// 勾上 = `CREATE_NEW_CONSOLE`，wp8f 有自己的命令行窗口（输出打在窗口里、不写日志、窗口不自动关）。
const CONSOLE_KEY = "wp8f-console-mode";
const consoleBox = $("#wp8f-console");
try { consoleBox.checked = localStorage.getItem(CONSOLE_KEY) === "1"; } catch (e) { /* 隐私模式忽略 */ }
consoleBox.addEventListener("change", () => {
  try { localStorage.setItem(CONSOLE_KEY, consoleBox.checked ? "1" : "0"); } catch (e) { /* 隐私模式忽略 */ }
});
$("#wp8f-launch").addEventListener("click", () => launchWp8f(false));
$("#wp8f-drag").addEventListener("click", () => launchWp8f(true));
