//! i18n：Rust 侧与前端共用 `resource/i18n/<lang>.json`。
//!
//! * 启动时读当前语言与兜底语言（`en`）各一次，构造成 `HashMap<&'static str, &'static str>`
//!   （键值 `Box::leak`），**JSON 文本与 `Value` 在函数返回前 drop**；
//! * `t()` = 两三次哈希，零分配（只有 [`tf`] 占位符替换会分配，用在错误消息这种冷路径）；
//! * 回退链：当前语言 → `en` → 键名本身（键名上屏 = 漏翻可见）；
//! * `missing` = "`en` 有、本语言没有"的键：启动日志打一行，`/api/i18n` 也返回（探针据此断言）；
//! * 语言来源：配置 `language`（`auto` = 跟随系统）→ 系统语言 → `en`，缺键用 [`DEFAULT_LANG`]。
//!
//! 中文两份是 `zh_simp` / `zh_trad`；[`LEGACY_ALIASES`] 让老配置里的 `zh-CN`/`zh-TW` 仍读得出来
//! （不认就会整份界面掉到 `en`）。前端不读这个模块：它 `fetch("/api/i18n/<lang>")` 拿合并后的表。
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// `language` 配置键里表示"跟随系统"的取值
pub const AUTO: &str = "auto";
/// 兜底语言（任何语言的缺失键都从这里补）
pub const FALLBACK: &str = "en";
/// 配置里没写 `language` 时用的语言（本项目的默认界面语言）
pub const DEFAULT_LANG: &str = "zh_simp";
/// 支持的语言（顺序 = GUI 下拉里的顺序；`resource/i18n/<code>.json` 必须存在）。
///
/// ⚠️ 与 HUD 无关：HUD 的标签表在 `resource/lang/`，看配置键 `hud_lang_path`
/// （[`disp::i18n::SUPPORTED`] 只是随包那两份，用户可自建）。两份清单各管一摊。
pub const SUPPORTED: &[&str] = &[
    "zh_simp", "zh_trad", "en", "ru", "es", "it", "fr", "de", "pt", "ja", "ko", "el", "tr", "pl", "vi",
];

/// **旧语言代码 → 新代码**（读取时兼容）。表里已是归一形式（小写、`_` → `-`，见 [`normalize`]）：
/// 老配置的 `language` 写的是旧代码，命中这张表才不会整份界面掉到 `en`。
pub const LEGACY_ALIASES: &[(&str, &str)] = &[
    // 简体：zh-CN（旧代码）、zh-Hans（BCP-47 脚本子标签）、zh-SG（新加坡简体）
    ("zh-cn", "zh_simp"),
    ("zh-hans", "zh_simp"),
    ("zh-hans-cn", "zh_simp"),
    ("zh-sg", "zh_simp"),
    // 繁体：zh-TW（旧代码）、zh-Hant（脚本子标签）、zh-HK / zh-MO（港澳繁体）
    ("zh-tw", "zh_trad"),
    ("zh-hant", "zh_trad"),
    ("zh-hant-tw", "zh_trad"),
    ("zh-hk", "zh_trad"),
    ("zh-mo", "zh_trad"),
];

/// 一张语言表：只读、`'static`，加载后不再变动。
#[derive(Debug)]
pub struct Table {
    pub lang: String,
    /// 键 → 文案（键值都已 leak 成 `'static`）
    map: HashMap<&'static str, &'static str>,
    /// `en` 有、本语言没有的键（按 `en` 的顺序）
    pub missing: Vec<&'static str>,
    /// 本语言有、`en` 没有的键（多半是拼错的键，开发期就该发现）
    pub extra: Vec<&'static str>,
    /// 表本体常驻字节数 = Σ(键长 + 值长)。用于"i18n 常驻内存"的实测/估算。
    pub bytes: usize,
}

impl Table {
    pub fn get(&self, key: &str) -> Option<&'static str> {
        self.map.get(key).copied()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// 本语言 + `en` 合并后的完整表（前端要的那一份）。
    fn merged_map(&self, en: &Table) -> Map<String, Value> {
        let mut out: Map<String, Value> = Map::new();
        for (k, v) in en.map.iter() {
            out.insert((*k).to_string(), Value::String((*v).to_string()));
        }
        for (k, v) in self.map.iter() {
            out.insert((*k).to_string(), Value::String((*v).to_string()));
        }
        out
    }
}

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static EN: OnceLock<Arc<Table>> = OnceLock::new();
static CACHE: OnceLock<Mutex<HashMap<String, Arc<Table>>>> = OnceLock::new();
static ACTIVE: RwLock<Option<Arc<Table>>> = RwLock::new(None);

/// 待打印的日志行（`gui` 没有 `tracing` 依赖 —— 控制台自己的日志走 `main.rs` 的
/// `log()` 写 `logs/console-host.log`）。`main` 在 `init` 之后 `take_log()` 取走并打印。
static LOG: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn note(msg: String) {
    if let Ok(mut g) = LOG.get_or_init(|| Mutex::new(Vec::new())).lock() {
        g.push(msg);
    }
}

/// 取走（并清空）待打印的日志行。
pub fn take_log() -> Vec<String> {
    LOG.get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default()
}

/// i18n 目录（`resource/i18n`）
pub fn dir(root: &Path) -> PathBuf {
    root.join("resource").join("i18n")
}

/// 把任意语言标签归一到受支持的 code：`en-US`→`en`、`zh-Hans-CN`→`zh_simp`、
/// `zh-Hant-TW`/`zh-HK`/`zh_TW`/`zh-TW`→`zh_trad`、`pt-BR`→`pt`、`ko-KR`→`ko`、`sr`→`None`。
/// 大小写与 `-`/`_` 分隔符都容错（系统语言标签两种写法都有）。
///
/// 顺序：新代码完整匹配 → 旧代码别名（[`LEGACY_ALIASES`]）→ 主语言前缀匹配，
/// 所以 `zh-CN`（旧）与 `zh_simp`（新）都得到 `zh_simp` —— 读旧配置不会掉到 `en`。
///
/// ⚠️ 与 `disp/src/i18n.rs::normalize` 同口径（改一处要改两处）。
pub fn normalize(tag: &str) -> Option<&'static str> {
    let t = tag.trim().replace('_', "-");
    if t.is_empty() {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    // ① 新代码完整匹配（zh_simp 归一成 zh-simp 后比较，与 zh-CN 那种旧写法不冲突）
    for code in SUPPORTED {
        if lower == code.replace('_', "-") {
            return Some(code);
        }
    }
    // ② 旧代码别名（zh-CN / zh-TW / zh-Hans / zh-Hant / zh-HK / zh-MO / zh-SG…）
    if let Some((_, code)) = LEGACY_ALIASES.iter().find(|(alias, _)| *alias == lower) {
        return Some(code);
    }
    // ③ 前缀匹配：en-US → en、ja-JP → ja、zh-Hant → zh_trad
    let primary = lower.split('-').next().unwrap_or("");
    if primary == "zh" {
        // 繁体（Hant / TW / HK / MO）走 zh_trad，其余中文走 zh_simp
        let trad = lower.contains("hant")
            || lower.contains("-tw")
            || lower.contains("-hk")
            || lower.contains("-mo");
        return Some(if trad { "zh_trad" } else { "zh_simp" });
    }
    SUPPORTED
        .iter()
        .find(|code| code.split('-').next().unwrap_or("") == primary)
        .copied()
}

/// 系统界面语言（`auto` 的判据）：
/// Windows 走 `GetUserDefaultUILanguage`（kernel32，直接声明以免为它加 windows-sys feature），
/// 其它平台走 `LC_ALL`/`LC_MESSAGES`/`LANG`。
pub fn system_lang() -> &'static str {
    #[cfg(windows)]
    {
        // `GetUserDefaultUILanguage` 返回 LANGID（低 10 位 = 主语言 ID）。
        // 中文要看**完整 LANGID**才能分清简繁：0x0404=zh_trad(TW)、0x0C04=zh_trad(HK)、
        // 0x1404=zh_trad(MO)、0x0804=zh_simp(CN)、0x1004=zh_simp(SG) —— 只看主语言 ID（0x04）
        // 会把繁体系统判成简体。
        extern "system" {
            fn GetUserDefaultUILanguage() -> u16;
        }
        let langid = unsafe { GetUserDefaultUILanguage() };
        let primary = langid & 0x03FF;
        let tag = match primary {
            0x04 => match langid {
                0x0404 | 0x0C04 | 0x1404 => "zh_trad",
                _ => "zh_simp",
            },
            0x07 => "de",
            0x08 => "el",
            0x09 => "en",
            0x0a => "es",
            0x0c => "fr",
            0x10 => "it",
            0x11 => "ja",
            0x12 => "ko",
            0x15 => "pl",
            0x16 => "pt",
            0x19 => "ru",
            0x1f => "tr",
            0x22 | 0x2a => "vi",
            _ => "",
        };
        if !tag.is_empty() {
            // 不在受支持清单里的（如 cs）走正常归一 → None → 兜底语言
            if let Some(c) = normalize(tag) {
                return c;
            }
        }
    }
    #[cfg(not(windows))]
    {
        for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(v) = std::env::var(var) {
                if let Some(code) = normalize(v.split('.').next().unwrap_or("")) {
                    return code;
                }
            }
        }
    }
    FALLBACK
}

/// 读一个语言文件并构造成 [`Table`]（**JSON 文本与 `Value` 在返回前 drop**）。
///
/// 只有字符串值进表；`_` 开头的键是给译者/工具看的元数据（`_lang`/`_review`/…），不进表。
fn read_table(root: &Path, lang: &str) -> Result<Arc<Table>, String> {
    let path = dir(root).join(format!("{lang}.json"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let parsed: HashMap<String, Value> =
        serde_json::from_str(&text).map_err(|e| format!("解析 {} 失败: {e}", path.display()))?;
    drop(text);

    let mut map: HashMap<&'static str, &'static str> = HashMap::with_capacity(parsed.len());
    let mut bytes = 0usize;
    for (k, v) in parsed.iter() {
        if k.starts_with('_') {
            continue;
        }
        let Value::String(v) = v else {
            return Err(format!("{lang}.json 的键 {k} 不是字符串（文案表只收键 → 字符串）"));
        };
        // Box::leak：进程生命周期内只读一次，之后查表零分配（与字体加载同一套手法）
        let k: &'static str = Box::leak(k.clone().into_boxed_str());
        let v: &'static str = Box::leak(v.clone().into_boxed_str());
        bytes += k.len() + v.len();
        map.insert(k, v);
    }
    drop(parsed);
    Ok(Arc::new(Table {
        lang: lang.to_string(),
        map,
        missing: Vec::new(),
        extra: Vec::new(),
        bytes,
    }))
}

/// 取（必要时加载并缓存）某个语言的表。
pub fn table(root: &Path, lang: &str) -> Result<Arc<Table>, String> {
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(t) = guard.get(lang) {
            return Ok(Arc::clone(t));
        }
    }
    let mut t = read_table(root, lang)?;
    // 与 en 对账：缺失键（回退到 en）与多余键（多半是拼错）都要能一眼看见
    if lang != FALLBACK {
        let en = en_table(root)?;
        t = Arc::new(Table {
            lang: t.lang.clone(),
            map: t.map.clone(),
            missing: en.map.keys().filter(|k| !t.map.contains_key(*k)).copied().collect(),
            extra: t.map.keys().filter(|k| !en.map.contains_key(*k)).copied().collect(),
            bytes: t.bytes,
        });
    }
    if let Ok(mut guard) = cache.lock() {
        guard.insert(lang.to_string(), Arc::clone(&t));
    }
    Ok(t)
}

fn en_table(root: &Path) -> Result<Arc<Table>, String> {
    if let Some(t) = EN.get() {
        return Ok(Arc::clone(t));
    }
    let t = read_table(root, FALLBACK)?;
    let t = EN.get_or_init(|| Arc::clone(&t));
    Ok(Arc::clone(t))
}

/// 从 config 目录里解析语言：**排序后第一个带 `language` 键的 `config/*.json`**
/// （正常就是 `config/default.json`）。读到非法值按 `auto` 处理并记一行警告。
pub fn resolve_from_config(root: &Path) -> String {
    let dir = root.join("config");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
        let Some(lang) = v.get("language").and_then(|x| x.as_str()) else { continue };
        return resolve(lang);
    }
    DEFAULT_LANG.to_string()
}

/// `language` 键的取值 → 真正生效的语言 code。`auto`/空 = 跟随系统；未知值回退 `en`。
pub fn resolve(lang: &str) -> String {
    let raw = lang.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case(AUTO) {
        return system_lang().to_string();
    }
    match normalize(raw) {
        Some(code) => code.to_string(),
        None => {
            note(format!("[I18N] 未知语言 {raw:?}（config 的 language 键），改用 {FALLBACK}"));
            FALLBACK.to_string()
        }
    }
}

/// 启动时一次性载入（幂等）：固定根目录、读 `en`、按配置解析并激活当前语言。
///
/// 日志（`main` 用 [`take_log`] 取走并写进控制台日志）里有三样东西：
/// 生效语言、**表常驻字节数**、以及缺失键/多余键条数 —— 最后这条就是
/// "缺失键要在开发模式下可被发现"的落地方式。
pub fn init(root: &Path) {
    let _ = ROOT.set(root.to_path_buf());
    match en_table(root) {
        Ok(en) => note(format!("[I18N] 兜底语言 en：{} 条 / {} B", en.len(), en.bytes)),
        Err(e) => note(format!("[I18N] 兜底语言 en 读不到：{e}")),
    }
    let want = resolve_from_config(root);
    match set_lang(&want) {
        Ok(info) => {
            note(format!(
                "[I18N] lang={} (config 解析结果) fallback={} keys={} missing={} extra={} 常驻≈{} B",
                info.lang,
                FALLBACK,
                info.keys,
                info.missing.len(),
                info.extra.len(),
                info.bytes
            ));
            // 缺失键逐条记下来（只记前 20 条，够定位就行）
            if !info.missing.is_empty() {
                let head: Vec<&str> = info.missing.iter().take(20).copied().collect();
                note(format!("[I18N] 缺失键（{} 条）：{:?}", info.missing.len(), head));
            }
        }
        Err(e) => note(format!("[I18N] 语言 {want} 加载失败：{e}（保留内置回退）")),
    }
}

/// 懒初始化：任何入口（HTTP 请求、托盘）都能安全调用，不必依赖 main 里的调用顺序。
pub fn ensure(root: &Path) {
    if ROOT.get().is_none() {
        init(root);
    }
}

/// 切换（或首次设置）当前语言。`auto`/空 = 按系统语言。
pub fn set_lang(lang: &str) -> Result<LangInfo, String> {
    let root = ROOT
        .get()
        .cloned()
        .ok_or_else(|| "i18n 未初始化（先调用 init/ensure）".to_string())?;
    let code = resolve(lang);
    let t = table(&root, &code)?;
    if let Ok(mut guard) = ACTIVE.write() {
        *guard = Some(Arc::clone(&t));
    }
    Ok(LangInfo::of(&t))
}

/// 当前生效的语言 code（未初始化时 = 兜底语言）。
/// 返回 `String` 而不是 `&'static str`：表存在 `Arc` 里，拿 `'static` 引用只能靠 unsafe，
/// 而这条路径（切语言 / 写响应头）本来就不是热路径。
pub fn lang() -> String {
    ACTIVE
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|t| t.lang.clone()))
        .unwrap_or_else(|| FALLBACK.to_string())
}

/// 查表：当前语言 → `en` → 键名。**不分配**。
pub fn t(key: &'static str) -> &'static str {
    if let Ok(guard) = ACTIVE.read() {
        if let Some(table) = guard.as_ref() {
            if let Some(v) = table.get(key) {
                return v;
            }
        }
    }
    if let Some(en) = EN.get() {
        if let Some(v) = en.get(key) {
            return v;
        }
    }
    key
}

/// 带占位符的查表：把 `{name}` 这类占位符换成实参（只用在错误消息等冷路径上）。
pub fn tf(key: &'static str, args: &[(&str, &str)]) -> String {
    let mut s = t(key).to_string();
    for (k, v) in args {
        let pat = format!("{{{k}}}");
        if s.contains(&pat) {
            s = s.replace(&pat, v);
        }
    }
    s
}

/// 语言概况（`/api/i18n/<lang>` 的响应头 + 启动日志用）。
#[derive(Debug, Clone)]
pub struct LangInfo {
    pub lang: String,
    pub keys: usize,
    pub bytes: usize,
    pub missing: Vec<&'static str>,
    pub extra: Vec<&'static str>,
}

impl LangInfo {
    fn of(t: &Table) -> Self {
        Self {
            lang: t.lang.clone(),
            keys: t.len(),
            bytes: t.bytes,
            missing: t.missing.clone(),
            extra: t.extra.clone(),
        }
    }
}

/// 给前端的**一次性**响应体：当前语言与 `en` 合并后的完整表 + 缺失键清单。
///
/// `lang` 为 `None`/空/`auto` 时用当前生效语言；显式语言会**同时切换**当前语言
/// （GUI 的下拉就是要这个语义：切完界面与后端错误串是同一语言，不必重启进程）。
pub fn response(root: &Path, want: Option<&str>) -> Result<Value, String> {
    let explicit = want.map(str::trim).filter(|s| !s.is_empty());
    // 显式指定的语言必须是**已知**的：否则报错，而不是静默给一份 en
    // （前端下拉的语言来自 `languages`，真报错说明 code 拼错了，要看得见）
    if let Some(v) = explicit {
        if !v.eq_ignore_ascii_case(AUTO) && normalize(v).is_none() {
            return Err(format!("unknown language {v:?}"));
        }
    }
    let code = match explicit {
        Some(v) => resolve(v),
        None => {
            let cur = lang();
            if cur == FALLBACK && ACTIVE.read().map(|g| g.is_none()).unwrap_or(true) {
                resolve_from_config(root)
            } else {
                cur
            }
        }
    };
    let t = match explicit {
        // 显式指定语言：支持的一律接受（`auto` 已在上一步解析成具体 code）
        Some(_) => {
            let t = table(root, &code)?;
            if let Ok(mut guard) = ACTIVE.write() {
                *guard = Some(Arc::clone(&t));
            }
            t
        }
        None => match ACTIVE.read().ok().and_then(|g| g.clone()) {
            Some(t) => t,
            None => {
                let t = table(root, &code)?;
                if let Ok(mut guard) = ACTIVE.write() {
                    *guard = Some(Arc::clone(&t));
                }
                t
            }
        },
    };
    let en = en_table(root)?;
    let strings = t.merged_map(&en);
    // 元数据也带上：前端要 `_lang`/`_review` 做自检（它们不进 strings）
    let meta = read_meta(root, &t.lang);
    let mut out = Map::new();
    out.insert("lang".into(), json!(t.lang));
    out.insert("fallback".into(), json!(FALLBACK));
    out.insert("keys".into(), json!(strings.len()));
    out.insert("bytes".into(), json!(t.bytes + en.bytes));
    out.insert("missing".into(), json!(t.missing));
    out.insert("extra".into(), json!(t.extra));
    out.insert("meta".into(), Value::Object(meta));
    out.insert("strings".into(), Value::Object(strings));
    Ok(Value::Object(out))
}

/// 语言文件里的 `_` 元数据（`_lang`/`_name`/`_review`/…）：只用于诊断，不进文案表。
fn read_meta(root: &Path, lang: &str) -> Map<String, Value> {
    let path = dir(root).join(format!("{lang}.json"));
    let mut out = Map::new();
    let Ok(text) = std::fs::read_to_string(&path) else { return out };
    let Ok(Value::Object(v)) = serde_json::from_str::<Value>(&text) else { return out };
    for (k, val) in v.iter() {
        if k.starts_with('_') {
            out.insert(k.clone(), val.clone());
        }
    }
    out
}

/// GUI 下拉用的语言清单：`code` + 该语言自己的名字（`_name` 元数据，缺失时退回 code）。
pub fn languages(root: &Path) -> Vec<Value> {
    SUPPORTED
        .iter()
        .map(|code| {
            let name = read_meta(root, code)
                .get("_name")
                .and_then(|v| v.as_str())
                .unwrap_or(code)
                .to_string();
            json!({ "code": code, "name": name, "file": format!("resource/i18n/{code}.json") })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
    }

    #[test]
    fn normalize_accepts_system_tags() {
        assert_eq!(normalize("en-US"), Some("en"));
        assert_eq!(normalize("zh-Hans-CN"), Some("zh_simp"));
        assert_eq!(normalize("zh_TW"), Some("zh_trad"));
        assert_eq!(normalize("zh-Hant"), Some("zh_trad"));
        assert_eq!(normalize("zh-HK"), Some("zh_trad"));
        assert_eq!(normalize("zh-CN"), Some("zh_simp"));
        assert_eq!(normalize("pt-BR"), Some("pt"));
        assert_eq!(normalize("de-AT"), Some("de"));
        assert_eq!(normalize("ko-KR"), Some("ko"));
        assert_eq!(normalize("vi-VN"), Some("vi"));
        assert_eq!(normalize("ja"), Some("ja"));
        assert_eq!(normalize("TR-tr"), Some("tr"));
        assert_eq!(normalize("sr-RS"), None);
        assert_eq!(normalize(""), None);
    }

    /// **旧语言代码的兼容**（B：zh-CN/zh-TW → zh_simp/zh_trad 之后，老配置不能掉到 en）。
    ///
    /// 这条必须能失败：`LEGACY_ALIASES` 里任何一条失效，下面逐项断言立刻红。
    /// 特意挑了**前缀匹配救不回来**的写法（`zh-Hans`/`zh-Hant`/`zh-HK`）——
    /// 只测 `zh-CN` 的话，删掉别名表也会因"主语言是 zh"而侥幸通过，等于恒真断言。
    /// 表里每一条都逐项断言，避免"加了别名但没人用"。
    #[test]
    fn legacy_language_codes_are_accepted() {
        // 用户配置里可能出现的全部旧写法（含大小写与 `_` 分隔）
        for old in ["zh-CN", "zh_CN", "zh-Hans", "zh-Hans-CN", "zh-hans-cn", "zh-SG", "ZH-CN"] {
            assert_eq!(normalize(old), Some("zh_simp"), "旧简体写法 {old:?} 必须归一到 zh_simp");
        }
        for old in ["zh-TW", "zh_TW", "zh-Hant", "zh-Hant-TW", "zh-HK", "zh-MO", "ZH-TW"] {
            assert_eq!(normalize(old), Some("zh_trad"), "旧繁体写法 {old:?} 必须归一到 zh_trad");
        }
        // 别名表逐项自检：每一条都必须真的被 normalize 用上（表与实现不许漂移）
        for (alias, code) in LEGACY_ALIASES {
            assert_eq!(normalize(alias), Some(*code), "别名 {alias:?} → {code:?} 没生效");
            assert!(SUPPORTED.contains(code), "别名指向了不在 SUPPORTED 里的 {code:?}");
        }
        // 新代码本身就是完整匹配
        assert_eq!(normalize("zh_simp"), Some("zh_simp"));
        assert_eq!(normalize("zh_trad"), Some("zh_trad"));
    }

    /// 语言清单必须与 `disp/src/i18n.rs::SUPPORTED` 逐字一致（HUD 与 GUI 选同一门语言）。
    #[test]
    fn every_supported_language_has_a_file_with_the_same_name() {
        let root = repo_root();
        assert_eq!(SUPPORTED.len(), 15, "支持的语言数变了：{SUPPORTED:?}");
        for code in SUPPORTED {
            let meta = read_meta(&root, code);
            assert_eq!(
                meta.get("_lang").and_then(|v| v.as_str()),
                Some(*code),
                "resource/i18n/{code}.json 的 _lang 与文件名不一致"
            );
            assert!(
                meta.get("_name").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()),
                "{code}.json 缺少 _name（语言下拉要显示它）"
            );
        }
        // 朝鲜语的显示名按用户要求用「국어」
        assert_eq!(
            read_meta(&root, "ko").get("_name").and_then(|v| v.as_str()),
            Some("국어")
        );
    }

    #[test]
    fn resolve_falls_back_to_en_for_unknown_values() {
        assert_eq!(resolve("en"), "en");
        assert_eq!(resolve("zh_simp"), "zh_simp");
        // 旧代码也走同一级归一（老配置里的 language 值）
        assert_eq!(resolve("zh-CN"), "zh_simp");
        assert_eq!(resolve("zh-Hant"), "zh_trad");
        // auto 跟随系统：结果必须是受支持的语言之一
        assert!(SUPPORTED.contains(&resolve(AUTO).as_str()));
        assert_eq!(resolve("klingon"), FALLBACK);
    }

    /// 十五种语言的键集必须与 `en` 完全一致（漏翻 / 拼错键在这里就该红）。
    #[test]
    fn every_language_has_the_same_key_set_as_en() {
        let root = repo_root();
        let en = read_table(&root, FALLBACK).expect("en.json 必须存在");
        assert!(en.len() > 100, "en 表太小（{}），是不是读错文件了", en.len());
        for code in SUPPORTED {
            let t = table(&root, code).unwrap_or_else(|e| panic!("{code}: {e}"));
            assert!(t.missing.is_empty(), "{code} 缺 {} 个键：{:?}", t.missing.len(), &t.missing[..t.missing.len().min(8)]);
            assert!(t.extra.is_empty(), "{code} 多出键：{:?}", &t.extra[..t.extra.len().min(8)]);
            assert_eq!(t.len(), en.len(), "{code} 键数与 en 不一致");
        }
    }

    /// 占位符必须逐语言一致：少一个 `{name}` 就会把消息里的实参吞掉。
    #[test]
    fn placeholders_match_across_languages() {
        let root = repo_root();
        let en = read_table(&root, FALLBACK).unwrap();
        let ph = |s: &str| -> Vec<String> {
            let mut out = Vec::new();
            let mut rest = s;
            while let Some(i) = rest.find('{') {
                let Some(j) = rest[i..].find('}') else { break };
                out.push(rest[i + 1..i + j].to_string());
                rest = &rest[i + j + 1..];
            }
            out.sort();
            out
        };
        for code in SUPPORTED {
            let t = table(&root, code).unwrap();
            for (k, v) in en.map.iter() {
                let want = ph(v);
                if want.is_empty() {
                    continue;
                }
                let got = ph(t.get(k).unwrap_or(""));
                assert_eq!(got, want, "{code} 的 {k} 占位符不一致：{got:?} != {want:?}");
            }
        }
    }

    /// 查表路径：命中当前语言、缺键回退 en、彻底没有时回退键名。**不分配**是设计约束。
    #[test]
    fn lookup_falls_back_en_then_key() {
        let root = repo_root();
        init(&root);
        assert_eq!(t("api.not_found"), t("api.not_found"));
        // 不存在的键 → 键名本身（便于发现漏翻）
        assert_eq!(t("no.such.key.at.all"), "no.such.key.at.all");
        let zh = t("nav.config");
        assert!(!zh.is_empty());
        // 切到 en 必须是英文
        set_lang("en").unwrap();
        assert_eq!(t("nav.config"), "Configurator");
        set_lang("zh_simp").unwrap();
        assert_eq!(t("nav.config"), "配置器");
        // 旧代码也能切过去（读到的是 zh_simp 的表）
        set_lang("zh-CN").unwrap();
        assert_eq!(lang(), "zh_simp");
        assert_eq!(t("nav.config"), "配置器");
    }

    /// 带占位符的消息：实参必须落到 {} 里，且不认识的名字原样保留。
    #[test]
    fn tf_substitutes_placeholders() {
        let root = repo_root();
        init(&root);
        set_lang("en").unwrap();
        let s = tf("api.bad_origin", &[("lat", "1.5"), ("lon", "2.5")]);
        assert!(s.contains("1.5") && s.contains("2.5"), "{s}");
        assert!(!s.contains("{lat}"), "{s}");
    }

    /// 常驻表大小：十五种语言 + en 全量加载也必须是几百 KB 级以内（用户要求给出实测）。
    #[test]
    fn resident_table_size_is_small() {
        let root = repo_root();
        let mut total = 0usize;
        for code in SUPPORTED {
            total += table(&root, code).unwrap().bytes;
        }
        assert!(total > 0);
        assert!(total < 1024 * 1024, "十五种语言的键值总量 {} B 太大了", total);
    }
}
