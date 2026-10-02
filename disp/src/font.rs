//! 字体加载：正文（配置 `font_path`）与图标（随包 `icons.ttf`）两份都在初始化时
//! **按需驻留**：字体文件 mmap 进地址空间（`memmap2`），页由内核按需调入、内存紧张时可回收，
//! 渲染路径不碰文件系统（命中页缓存）。
//!
//! # 正文字体只看配置的 `font_path`
//!
//! 值形如 `"resource/fonts/SarasaMonoSC-Regular.ttf"`（自带 `resource/fonts/` 这一段）。
//! 解析：① 值原样（绝对路径 / 相对 cwd）→ ② `<基准>/<值>`；基准 = cwd → cwd 上一级 →
//! exe 目录 → exe 目录上一级（从仓库根直接跑时 cwd 就是根；产物在 `binary/` 时上一级才是根）。
//! 命中第一个**能读且能被 ab_glyph 解析**的 → `FontSource::Config`，都不行 → `Missing`。
//!
//! **没有**隐式兜底：不会"只写文件名自动去目录里找"、不会"空就用默认字体"、
//! 也不会"默认文件缺失就随便挑一个能解析的" —— 那会让用户以为换了字体却没换。
//!
//! # 图标字体
//!
//! 固定 `<基准>/resource/fonts/icons.ttf`（项目自带资源，不给用户选），同样读盘 + leak。
//!
//! # 缺字体时明确退出
//!
//! 缺任何一份 → [`missing_error`] 给一条能照着做的错误，[`crate::init_hud_layout`] 返回 `Err`，
//! `core/src/main.rs` 打印后 `exit(2)`。没有字体 HUD 一个字都画不出来，而它是覆盖在游戏上的
//! 透明窗口 —— 退出并在日志里写清"把 font_path 指到 resource/fonts/ 里的某个字体"才查得动。
//!
//! 用 `thread_local!`（而不是全局 `OnceLock<FontRef>`）是因为调用点全是 `FONT.with(|font| …)`，
//! 而 `FontRef` 只是借用视图、构造极便宜：每线程解析一次就能一个调用点都不用改。
//! "选哪个字体"仍只有一份状态（本模块的 `OnceLock`）。

use ab_glyph::FontRef;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 随包正文文件的**文件名**（`config/default.json` 的 `font_path` 末尾就是它，
/// `scripts/zip.sh` 的 `FONT_FILES` 也按它列清单）。⚠️ 它只是"配置里预置的那个值"，
/// **不是**"找不到就回退到这里"的兜底 —— 没有兜底。
pub const DEFAULT_TEXT_FILE: &str = "SarasaMonoSC-Regular.ttf";
/// 图标字体（固定 `<基准>/resource/fonts/` 下的这个文件名；`gui/src/api.rs` 会把它从可选字体里排除）。
pub const ICON_FILE: &str = "icons.ttf";
/// 字体目录名（相对每个"基准目录"；`font_path` 的值里也带这一段）
pub const FONT_DIR: &str = "resource/fonts";

/// 这次到底用了哪个字体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontSource {
    /// 配置里 `font_path` 指定的那个，从磁盘读入成功
    Config,
    /// 图标字体（固定 [`ICON_FILE`]，不给用户选）
    Icon,
    /// 读不到 → 明确失败（不静默降级）
    Missing,
}

/// 一份字体的实测结果（启动日志 / 诊断用）。
#[derive(Debug, Clone)]
pub struct FontChoice {
    pub source: FontSource,
    /// 正文 = 配置里写的原值；图标 = 固定文件名
    pub requested: String,
    /// 真正读进来的文件（`Missing` 时为 `None`）
    pub resolved: Option<PathBuf>,
    /// **从磁盘读入的字节数**（`Missing` 时为 0）
    pub bytes: usize,
    /// 非等宽字体（文件名不含 `mono` 且不在白名单里）——**只用于日志**：
    /// 提示是 GUI 的职责（`gui/src/api.rs` 的 `/api/fonts` 给出同一个判据的值）。
    pub looks_non_mono: bool,
/// 失败原因（成功时为 `None`）
    pub note: Option<String>,
}

impl FontChoice {
/// 一行人话（启动日志用），要带上"从磁盘读入 N 字节"。
    pub fn describe(&self) -> String {
        let path = self.resolved.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
        match self.source {
            FontSource::Config => format!(
                "自定义 {path}（从磁盘读入 {} 字节{}）",
                self.bytes,
                if self.looks_non_mono { "，⚠ 文件名不含 mono，可能不是等宽字体" } else { "，等宽" }
            ),
            FontSource::Icon => format!("图标 {path}（从磁盘读入 {} 字节）", self.bytes),
            FontSource::Missing => format!(
                "⚠ 读不到字体：{}（请求的是 {:?}）",
                self.note.as_deref().unwrap_or("未说明原因"),
                self.requested
            ),
        }
    }
}

// ------------------------------------------------------------------ 查找 --

/// `<基准>/resource/fonts`（按 [`crate::paths::bases`] 的顺序，去重）。图标字体用它。
pub fn font_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for b in crate::paths::bases() {
        let d = b.join(FONT_DIR);
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// 一次选字体的结果（还没 leak）。
#[derive(Debug)]
struct Picked {
    source: FontSource,
    bytes: Option<Vec<u8>>,
    path: Option<PathBuf>,
    note: Option<String>,
}

/// 正文字的查找（纯函数，可注入基准目录 → 单测不依赖真实 cwd 与真实字体）。
///
/// **只认 `font_path`**：值原样 → `<基准>/<值>`，没有"默认/扫描"兜底。
fn pick_text(requested: &str, bases: &[PathBuf]) -> Picked {
    let want = requested.trim();
    if want.is_empty() {
        return Picked {
            source: FontSource::Missing,
            bytes: None,
            path: None,
            note: Some(
                "配置里没写 font_path（现在是必填：值形如 `resource/fonts/SarasaMonoSC-Regular.ttf`）"
                    .into(),
            ),
        };
    }
    let v = Path::new(want);
    let mut cands = vec![v.to_path_buf()];
    if !v.is_absolute() {
        cands.extend(bases.iter().map(|b| b.join(v)));
    }
    let mut why = format!("{want:?} 找不到");
    for c in cands {
        let bytes = match std::fs::read(&c) {
            Ok(b) => b,
            Err(e) => {
                why = format!("{} 读不到（{e}）", c.display());
                continue;
            }
        };
        if FontRef::try_from_slice(&bytes).is_ok() {
            return Picked { source: FontSource::Config, bytes: Some(bytes), path: Some(c), note: None };
        }
        why = format!("{} 不是有效的 TTF/OTF", c.display());
        tracing::warn!("[DISP] font: {why}");
    }
    Picked { source: FontSource::Missing, bytes: None, path: None, note: Some(why) }
}

/// 图标字体的查找（纯函数）：只看 `<基准>/resource/fonts/icons.ttf`。
/// 别的 ttf/otf **不能顶替**它（图标码位不同，换一个只会画出错字形）。
fn pick_icon(dirs: &[PathBuf]) -> Picked {
    for d in dirs {
        let p = d.join(ICON_FILE);
        if let Ok(bytes) = std::fs::read(&p) {
            if FontRef::try_from_slice(&bytes).is_ok() {
                return Picked { source: FontSource::Icon, bytes: Some(bytes), path: Some(p), note: None };
            }
            tracing::warn!("[DISP] font: {} 不是有效的 TTF/OTF", p.display());
        }
    }
    Picked {
        source: FontSource::Missing,
        bytes: None,
        path: None,
        note: Some(format!("图标字体 {ICON_FILE} 不在任何 resource/fonts/ 下")),
    }
}

// ------------------------------------------------------------------ 全局 --

static TEXT: OnceLock<&'static [u8]> = OnceLock::new();
static TEXT_CHOICE: OnceLock<FontChoice> = OnceLock::new();
static ICON: OnceLock<&'static [u8]> = OnceLock::new();
static ICON_CHOICE: OnceLock<FontChoice> = OnceLock::new();
/// 初始化串行化：`init` 可能被渲染线程与测试线程同时叫到，两边都去读 25 MB 就浪费了。
static TEXT_INIT: Mutex<()> = Mutex::new(());
static ICON_INIT: Mutex<()> = Mutex::new(());

/// 把选中的字体**映射**进地址空间（不是整份读进堆）：页按需驻留，内存紧张时内核能回收，
/// 而渲染路径仍然不碰文件系统（命中页缓存；首次触碰的缺页是一次性的）。
///
/// 映射不了（极罕见：无权限 / 非常规文件）才退回"整读 + leak"。
fn map_file(path: &Path) -> Option<&'static [u8]> {
    let f = std::fs::File::open(path).ok()?;
    // SAFETY: 只读映射；文件在进程存活期间由用户自己管（改了字体要重启 HUD，与整读同口径），
    // 映射本身 Box::leak 常驻到进程结束 —— 与原来的 `Box::leak(bytes)` 生命周期一致。
    let m = unsafe { memmap2::Mmap::map(&f) }.ok()?;
    Some(&*Box::leak(Box::new(m)))
}

fn build(p: Picked, requested: &str) -> (FontChoice, Option<&'static [u8]>) {
    let looks_non_mono = p
        .path
        .as_ref()
        .and_then(|x| x.file_name().map(|n| n.to_string_lossy().to_string()))
        .map(|n| !looks_mono(&n))
        .unwrap_or(false);
    let bytes = p.bytes.as_ref().map(|b| b.len()).unwrap_or(0);
    // 优先 mmap（字体动辄 25 MB，整读常驻太占内存）；失败才 leak 那份读进来的字节。
    // 走 mmap 时 `p.bytes`（选字体时为了校验能不能解析而读的那一份）在这里被丢掉。
    let data: Option<&'static [u8]> = match p.path.as_deref().and_then(map_file) {
        Some(d) => Some(d),
        None => p.bytes.map(|v| &*Box::leak(v.into_boxed_slice())),
    };
    (
        FontChoice {
            source: p.source,
            requested: requested.to_string(),
            resolved: p.path,
            bytes,
            looks_non_mono,
            note: p.note,
        },
        data,
    )
}

/// 选正文字体（**第一次调用生效**，之后幂等）：由 [`crate::init_hud_layout`] 调用，
/// 那时配置刚读完、渲染线程还没起来。
pub fn init(font_path: &str) -> &'static FontChoice {
    if let Some(c) = TEXT_CHOICE.get() {
        return c;
    }
    let _g = TEXT_INIT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = TEXT_CHOICE.get() {
        return c;
    }
    let (choice, data) = build(pick_text(font_path, &crate::paths::bases()), font_path);
    let choice = TEXT_CHOICE.get_or_init(|| choice);
    if let Some(d) = data {
        let _ = TEXT.set(d);
    }
    choice
}

/// 选图标字体（**第一次调用生效**，幂等）。同样在 `init_hud_layout` 里调。
pub fn icon_init() -> &'static FontChoice {
    if let Some(c) = ICON_CHOICE.get() {
        return c;
    }
    let _g = ICON_INIT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = ICON_CHOICE.get() {
        return c;
    }
    let (choice, data) = build(pick_icon(&font_dirs()), ICON_FILE);
    let choice = ICON_CHOICE.get_or_init(|| choice);
    if let Some(d) = data {
        let _ = ICON.set(d);
    }
    choice
}

/// 当前选中的正文字体字节（读盘一次后常驻）。`FONT` 的 `thread_local!` 初始化器就调它，
/// 所以 [`init`] 必须在渲染线程起来之前调过（没调过时这里按"配置为空"解析一次）。
///
/// 一个字节都读不到时直接 panic 并给出可照做的错误：`FONT.with` 抛一条看得懂的消息，
/// 好过它在渲染线程里以别的方式炸掉。
pub fn data() -> &'static [u8] {
    let _ = init("");
    TEXT.get().copied().unwrap_or_else(|| panic!("{}", missing_error().unwrap_or_default()))
}

/// 图标字体字节（同上）。
pub fn icon_data() -> &'static [u8] {
    let _ = icon_init();
    ICON.get().copied().unwrap_or_else(|| panic!("{}", missing_error().unwrap_or_default()))
}

/// 本次进程的正文字体选择（未初始化时按"配置为空"解析一次）。
pub fn choice() -> &'static FontChoice {
    init("")
}

/// 图标字体的选择结果。
pub fn icon_choice() -> &'static FontChoice {
    icon_init()
}

/// 两份字体都齐了 → `None`；否则给一条错误（[`crate::init_hud_layout`] 检查，
/// `Some` 就让 `core` 打印并 `exit(2)`）。
pub fn missing_error() -> Option<String> {
    let _ = init("");
    let _ = icon_init();
    let text_bad = TEXT.get().is_none();
    let icon_bad = ICON.get().is_none();
    if !text_bad && !icon_bad {
        return None;
    }
    let mut lack: Vec<String> = Vec::new();
    if text_bad {
        lack.push(format!("正文字体（font_path = {:?}）", choice().requested));
    }
    if icon_bad {
        lack.push(format!("图标字体 {ICON_FILE}"));
    }
    let text_why = choice().note.clone().unwrap_or_default();
    // 只有一份缺时不能说"都读不到"
    let verb = if text_bad && icon_bad { "都读不到" } else { "读不到" };
    Some(format!(
        "[DISP] 字体{} {verb}：{text_why}。配置键 font_path，HUD 退出（码 2）",
        lack.join(" / ")
    ))
}

/// 正文 + 图标两份字体**映射**进来的字节数（不是 RSS：页按需驻留，实际占用通常远小于它）。
pub fn resident_bytes() -> usize {
    let _ = init("");
    let _ = icon_init();
    TEXT.get().map(|d| d.len()).unwrap_or(0) + ICON.get().map(|d| d.len()).unwrap_or(0)
}

/// 字体名里"看起来是等宽"的判据（**与 `gui/src/api.rs::is_mono_font_name` 同口径**：
/// 文件名含 `mono`，或主干命中已知等宽字体白名单）。两边各实现一次是刻意的 ——
/// GUI 不能依赖 disp（会把 winit 的依赖拖进控制台 exe）。
fn looks_mono(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.contains("mono") {
        return true;
    }
    let stem = lower.rsplit_once('.').map(|(s, _)| s).unwrap_or(&lower);
    let compact: String = stem.chars().filter(|c| !matches!(c, ' ' | '-' | '_')).collect();
    const WHITELIST: [&str; 8] = [
        "consolas", "sarasa", "firacode", "sourcecodepro", "hack", "inconsolata", "iosevka",
        "cascadiacode",
    ];
    WHITELIST.contains(&compact.as_str())
}

// ------------------------------------------------------------------ 测试 --

#[cfg(test)]
mod tests {
    use super::*;
    use ab_glyph::Font; // glyph_id / h_advance_unscaled / height_unscaled

    /// 图标字体目录必须**就是**共用基准各加一段 `resource/fonts`（与 `hud_lang_path` 同一份基准）。
    #[test]
    fn font_dirs_follow_the_shared_bases() {
        let want: Vec<PathBuf> = crate::paths::bases().into_iter().map(|b| b.join(FONT_DIR)).collect();
        assert_eq!(font_dirs(), want);
    }

    /// 仓库里那份正文字体（单测的"真实文件"来源）；用 `CARGO_MANIFEST_DIR` 拼，
    /// **不依赖 cwd**。缺文件时返回 `None`，调用方**必须**打印原因并 skip（不许静默通过）。
    fn repo_text_font() -> Option<PathBuf> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(FONT_DIR)
            .join(DEFAULT_TEXT_FILE);
        if p.is_file() {
            return Some(p);
        }
        eprintln!(
            "[SKIP] 仓库里没有 {FONT_DIR}/{DEFAULT_TEXT_FILE}（找的是 {}）——这一条需要真实字体文件\
             才能跑，跳过；打包产物里没有源文件是正常的，开发机上请确认 resource/fonts/ 在仓库根下。\
             （HUD 缺这份字体时会明确退出并说明，不会静默降级。）",
            p.display()
        );
        None
    }

    /// 一个临时目录（Drop 时删干净）。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("wp8f_p7_font_{}_{}", std::process::id(), tag));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("建临时目录");
            TempDir(p)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// ① 配置里写的相对路径（自带 `resource/fonts/`）按基准拼出来，真的从磁盘读。
    #[test]
    fn config_path_is_resolved_against_the_bases_and_read_from_disk() {
        let Some(repo) = repo_text_font() else { return };
        // <repo>/resource/fonts/<file> → 三级 parent 才是 <repo>
        let root = repo.parent().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
        let rel = format!("{FONT_DIR}/{DEFAULT_TEXT_FILE}");
        let picked = pick_text(&rel, &[root.clone()]);
        assert_eq!(picked.source, FontSource::Config, "note={:?}", picked.note);
        let bytes = picked.bytes.expect("必须真的读到了字节");
        assert_eq!(bytes.len() as u64, std::fs::metadata(&repo).unwrap().len());
        assert!(bytes.len() > 1_000_000, "随包正文应当是那份 25 MB 级的中文字体");
        assert_eq!(picked.path.unwrap(), root.join(&rel));
        // 读进来的字节必须真的能被 ab_glyph 解析（不是"读到了就算"）
        assert!(FontRef::try_from_slice(&bytes).is_ok());
    }

    /// ② 绝对路径也认（"值原样"那条候选）。
    #[test]
    fn absolute_config_path_is_used_as_is() {
        let Some(path) = repo_text_font() else { return };
        let picked = pick_text(path.to_str().unwrap(), &[]);
        assert_eq!(picked.source, FontSource::Config, "note={:?}", picked.note);
        assert_eq!(picked.path.as_deref(), Some(path.as_path()));
        assert_eq!(std::fs::metadata(&path).unwrap().len() as usize, picked.bytes.unwrap().len());
    }

    /// ③ `font_path` 为空 → `Missing`（没有"空 = 用默认字体"这种隐式行为）。
    #[test]
    fn empty_config_path_is_missing_not_a_default() {
        let Some(_p) = repo_text_font() else { return };
        // 就算基准目录里字体齐全，空值也不许"自己找一份"
        let picked = pick_text("", &crate::paths::bases());
        assert_eq!(picked.source, FontSource::Missing, "空 font_path 必须是失败：{picked:?}");
        assert!(picked.bytes.is_none());
        assert!(picked.note.is_some(), "必须给出原因");
    }

    /// ④ `font_path` 指向不存在的文件 → `Missing`，且**不会**去目录里挑别的字体顶上。
    #[test]
    fn broken_config_path_is_missing_and_never_scans_the_directory() {
        let Some(src) = repo_text_font() else { return };
        let temp = TempDir::new("nofallback");
        // 目录里**确实**放着一份能解析的字体：扫描目录的实现在这里会挑中它
        std::fs::copy(&src, temp.path().join("aaa-other-font.otf")).unwrap();
        let picked = pick_text("resource/fonts/no-such-font.ttf", &[temp.path().to_path_buf()]);
        assert_eq!(picked.source, FontSource::Missing, "不许扫描目录兜底：{picked:?}");
        assert!(picked.bytes.is_none());
    }

    /// ⑤ 文件在、但不是字体 → 同样归为 `Missing`（而不是硬塞给 ab_glyph）。
    #[test]
    fn unparsable_file_is_missing_with_a_reason() {
        let temp = TempDir::new("badfile");
        let bad = temp.path().join("not-a-font.ttf");
        std::fs::write(&bad, b"this is not a font at all").unwrap();
        let picked = pick_text(bad.to_str().unwrap(), &[temp.path().to_path_buf()]);
        assert_eq!(picked.source, FontSource::Missing);
        assert!(picked.note.is_some(), "必须给出原因");
    }

    /// ⑥ 图标字体：只看 `resource/fonts/icons.ttf`，别的 ttf 不能顶替。
    #[test]
    fn icon_font_is_fixed_and_required() {
        let temp = TempDir::new("icon");
        let icon = pick_icon(&[temp.path().to_path_buf()]);
        assert_eq!(icon.source, FontSource::Missing);
        assert!(icon.note.is_some(), "必须给出原因");
        // 仓库里那份必须真的存在且能解析
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(FONT_DIR).join(ICON_FILE);
        if !p.is_file() {
            eprintln!(
                "[SKIP] 仓库里没有 {FONT_DIR}/{ICON_FILE}（找的是 {}）——图标字体缺失时 HUD 会明确退出\
                 而不是静默降级；打包产物里没有源文件是正常的。",
                p.display()
            );
            return;
        }
        let picked = pick_icon(&font_dirs());
        assert_eq!(picked.source, FontSource::Icon, "note={:?}", picked.note);
        assert!(picked.bytes.unwrap().len() > 100_000);
    }

    /// ⑦ 全局入口：`init` 幂等、`data()` 返回的就是那些字节、`resident_bytes()` 对得上。
    #[test]
    fn init_is_idempotent_and_data_matches_resident_bytes() {
        let Some(_p) = repo_text_font() else { return };
        let rel = format!("{FONT_DIR}/{DEFAULT_TEXT_FILE}");
        let c = init(&rel);
        assert_eq!(c.source, FontSource::Config, "{c:?}");
        assert!(c.bytes > 1_000_000);
        assert_eq!(data().len(), c.bytes, "data() 必须就是这次读进来的那份");
        // 幂等：再调一次（哪怕给个别的路径）不会换字体
        assert_eq!(init("whatever.ttf").source, c.source);
        let icon = icon_init();
        assert_eq!(icon.source, FontSource::Icon, "{icon:?}");
        assert_eq!(resident_bytes(), data().len() + icon_data().len(), "常驻字节数 = 正文 + 图标");
        assert!(resident_bytes() > 1_000_000);
    }

    /// ⑧ 真正的文件 + `ab_glyph`：随包正文可解析、被判为等宽、且真的是等宽（advance 相等）。
    #[test]
    fn repo_text_font_is_loadable_and_mono() {
        let Some(path) = repo_text_font() else { return };
        let bytes = std::fs::read(&path).expect("读文件");
        let f = FontRef::try_from_slice(&bytes)
            .expect("随包正文必须能被解析（否则 HUD 一启动就退出）");
        assert!(looks_mono(DEFAULT_TEXT_FILE), "随包正文的文件名应当被判为等宽");
        let w = |c: char| f.h_advance_unscaled(f.glyph_id(c)) / f.height_unscaled();
        // Sarasa Mono SC：数字与拉丁字符的 advance 必须相等（这就是"等宽"的可测判据）
        for c in ['0', 'i', 'W', 'M'] {
            assert!((w(c) - w('0')).abs() < 1e-6, "字符 {c} 的 advance 与 '0' 不等 → 不是等宽");
        }
        // 白名单与 neg 例：不带 mono 的已知等宽字体与明显非等宽的字体
        assert!(looks_mono("Consolas.ttf"));
        assert!(!looks_mono("Arial.ttf"));
        assert!(!looks_mono("Comic Sans MS.ttf"));
    }

    /// ⑨ 字体齐全时 `missing_error()` 必须是 `None`；缺的时候文案要能照着做。
    #[test]
    fn missing_error_is_none_when_fonts_are_present() {
        let Some(_p) = repo_text_font() else { return };
        let rel = format!("{FONT_DIR}/{DEFAULT_TEXT_FILE}");
        let _ = init(&rel);
        assert!(missing_error().is_none(), "字体齐全时不许报错：{}", missing_error().unwrap());
    }

    /// ⑩ 代码里的默认值与 `config/default.json` 里的值都必须指向真实存在的字体，否则默认配置一跑就退出。
    ///
    /// 活配置是**用户的**（他可以在控制台里换成 `resource/fonts/` 下的任何字体），所以这里不钉死
    /// 具体哪一份，只钉两件会坏事的事：① 代码默认值 = 随包那份 Sarasa；② 活配置的 `font_path`
    /// 非空且指向 `resource/fonts/` 下真实存在的文件。
    #[test]
    fn shipped_default_config_points_at_the_repo_font() {
        // ① 代码里的默认（缺键时用它，也是随包分发的那份）
        let want = format!("{FONT_DIR}/{DEFAULT_TEXT_FILE}");
        assert_eq!(
            crate::default_font_path(),
            want,
            "serde default / Default::default() 里的 font_path 必须是随包那份 Sarasa"
        );
        assert!(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(&want).is_file(),
            "随包正文必须真的在仓库里：{want}"
        );

// ② 活配置（用户可能换过字体）
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let cfg = root.join("config").join("default.json");
        if !cfg.is_file() {
            eprintln!("[SKIP] 仓库里没有 {}（打包产物里没有源文件是正常的）", cfg.display());
            return;
        }
        let txt = std::fs::read_to_string(&cfg).expect("读 default.json");
        let v: serde_json::Value = serde_json::from_str(&txt).expect("default.json 必须是合法 JSON");
        let fp = v.get("font_path").and_then(|x| x.as_str()).unwrap_or("");
        assert!(!fp.trim().is_empty(), "config/default.json 的 font_path 不能为空（空 = HUD 报错退出）");
        assert!(
            fp.starts_with(&format!("{FONT_DIR}/")) || Path::new(fp).is_absolute(),
            "config/default.json 的 font_path 必须带 `{FONT_DIR}/` 这一段（或写绝对路径）：{fp}"
        );
        assert!(root.join(fp).is_file(), "它指向的文件必须真的在仓库里：{fp}");
    }
}
