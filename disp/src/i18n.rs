//! HUD 侧语言表：`resource/lang/<name>.json`（与 GUI 的 `resource/i18n/` 是两套文件，
//! 互不影响 —— GUI 那 15 种是控制台界面语言，见 `gui/src/i18n.rs`）。
//!
//! # 语言从哪来（只有一个键）
//!
//! `hud_lang_path` = **相对安装根的路径**（绝对路径也接受），例如 `resource/lang/zh.json`；
//! 随包只有 `zh.json` / `en.json`，用户可自建任意 `xxx.json` 指过来。
//! **没写就用随包的中文表** —— HUD 不看 GUI 的 `language`，也不认旧语言代码。
//!
//! # 什么时候读
//!
//! `core/src/main.rs` 的启动序列里调一次 [`init`]（配置刚读完、字体/语音/窗口/渲染线程都还没起来）。
//!
//! # 读入与释放
//!
//! [`init`] 把文件读成 `String`、解析成 `Value`，逐条 `Box::leak` 成
//! `HashMap<&'static str, &'static str>`；**文本与中间对象在函数返回前 drop**，
//! `_` 开头的元数据（`_lang`/`_name`/`_about`）**不进表**。常驻的只有这张表
//! （`Σ(键字节 + 值字节)` = [`Table::bytes`]），实测见 `disp/tests/hud_lang_release.rs`。
//!
//! # 查表零分配
//!
//! [`t`] = 一次 `RwLock` 读锁 + 一次哈希，返回 `&'static str`（`DrawItem::label` 的
//! `Cow::Borrowed`），所以每帧画 HUD 一个 `String` 都不新建。回退链：当前表 →
//! 编译期内嵌的 `en`（`include_str!`）→ 键名本身（键名上屏 = 漏翻一眼可见）。
//!
//! # 读不到 / 解析失败：明确报错，不静默回退
//!
//! [`init`] 返回 `Err`（一条讲清问题的消息），调用方打印后 `exit(2)`；
//! 只是少了某几个键时回退内嵌 `en` 并记一条 warn。

use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// 兜底语言（本表缺键时从它补；也是编译期内嵌的那一份）
pub const FALLBACK: &str = "en";
/// `hud_lang_path` 没写时 HUD 用的语言（随包的中文表）
pub const DEFAULT: &str = "zh";
/// HUD **随包分发**的语言表（`resource/lang/<code>.json` 必须存在）。
/// 想用别的语言：自己写 `resource/lang/xxx.json`（键集与这两份一致），把 `hud_lang_path` 指过去。
/// `scripts/tests/i18n_keys.py` 会按这份清单核对磁盘文件。
pub const SUPPORTED: &[&str] = &["zh", "en"];

/// HUD 标签表目录（⚠️ 与 GUI 的 `resource/i18n/` 是两个目录）。
pub const LANG_DIR: &str = "resource/lang";

/// 编译期内嵌的兜底表（`resource/lang/en.json` 的同一份文件，不会漂移）：
/// 磁盘上读不到语言文件、或自建文件少了某几个键时，HUD 显示英文而不是键名。
const FALLBACK_JSON: &str = include_str!("../../resource/lang/en.json");

/// 一张语言表：只读、加载后不再变动。
#[derive(Debug)]
pub struct Table {
    pub lang: String,
    /// 键 → 文案（键值都已 leak 成 `'static`）
    map: HashMap<&'static str, &'static str>,
    /// 表本体常驻字节数 = Σ(键长 + 值长)
    pub bytes: usize,
}

impl Table {
    pub fn get(&self, key: &str) -> Option<&'static str> {
        self.map.get(key).copied()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &&'static str> {
        self.map.keys()
    }
}

/// 语言概况（启动日志 / 测试断言用）。
#[derive(Debug, Clone)]
pub struct LangInfo {
    /// 生效的 HUD 语言 code：内置表是 `zh`/`en`；`hud_lang_path` 指向自建文件时是文件名主干。
    pub lang: String,
    /// 实际读的那个文件（相对安装根的写法，如 `resource/lang/zh.json`）
    pub file: String,
    /// 语言是**从哪来的**：`hud_lang_path` / `default`（测试里还有 `set_lang`）
    pub source: String,
    pub keys: usize,
    /// 表的**常驻**字节数（`Σ(键字节 + 值字节)`）—— 读完之后留在内存里的就是它
    pub bytes: usize,
    /// 读入的 **JSON 文本字节数**（读完即释放）：日志里"读进来多少 → 留下多少"的分子
    pub json_bytes: usize,
    /// 内嵌 `en` 有、本表没有的键（回退到 `en` 的那些，启动日志会列前 20 条）
    pub missing: Vec<&'static str>,
    /// 本表有、内嵌 `en` 没有的键（多半是拼错的键）
    pub extra: Vec<&'static str>,
}

static EMBEDDED_EN: OnceLock<Table> = OnceLock::new();
static CACHE: OnceLock<Mutex<HashMap<String, Arc<Table>>>> = OnceLock::new();
static ACTIVE: RwLock<Option<Arc<Table>>> = RwLock::new(None);
static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// **测试专用**的全局锁：`set_lang` 改的是进程级状态，而 cargo 的测试线程并行跑 ——
/// 读当前语言的用例必须和改语言的用例共用这把锁，否则两次渲染之间被切一次语言就会变成
/// 没法复现的假失败。
#[cfg(test)]
pub static TEST_LOCK: Mutex<()> = Mutex::new(());

/// 把 `<lang>.json` 解析成一张表；JSON 文本由调用方持有，`Value` 在本函数返回前 drop。
fn parse(lang: &str, text: &str) -> Result<Table, String> {
    let parsed: HashMap<String, Value> =
        serde_json::from_str(text).map_err(|e| format!("{lang}.json 解析失败: {e}"))?;
    let mut map: HashMap<&'static str, &'static str> = HashMap::with_capacity(parsed.len());
    let mut bytes = 0usize;
    for (k, v) in parsed.iter() {
        if k.starts_with('_') {
            continue; // `_lang`/`_name`/`_about` 是元数据，不进表
        }
        let Value::String(v) = v else {
            return Err(format!("{lang}.json 的键 {k} 不是字符串（语言表只收键 → 字符串）"));
        };
        // Box::leak：只读一次、进程内常驻，之后查表零分配
        let k: &'static str = Box::leak(k.clone().into_boxed_str());
        let v: &'static str = Box::leak(v.clone().into_boxed_str());
        bytes += k.len() + v.len();
        map.insert(k, v);
    }
    Ok(Table { lang: lang.to_string(), map, bytes })
}

/// 内嵌的兜底（`en`）表：任何情况下都拿得到。
pub fn embedded_en() -> &'static Table {
    EMBEDDED_EN.get_or_init(|| {
        parse(FALLBACK, FALLBACK_JSON).expect("内嵌的 resource/lang/en.json 必须是合法语言表")
    })
}

/// 候选语言目录（按顺序试）：基准 + `resource/lang`（基准 = [`crate::paths::bases`]）。
fn candidates() -> Vec<PathBuf> {
    crate::paths::bases().into_iter().map(|b| b.join(LANG_DIR)).collect()
}

/// 定位 `resource/lang`（第一次调用生效）。找不到返回 `None`（调用方写进错误消息，不 panic）。
pub fn dir() -> Option<&'static Path> {
    DIR.get_or_init(|| candidates().into_iter().find(|p| p.is_dir()))
        .as_deref()
}

/// `resource/lang/` 下所有 `<name>.json` 的名字（排序）。测试用它逐语言量标签列宽：
/// 自建的语言文件也必须塞得进定宽标签列。
pub fn lang_files() -> Vec<String> {
    let Some(dir) = dir() else { return Vec::new() };
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// 把配置里写的路径（`hud_lang_path` 的值）解析成一个**真实存在**的文件路径。
///
/// 规则与 `font_path` 同口径（同一份基准候选：[`crate::paths::bases`]）：值原样（绝对路径 /
/// 相对 cwd）→ `<基准>/<值>`。都不存在 → `Err`，消息里列出**找过哪些位置**。
fn resolve_path(value: &str) -> Result<PathBuf, String> {
    let raw = Path::new(value);
    if raw.is_absolute() {
        return if raw.is_file() {
            Ok(raw.to_path_buf())
        } else {
            Err(format!("{} 不是一个文件", raw.display()))
        };
    }
    let tried: Vec<PathBuf> = crate::paths::bases().iter().map(|b| b.join(raw)).collect();
    tried
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .ok_or_else(|| {
            let mut s = format!("找不到语言文件 {value:?}；找过这些位置：");
            for p in &tried {
                s.push_str(&format!("\n    - {}", p.display()));
            }
            s
        })
}

/// 选语言文件：**只看 `hud_lang_path`**（HUD 不认 GUI 的 `language`，也不认旧语言代码）。
///
/// 返回 `(要读的文件, 语言 code, 来源标签)`；非空时直接把值当路径（code 取文件名主干，仅用于日志），
/// 没写时用随包的中文表（[`DEFAULT`]）。
pub fn pick(hud_lang_path: &str) -> (String, String, String) {
    let custom = hud_lang_path.trim();
    if !custom.is_empty() {
        let stem = Path::new(custom)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| custom.to_string());
        return (custom.to_string(), stem, "hud_lang_path".to_string());
    }
    (format!("{LANG_DIR}/{DEFAULT}.json"), DEFAULT.to_string(), "default".to_string())
}

/// 读某个语言（内置 code 或 `hud_lang_path` 那种路径）的表（带缓存）。
///
/// 文件不存在 / 解析失败 → `None`（[`init`] 会把它变成**带回溯信息的错误**）。
pub fn table(lang: &str) -> Option<Arc<Table>> {
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(t) = guard.get(lang) {
            return Some(Arc::clone(t));
        }
    }
    let path = if lang.contains('/') || lang.contains('\\') {
        resolve_path(lang).ok()?
    } else {
        dir()?.join(format!("{lang}.json"))
    };
    let text = std::fs::read_to_string(&path).ok()?;
    let t = match parse(lang, &text) {
        Ok(t) => Arc::new(t),
        Err(e) => {
            tracing::warn!("[DISP] i18n: {e}");
            return None;
        }
    };
    drop(text);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(lang.to_string(), Arc::clone(&t));
    }
    Some(t)
}

/// 直接读一个**已知路径**的语言文件（`hud_lang_path` 那条路）：错误信息比 [`table`] 详细得多。
///
/// 返回 `(表, 读入的 JSON 文本字节数)` —— 文本**在本函数返回前就释放**，只留只读表。
fn table_at(path: &Path, lang: &str) -> Result<(Arc<Table>, usize), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    let t = parse(lang, &text)?;
    let json_bytes = text.len();
    drop(text); // 用户口径：读完就把 JSON 文本放掉，只留只读表
    Ok((Arc::new(t), json_bytes))
}

/// 按选出来的目标读语言文件（纯函数：不碰进程级状态，便于单测）。
/// 返回 `(表, 实际读的路径, 读入的 JSON 字节数)`；读不到 / 解析失败 → 一条讲清问题的 `Err`。
fn load(want: &str, code: &str, source: &str) -> Result<(Arc<Table>, PathBuf, usize), String> {
    let path = if want.contains('/') || want.contains('\\') {
        resolve_path(want).map_err(|e| load_failed(want, code, source, &e))?
    } else {
        let Some(dir) = dir() else {
            return Err(load_failed(
                want,
                code,
                source,
                &format!("连目录 {LANG_DIR}/ 都没找到（打包不全？从别处启动的？）"),
            ));
        };
        let p = dir.join(format!("{code}.json"));
        if !p.is_file() {
            return Err(load_failed(want, code, source, &format!("{} 不存在", p.display())));
        }
        p
    };
    let (t, json_bytes) = table_at(&path, code).map_err(|e| load_failed(want, code, source, &e))?;
    Ok((t, path, json_bytes))
}

/// 启动时载入一次（`core/src/main.rs` 的初始化调）：幂等，第二次调用直接返回当前概况。
/// 参数 = 配置的 `hud_lang_path`（路径，写哪儿读哪儿；空的用随包中文表）。
pub fn init(hud_lang_path: &str) -> Result<LangInfo, String> {
    if let Some(t) = ACTIVE.read().ok().and_then(|g| g.clone()) {
        return Ok(LangInfo {
            lang: t.lang.clone(),
            file: String::new(),
            source: "already-initialized".to_string(),
            keys: t.len(),
            bytes: t.bytes,
            json_bytes: 0,
            missing: Vec::new(),
            extra: Vec::new(),
        });
    }
    let (want, code, source) = pick(hud_lang_path);
    let (t, _path, json_bytes) = load(&want, &code, &source)?;
    Ok(finish(t, &want, &source, json_bytes))
}

/// 装载成功后的收尾：与内嵌 `en` 对账（缺键/多键）、写进 `ACTIVE`、打日志。
fn finish(t: Arc<Table>, file: &str, source: &str, json_bytes: usize) -> LangInfo {
    let en = embedded_en();
    let missing: Vec<&'static str> = en.keys().filter(|k| !t.map.contains_key(*k)).copied().collect();
    let extra: Vec<&'static str> = t.keys().filter(|k| !en.map.contains_key(*k)).copied().collect();
    let info = LangInfo {
        lang: t.lang.clone(),
        file: file.to_string(),
        source: source.to_string(),
        keys: t.len(),
        bytes: t.bytes,
        json_bytes,
        missing,
        extra,
    };
    if let Ok(mut guard) = ACTIVE.write() {
        *guard = Some(t);
    }
    tracing::info!(
        "[DISP] i18n: {}（来源 {}）lang={} keys={} missing={} extra={} 读入 JSON {} B → 常驻 {} B\
         （JSON 文本与中间对象已释放；内嵌 {} {} B）",
        file,
        source,
        info.lang,
        info.keys,
        info.missing.len(),
        info.extra.len(),
        info.json_bytes,
        info.bytes,
        FALLBACK,
        en.bytes
    );
    if !info.missing.is_empty() {
        let head: Vec<&str> = info.missing.iter().take(20).copied().collect();
        tracing::warn!(
            "[DISP] i18n: {} 缺 {} 个键（这些标签回退内嵌 en）：{:?}",
            info.lang,
            info.missing.len(),
            head
        );
    }
    if !info.extra.is_empty() {
        let head: Vec<&str> = info.extra.iter().take(20).copied().collect();
        tracing::warn!("[DISP] i18n: {} 多出 {} 个键（多半是拼错）：{:?}", info.lang, info.extra.len(), head);
    }
    info
}

/// 「读不到 / 解析失败」的错误：一条讲清问题 + 配置键 + 退出码，不写"回退到 en"。
fn load_failed(want: &str, code: &str, source: &str, why: &str) -> String {
    format!(
        "[DISP] 读不到 HUD 语言文件 {want}（来源 {source}，语言 {code}）：{why}。\
         配置键 hud_lang_path，HUD 退出（码 2）"
    )
}

/// 切换当前语言（**测试用**：正常路径只有 [`init`] 调一次）。
/// `lang` 可以是内置 code（`zh`/`en`），也可以是路径；每次都真读文件（不受幂等保护）。
pub fn set_lang(lang: &str) -> Result<LangInfo, String> {
    let (want, code, source) = if lang.contains('/') || lang.contains('\\') || lang.ends_with(".json")
    {
        let stem = Path::new(lang)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| lang.to_string());
        (lang.to_string(), stem, "set_lang".to_string())
    } else {
        (format!("{LANG_DIR}/{lang}.json"), lang.to_string(), "set_lang".to_string())
    };
    let (t, _path, json_bytes) = load(&want, &code, &source)?;
    Ok(finish(t, &want, &source, json_bytes))
}

/// 当前生效的语言 code（未初始化时 = 兜底语言）。
pub fn lang() -> String {
    ACTIVE
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|t| t.lang.clone()))
        .unwrap_or_else(|| FALLBACK.to_string())
}

/// 查表：当前表 → 内嵌 `en` → 键名。**不分配**（`RwLock` 读锁 + 一次哈希）。
pub fn t(key: &'static str) -> &'static str {
    if let Ok(guard) = ACTIVE.read() {
        if let Some(table) = guard.as_ref() {
            if let Some(v) = table.get(key) {
                return v;
            }
        }
    }
    if let Some(v) = embedded_en().get(key) {
        return v;
    }
    key
}

/// 带占位符的查表（`{name}` 这类）：只用在日志/错误消息这种冷路径上，可以分配。
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

/// 把当前生效的表换掉但不打日志（**只在测试里用**，`set_lang` 已经够了）。
#[cfg(test)]
pub fn active_bytes() -> usize {
    ACTIVE
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|t| t.bytes))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 语言目录候选必须**就是**共用基准各加一段 `resource/lang`：谁再手抄一份自己的顺序，
    /// 就会和 `font_path` 的解析挑到不同目录（"字体能加载、语言表不能"）。
    #[test]
    fn lang_dir_candidates_follow_the_shared_bases() {
        let want: Vec<PathBuf> =
            crate::paths::bases().into_iter().map(|b| b.join(LANG_DIR)).collect();
        assert_eq!(candidates(), want);
    }

    /// 内置的两份表都在，且**键集与内嵌 en 完全一致**（漏翻/拼错键在这里就该红）。
    #[test]
    fn builtin_tables_have_the_same_key_set_as_embedded_en() {
        assert!(dir().is_some(), "找不到 {LANG_DIR}/（cargo test 的 cwd 是 disp/）");
        let en = embedded_en();
        assert!(en.len() > 20, "内嵌 en 表太小（{}）—— 读错文件了？", en.len());
        let mut bad = Vec::new();
        for code in SUPPORTED {
            let Some(t) = table(code) else {
                bad.push(format!("{code}: 读不到 {LANG_DIR}/{code}.json"));
                continue;
            };
            let missing: Vec<&str> = en.keys().filter(|k| !t.map.contains_key(*k)).copied().collect();
            let extra: Vec<&str> = t.keys().filter(|k| !en.map.contains_key(*k)).copied().collect();
            if !missing.is_empty() || !extra.is_empty() {
                bad.push(format!("{code}: 缺 {:?} / 多 {:?}", missing, extra));
            }
        }
        assert!(bad.is_empty(), "HUD 语言表键集不一致：{bad:?}");
    }

    /// 语言列表**只有中文/英文**（用户口径）+ 语言文件里的 `_lang` 与文件名一致。
    #[test]
    fn only_chinese_and_english_are_built_in() {
        assert_eq!(SUPPORTED, &["zh", "en"], "HUD 语言选项只能是中文/英文");
        for code in SUPPORTED {
            let path = dir().unwrap().join(format!("{code}.json"));
            let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(raw.get("_lang").and_then(|v| v.as_str()), Some(*code),
                       "{code}.json 的 _lang 与文件名不一致");
            assert!(raw.get("_name").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()),
                    "{code}.json 缺少 _name");
        }
    }

    /// 选文件：只看 `hud_lang_path`，没写就用随包中文表（不认 `language`、不认旧语言代码）。
    #[test]
    fn pick_uses_only_hud_lang_path() {
        let (f, code, src) = pick("resource/lang/en.json");
        assert_eq!((f.as_str(), code.as_str(), src.as_str()),
                   ("resource/lang/en.json", "en", "hud_lang_path"));
        // 自建文件：语言 code 取文件名主干（只用于日志/概况）
        let (f, code, src) = pick("resource/lang/my.json");
        assert_eq!((f.as_str(), code.as_str(), src.as_str()),
                   ("resource/lang/my.json", "my", "hud_lang_path"));
        // 空（含只有空白）→ 随包中文表
        for empty in ["", "   "] {
            let (f, code, src) = pick(empty);
            assert_eq!((f.as_str(), code.as_str(), src.as_str()),
                       ("resource/lang/zh.json", "zh", "default"));
        }
    }

    /// **读不到就报错**（不静默回退）：返回 `Err`，而不是拿到一张兜底表。
    ///
    /// 这里直接走**纯函数** [`load`]（不碰进程级状态）—— `init` 是幂等的，先跑过的用例
    /// 会让后面的 `init` 直接返回"已初始化"，那样这些断言就成了永远绿的假证据。
    #[test]
    fn missing_file_is_an_explicit_error() {
        assert!(load("resource/lang/no_such_lang.json", "no_such_lang", "hud_lang_path").is_err());
        // 目录里找不到那个文件名时同样报错 ——
        // 传一个**不带路径分隔符**的名字才会走 `dir()` 那条分支
        assert!(load("no_such_file.json", "no_such_file", "hud_lang_path").is_err());
    }

    /// **解析失败也报错**：临时写一个坏 JSON，同样不许回退。
    #[test]
    fn broken_file_is_an_explicit_error() {
        let tmp = std::env::temp_dir().join("wp8f_hud_lang_broken.json");
        std::fs::write(&tmp, "{ this is not json").unwrap();
        assert!(load(&tmp.display().to_string(), "broken", "hud_lang_path").is_err());
        let _ = std::fs::remove_file(&tmp);
    }

    /// 自建语言文件（`hud_lang_path` 指向任意 `xxx.json`）能读；缺键回退内嵌 en 并记进 `missing`。
    #[test]
    fn custom_hud_lang_path_is_loaded_and_missing_keys_are_reported() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join("wp8f_hud_lang_custom.json");
        std::fs::write(&tmp, r#"{"_name":"自建","hud.f.ias":"IAS-自建"}"#).unwrap();
        let info = set_lang(&tmp.display().to_string()).unwrap();
        assert_eq!(info.lang, "wp8f_hud_lang_custom");
        assert_eq!(info.source, "set_lang");
        assert_eq!(t("hud.f.ias"), "IAS-自建");
        // 缺的键回退内嵌 en（不是键名），并且被记下来
        assert!(info.missing.contains(&"hud.f.alt"));
        assert_eq!(t("hud.f.alt"), embedded_en().get("hud.f.alt").unwrap());
        let _ = std::fs::remove_file(&tmp);
    }

    /// 回退链：当前表 → 内嵌 en → 键名本身（键名兜底是"漏翻一眼可见"的设计）。
    #[test]
    fn lookup_falls_back_en_then_key() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert!(set_lang("en").is_ok());
        assert_eq!(t("hud.f.ias"), "IAS");
        assert_eq!(t("no.such.key.at.all"), "no.such.key.at.all");
        if set_lang("zh").is_ok() {
            assert_eq!(t("hud.f.ias"), "表　速");
        }
        let _ = set_lang(DEFAULT);
    }

    /// 生效语言**只在启动时读一次**：再调 `init` 不会换语言（幂等）。
    #[test]
    fn init_is_idempotent() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ = set_lang("zh");
        let a = init("").unwrap();
        let b = init("").unwrap();
        assert_eq!(a.lang, b.lang, "init 不该在第二次调用时换语言");
        assert_eq!(a.lang, "zh");
    }

    /// `resource/lang/` 里**每个** `<name>.json` 都必须带全 HUD 用到的键
    /// （用户自建的语言文件也一样：少键只回退 en，但启动日志必须点名）。
    #[test]
    fn every_lang_file_covers_all_hud_labels() {
        let need = crate::hud::hud::all_label_keys();
        let en = embedded_en();
        for k in &need {
            assert!(en.get(k).is_some(), "内嵌 en 里没有 HUD 标签键 {k}");
        }
        for entry in std::fs::read_dir(dir().unwrap()).unwrap().flatten() {
            let p = entry.path();
            if p.extension().map(|e| e != "json").unwrap_or(true) {
                continue;
            }
            let stem = p.file_stem().unwrap().to_string_lossy().to_string();
            let t = table(&stem).unwrap_or_else(|| panic!("读不到 {}", p.display()));
            let missing: Vec<&&str> = need.iter().filter(|k| t.get(k).is_none()).collect();
            assert!(missing.is_empty(), "{} 缺 HUD 标签键：{missing:?}", p.display());
        }
    }
}
