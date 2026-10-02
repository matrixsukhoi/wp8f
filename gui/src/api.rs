//! API 路由与处理：路径、状态码、JSON 形状与前端约定一致。
use serde_json::{json, Value};
use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::http::{self, Request};
use crate::i18n;
use crate::Ui;

/// FM 数据库更新器的接口层（`/api/update/*`）：只把 HTTP 请求转成
/// `wp8f_flightmodel::update` 的调用。
mod update;

/// UI 侧能力（窗口/托盘/子进程）：由 main 的事件循环实现，HTTP 线程通过它驱动 UI
pub trait UiBridge: Send + Sync {
    fn tray_available(&self) -> bool;
    fn window_exists(&self) -> bool;
    fn window_shown(&self) -> bool;
    /// 结束 wp8f（「显示窗口」/关窗前先做）；**只结束本进程启动的那个**
    fn stop_wp8f(&self) -> Vec<u32>;
    /// 启动 wp8f（drag=true 时连 test-server 一起；console=true 时给它一个自己的控制台窗口）；
    /// **先结束自己启动的上一批子进程**
    fn launch_wp8f(&self, config: &str, drag: bool, console: bool) -> Result<(u32, Option<u32>), (u16, String)>;
    /// 本进程启动的 wp8f 是否还在跑（不含用户自己开的）
    fn wp8f_running(&self) -> bool;
    /// 销毁 WebView、隐藏窗口（释放 WebView2，进程留在托盘）
    fn close_window(&self) -> bool;
    /// 重建/显示窗口
    fn show_window(&self) -> bool;
    /// 控制通道状态 JSON
    fn status_json(&self) -> String;
    /// 请求退出进程
    fn request_quit(&self) -> bool;
}

pub struct App {
    pub root: PathBuf,
    pub port: u16,
    pub ui: Arc<Ui>,
}

impl App {
    fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }
    fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// 只允许 config/ 下的纯文件名（防目录穿越）
    fn safe_name(name: &str) -> Result<String, (u16, String)> {
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name == "."
            || name == ".."
            || Path::new(name).file_name().map(|f| f.to_string_lossy().to_string()) != Some(name.to_string())
        {
            return Err((400, i18n::tf("api.bad_filename", &[("name", &format!("{name:?}"))])));
        }
        // ':' 是 NTFS 的备用数据流分隔符：config/foo:bar 会写进 foo 的 ADS 而不是新文件
        if name.contains(':') {
            return Err((400, i18n::tf("api.name_colon", &[("name", &format!("{name:?}"))])));
        }
        // 设备名带扩展名也照样指向设备（nul.json 仍是 NUL），不能当配置文件写
        if is_reserved_device_name(name) {
            return Err((400, i18n::tf("api.reserved_name", &[("name", &format!("{name:?}"))])));
        }
        Ok(name.to_string())
    }

}

/// Windows 保留设备名（忽略大小写；先去掉扩展名：`nul.json` 依然是 NUL）
fn is_reserved_device_name(name: &str) -> bool {
    const DEVICES: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL",
        "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or("");
    DEVICES.iter().any(|d| stem.eq_ignore_ascii_case(d))
}

fn json_ok(stream: &mut TcpStream, value: &Value) {
    let _ = http::respond_json(stream, 200, &value.to_string());
}

fn err(stream: &mut TcpStream, code: u16, detail: &str) {
    let _ = http::respond_error(stream, code, detail);
}

/// 请求守卫（三条都在改动生效前拦截）：
/// ① 请求体超限 → 413（`http::Request::read` 不再把它读进内存）；
/// ② 跨站来源 → 403：控制台监听 127.0.0.1，任意网页都能发**简单请求**，响应读不到但副作用照做
///    （改配置 / 上传文件 / 启动进程），必须按 Origin 拒绝；
/// ③ POST 必须是非 CORS 简单类型 → 415：表单能发的三种 Content-Type 一律不收，
///    这样跨站请求连"发出去"都做不到（会先触发预检，而预检必然 404）。
fn guard_request(app: &Arc<App>, req: &Request) -> Result<(), (u16, String)> {
    if req.body_too_large {
        return Err((413, i18n::tf(
            "api.body_too_large",
            &[("n", &(http::MAX_BODY >> 20).to_string())],
        )));
    }
    if let Some(origin) = req.headers.get("origin") {
        let port = app.port;
        let ok = origin == &format!("http://127.0.0.1:{port}")
            || origin == &format!("http://localhost:{port}");
        if !ok {
            return Err((403, i18n::tf("api.cross_origin", &[("origin", origin)])));
        }
    }
    if req.method == "POST" {
        let ct = req
            .headers
            .get("content-type")
            .map(|v| v.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
            .unwrap_or_default();
        if ct != "application/json" && ct != "application/octet-stream" {
            return Err((
                415,
                i18n::tf("api.bad_content_type", &[("ct", &format!("{ct:?}"))]),
            ));
        }
    }
    Ok(())
}

/// 路由入口
pub fn handle(app: &Arc<App>, stream: &mut TcpStream, req: &Request) {
    // Rust 侧错误串也要是当前语言：main 已 `i18n::init` 过，
    // 这里是懒兜底（单元测试/未来别的入口直接调 handle 时也能用）。
    // **必须在 guard_request 之前**：请求守卫本身就是这个函数里最早会报错的地方。
    i18n::ensure(&app.root);
    if let Err((code, detail)) = guard_request(app, req) {
        return err(stream, code, &detail);
    }
    // 更新器（P3）：`/api/update/*` 命中就直接处理（含 404 之外的未知动作）
    if update::route(app, stream, req) {
        return;
    }
    let path = req.path.as_str();
    let method = req.method.as_str();

    match (method, path) {
        ("GET", "/api/health") => {
            json_ok(stream, &json!({"ok": true, "root": app.root.to_string_lossy(), "platform": "win32"}));
        }
        ("GET", "/api/system") => system(app, stream),
        // i18n：不带语言 = 用当前生效语言（config 的 `language` → 系统 → en）
        ("GET", "/api/i18n") => i18n_json(app, stream, None),
        // 字体清单：`resource/fonts/*.ttf|*.otf`（配置键 `font_path` 的取值来源）
        ("GET", "/api/fonts") => fonts_json(app, stream),
        // HUD 标签语言（C）：内置只有中文/英文两项 + `resource/lang/` 里现有文件清单
        //（配置键 `hud_lang_path`）
        ("GET", "/api/hud-lang") => hud_lang_json(app, stream),
        // 语音包（D）：`resource/voice/` 下的**子目录**（配置键 `voice_path` 的取值来源）
        ("GET", "/api/voice-packs") => voice_packs_json(app, stream),
        ("GET", "/api/resident/status") => {
            let st: Value = serde_json::from_str(&app.ui.status_json()).unwrap_or(json!({}));
            json_ok(stream, &json!({
                "available": true,
                "error": "",
                "resident": st,
            }));
        }
        ("GET", "/api/config/list") => {
            let mut names: Vec<String> = std::fs::read_dir(app.config_dir())
                .map(|rd| {
                    rd.filter_map(|e| e.ok())
                        .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                        .map(|e| e.file_name().to_string_lossy().to_string())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            json_ok(stream, &json!(names));
        }
        ("GET", "/api/config/save") => err(stream, 400, i18n::t("api.unknown_action")),
        ("POST", "/api/config/save") => config_save(app, stream, req),
        ("GET", "/api/fm/version") => fm_version(app, stream),
        ("GET", "/api/fm/aircraft") => {
            let dir = crate::env::fm_dir(&app.root);
            let mut names: Vec<String> = std::fs::read_dir(&dir)
                .map(|rd| {
                    rd.filter_map(|e| e.ok())
                        .filter(|e| e.path().extension().map(|x| x == "blkx").unwrap_or(false))
                        .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_string()))
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            json_ok(stream, &json!(names));
        }
        ("POST", "/api/wp8f/launch") => wp8f_launch(app, stream, req, false),
        ("POST", "/api/wp8f/drag-preview") => wp8f_launch(app, stream, req, true),
        ("POST", "/api/gui/window") => gui_window(app, stream, req),
        ("GET", "/api/replay/list") => replay_list(app, stream),
        ("GET", "/api/replay/load") => replay_load(app, stream, req),
        ("POST", "/api/replay/upload") => replay_upload(app, stream, req),
        // 底图：.wpr 内嵌图片的原始字节（回放把它铺到 xoy 底面上：前端按 URL 取样像素）
        ("GET", "/api/replay/map") => replay_map(app, stream, req),
        // 导出：.wpr → TacView ACMI / TacView CSV / FlatCSV（零点经纬度由调用方给）
        ("GET", "/api/replay/convert") => replay_convert(app, stream, req),
        ("GET", "/") => index(app, stream),
        ("GET", p) if p.starts_with("/static/") => static_file(app, stream, p),
        _ => {
            // 动态段：/api/config/{name}、/api/fm/{name}、/api/fm/{name}/curves、/api/i18n/{lang}
            if method == "GET" {
                if let Some(lang) = path.strip_prefix("/api/i18n/") {
                    return i18n_json(app, stream, Some(lang));
                }
                if let Some(name) = path.strip_prefix("/api/fm/") {
                    if let Some(aircraft) = name.strip_suffix("/curves") {
                        return fm(stream, app, aircraft, &["--curves-json"], req);
                    }
                    return fm(stream, app, name, &["--format", "json"], req);
                }
                if let Some(name) = path.strip_prefix("/api/config/") {
                    return config_get(app, stream, name);
                }
            }
            err(stream, 404, &i18n::t("api.not_found"));
        }
    }
}

/// 文案表：`{"lang","fallback","keys","bytes","missing","extra","strings":{…}}`。
///
/// * 不带语言 = 当前生效语言；带语言（`/api/i18n/zh_simp`）= 切到该语言并返回它（下拉要这个语义）。
///   旧代码（`zh-CN`）读取时仍归一（见 `i18n::normalize`）。
/// * `strings` = "当前语言覆盖 `en`"的合并结果（前端一次 fetch 就能冻结使用）；
///   `missing` = "`en` 有、本语言没有"的键（`ui_checks` 据此断言无缺失键）。
fn i18n_json(app: &Arc<App>, stream: &mut TcpStream, lang: Option<&str>) {
    match crate::i18n::response(&app.root, lang) {
        Ok(mut v) => {
            // 下拉要用的语言清单（code + 该语言自己的名字）也从这里给：单一来源 = resource/i18n
            if let Some(obj) = v.as_object_mut() {
                obj.insert("languages".into(), json!(crate::i18n::languages(&app.root)));
            }
            json_ok(stream, &v);
        }
        Err(detail) => {
            // 显式要一个没有语言包的语言 → 404（**不要**静默回退 en：前端下拉里
            // 的语言一定来自 `languages`，真走到这里说明请求的 code 是错的，要看得见）
            let msg = i18n::tf("api.unknown_lang", &[("lang", lang.unwrap_or(""))]);
            err(stream, 404, &format!("{msg} [{detail}]"));
        }
    }
}

/// `resource/fonts/` 下的字体清单（配置键 `font_path` 的取值来源）。
///
/// * 只列 `*.ttf` / `*.otf`，**排除 `icons.ttf`**（那是 HUD 的图标字体，不是可选的正文字体）；
/// * `mono` = "文件名含 mono（不分大小写）或命中已知等宽白名单" —— 判据只在 Rust 侧实现一次，
///   前端直接读这个字段，避免两边各写一套慢慢漂移；
/// * 返回的就是目录里真实存在的文件（没有"默认/内置"这种合成条目），
///   前端把 `font_path` 的值直接当选项值用（`resource/fonts/<文件名>`）。
fn fonts_json(app: &Arc<App>, stream: &mut TcpStream) {
    let dir = app.root.join(FONT_DIR);
    let mut out: Vec<Value> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.filter_map(|e| e.ok()) {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if ext != "ttf" && ext != "otf" {
                continue;
            }
            if name.eq_ignore_ascii_case("icons.ttf") {
                continue;
            }
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            out.push(json!({
                "name": name,
                // 前端直接拿它当 `font_path` 的值用（选完写回配置的就是这个字符串）
                "path": format!("{FONT_DIR}/{name}"),
                "size": size,
                "mono": is_mono_font_name(&name),
            }));
        }
    }
    out.sort_by(|a, b| {
        a.get("name").and_then(|v| v.as_str()).unwrap_or("")
            .cmp(b.get("name").and_then(|v| v.as_str()).unwrap_or(""))
    });
    json_ok(stream, &Value::Array(out));
}

/// `resource/fonts/` 这个目录名（与 `disp/src/font.rs::FONT_DIR` 同源）：
/// `font_path` 的值就是 `<这个>/<文件名>`，`/api/fonts` 也按它拼 `path` 字段。
/// ⚠️ 改这里要同时改 `disp/src/font.rs` 与 `scripts/zip.sh` 的 `FONT_FILES`。
const FONT_DIR: &str = "resource/fonts";

/// HUD 标签语言目录（与 `disp/src/i18n.rs::LANG_DIR` 同源）与语音包目录
/// （与 `core/src/warnings.rs` 的默认 `voice_path` 同源）。
/// ⚠️ 改这里要同时改 `disp/src/i18n.rs` / `core/src/warnings.rs` 与 `scripts/zip.sh` 的清单。
const HUD_LANG_DIR: &str = "resource/lang";
const VOICE_DIR: &str = "resource/voice";

/// **HUD 语言文件清单**（C）：配置器「HUD 语言文件」输入框的候选。
///
/// * `files` —— `resource/lang/` 下**真实存在**的 `*.json`（随包的 `zh`/`en` + 用户自建的
///   `xxx.json`），每项 `{file, path, name}`：`path`（= `<dir>/<文件名>`）直接当配置键
///   `hud_lang_path` 的值；`name` 是该文件自己的 `_name`（读不到就退回文件名主干）；
/// * `dir` —— 目录名（前端拼 `path` 用；与 `disp/src/i18n.rs::LANG_DIR` 同源，见上）；
/// * `note` —— 一句话说明这个键是干什么的（接口自描述）。
///
/// 与 `/api/i18n`（GUI 自己的 15 种界面语言）**互不影响**：HUD 的标签表和 GUI 的界面文案
/// 是两套文件、两个配置键 —— 这里列的是前者，且**写哪个就读哪个**（只有一个键）。
fn hud_lang_json(app: &Arc<App>, stream: &mut TcpStream) {
    let dir = app.root.join(HUD_LANG_DIR);
    let mut names: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.extension().map(|x| x == "json").unwrap_or(false) {
                names.push(e.file_name().to_string_lossy().to_string());
            }
        }
    }
    names.sort();
    let files: Vec<Value> = names
        .iter()
        .map(|file| {
            // `_name` 是语言文件里的自述名（"中文" / "English"）：给下拉/候选做标签用
            let name = std::fs::read_to_string(dir.join(file))
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v.get("_name").and_then(|x| x.as_str()).map(str::to_string))
                .unwrap_or_else(|| {
                    std::path::Path::new(file)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| file.clone())
                });
            json!({
                "file": file,
                "path": format!("{HUD_LANG_DIR}/{file}"),
                "name": name,
            })
        })
        .collect();
    json_ok(stream, &json!({
        "files": files,
        "dir": HUD_LANG_DIR,
        "note": "HUD 的标签语言：配置键 `hud_lang_path` 写哪个 json 就读哪个；\
                 resource/lang/ 里放一个新文件即可新增语言",
    }));
}

/// **语音包清单**：`resource/voice/` 下的**子目录**（每个子目录 = 一个语音包）。
///
/// * `name` = 子目录名；`path` = `<VOICE_DIR>/<子目录名>`（直接当 `voice_path` 的值）；
/// * `wavs` / `bytes` = 该包里 `.wav` 的个数与总字节（前端显示"这个包里有几个音"）；
/// * 散落在 `resource/voice/` **根上**的 `.wav` 一并报在 `loose` 里：语音按包加载，
///   根上的文件不会被加载 —— 让它在界面上可见，而不是静默失效。
fn voice_packs_json(app: &Arc<App>, stream: &mut TcpStream) {
    let dir = app.root.join(VOICE_DIR);
    let mut packs: Vec<Value> = Vec::new();
    let mut loose: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                let mut wavs = 0usize;
                let mut bytes = 0u64;
                if let Ok(inner) = std::fs::read_dir(&p) {
                    for f in inner.filter_map(|f| f.ok()) {
                        let fp = f.path();
                        if fp.extension().map(|x| x.eq_ignore_ascii_case("wav")).unwrap_or(false) {
                            wavs += 1;
                            bytes += std::fs::metadata(&fp).map(|m| m.len()).unwrap_or(0);
                        }
                    }
                }
                packs.push(json!({
                    "name": name,
                    "path": format!("{VOICE_DIR}/{name}"),
                    "wavs": wavs,
                    "bytes": bytes,
                }));
            } else if p.extension().map(|x| x.eq_ignore_ascii_case("wav")).unwrap_or(false) {
                loose.push(name);
            }
        }
    }
    packs.sort_by(|a, b| {
        a.get("name").and_then(|v| v.as_str()).unwrap_or("")
            .cmp(b.get("name").and_then(|v| v.as_str()).unwrap_or(""))
    });
    loose.sort();
    json_ok(stream, &json!({
        "packs": packs,
        "dir": VOICE_DIR,
        "loose": loose,
        "note": "每个子目录就是一个语音包；放一个新子目录就能新增语音包（用 voice_path 选）",
    }));
}

/// 已知等宽字体（文件名里**不含** `mono` 的那几个）：命中就不弹"可能错位"的警告。
/// 只按**文件名主干**（去扩展名、转小写）精确匹配，不做模糊匹配 —— 宁可多提示一次，
/// 也别把非等宽字体放行。
const MONO_WHITELIST: [&str; 12] = [
    "consolas", "sarasa", "sarasa-mono-sc", "fira code", "firacode", "source code pro",
    "sourcecodepro", "hack", "inconsolata", "iosevka", "cascadia code", "cascadiacode",
];

/// `mono` 判定（D18 起步口径 = 文件名 + 白名单）：
/// 文件名（含扩展名那段也算）不区分大小写地包含 `mono`，或主干命中 [`MONO_WHITELIST`]。
fn is_mono_font_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.contains("mono") {
        return true;
    }
    let stem = lower.rsplit_once('.').map(|(s, _)| s).unwrap_or(&lower);
    let compact: String = stem.chars().filter(|c| *c != ' ' && *c != '-' && *c != '_').collect();
    MONO_WHITELIST.iter().any(|w| {
        let wl = w.to_lowercase();
        let wc: String = wl.chars().filter(|c| *c != ' ' && *c != '-' && *c != '_').collect();
        stem == wl || compact == wc
    })
}

fn system(app: &Arc<App>, stream: &mut TcpStream) {
    let win = crate::env::win_exe(&app.root);
    let fm = crate::env::fm_dir(&app.root);
    let st: Value = serde_json::from_str(&app.ui.status_json()).unwrap_or(json!({}));
    json_ok(stream, &json!({
        "root": app.root.to_string_lossy(),
        "wp8f_exe": win.to_string_lossy(),
        "wp8f_exe_exists": win.is_file(),
        "config_dir": app.config_dir().to_string_lossy(),
        "logs_dir": app.logs_dir().to_string_lossy(),
        "fm_dir": fm.to_string_lossy(),
        "fm_dir_exists": fm.is_dir(),
        "tray_available": app.ui.tray_available(),
        "tray_error": "",
        "resident": true,            // 单进程：托盘/子进程监管都在本进程
        "resident_status": st,
        "window_exists": app.ui.window_exists(),
        "window_visible": app.ui.window_shown(),
        "window_pid": std::process::id(),
        "window_error": "",
        "wp8f_running": app.ui.wp8f_running(),
        "children": [],
        "job_object": true,
        "job_assigned": 0,
        "job_error": "",
    }));
}

fn config_get(app: &Arc<App>, stream: &mut TcpStream, name: &str) {
    let name = match App::safe_name(name) {
        Ok(n) => n,
        Err((code, msg)) => return err(stream, code, &msg),
    };
    let path = app.config_dir().join(&name);
    if !path.is_file() {
        return err(stream, 404, &i18n::tf("api.config_not_found", &[("name", &name)]));
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => json_ok(stream, &json!({"name": name, "content": content})),
        Err(e) => err(stream, 500, &i18n::tf("api.read_failed", &[("err", &e.to_string())])),
    }
}

/// 从配置文本里取一个**顶层字符串**键（取不到或不是字符串 → 空串）。
/// 只用于保存前的路径存在性校验：不做完整反序列化（gui 不依赖 `wp8f_disp`）。
fn raw_str(content: &str, key: &str) -> String {
    serde_json::from_str::<Value>(content)
        .ok()
        .and_then(|v| v.get(key).and_then(|x| x.as_str()).map(str::to_string))
        .unwrap_or_default()
}

fn config_save(app: &Arc<App>, stream: &mut TcpStream, req: &Request) {
    let body = req.json();
    let path_name = body.get("path").and_then(|v| v.as_str()).unwrap_or("default.json").to_string();
    let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let name = match App::safe_name(&path_name) {
        Ok(n) => n,
        Err((code, msg)) => return err(stream, code, &msg),
    };
    if let Err(e) = serde_json::from_str::<Value>(&content) {
        return err(stream, 400, &i18n::tf("api.json_syntax", &[("err", &e.to_string())]));
    }
    // 语义检查：**不拦保存**（用户可能正在编辑/先填路径后放文件），但把"HUD 起来会报错退出"
    // 的那两项作为 warning 回给前端，让它在界面上说清楚。
    let mut warnings: Vec<String> = Vec::new();
    let fields: [(&str, String, bool); 2] = [
        ("font_path", raw_str(&content, "font_path"), true),
        ("hud_lang_path", raw_str(&content, "hud_lang_path"), false),
    ];
    for (key, value, required) in fields {
        let v = value.trim();
        if v.is_empty() {
            if required {
                warnings.push(i18n::tf("api.config_path_empty", &[("key", key)]));
            }
            continue;
        }
        let p = std::path::Path::new(v);
        let abs = if p.is_absolute() { p.to_path_buf() } else { app.root.join(p) };
        if !abs.is_file() {
            warnings.push(i18n::tf("api.config_path_missing", &[("key", key), ("path", v)]));
        }
    }
    let path = app.config_dir().join(&name);
    match std::fs::write(&path, content) {
        Ok(_) => json_ok(stream, &json!({
            "ok": true,
            "path": path.to_string_lossy(),
            "warnings": warnings,
        })),
        Err(e) => err(stream, 500, &i18n::tf("api.write_failed", &[("err", &e.to_string())])),
    }
}

fn fm(stream: &mut TcpStream, app: &Arc<App>, aircraft: &str, mode: &[&str], req: &Request) {
    let fuel = req.query_f64("fuel_pct", 50.0);
    let extra = req.query_f64("extra_weight", 0.0);
    match crate::env::run_flightmodel(&app.root, aircraft, mode, fuel, extra) {
        Ok(v) => json_ok(stream, &v),
        Err((code, msg)) => err(stream, code, &msg),
    }
}

/// FM 数据库版本（本地数据自带的 `resource/data/version`，纯文本一行）。
/// 飞行模型页签显著位置与启动日志用的是**同一份值**；读不到时 `ok=false, version=null`
/// （不编造版本号，也不拿飞机的 `fmFile` 或任何别的字段凑数）。
fn fm_version(app: &Arc<App>, stream: &mut TcpStream) {
    let source = crate::env::fm_version_file(&app.root);
    let ver = crate::env::fm_db_version(&app.root);
    json_ok(stream, &json!({
        "ok": ver.is_some(),
        "version": ver,
        "source": source.to_string_lossy(),
    }));
}

fn wp8f_launch(app: &Arc<App>, stream: &mut TcpStream, req: &Request, drag: bool) {
    let body = req.json();
    let config = body
        .get("config")
        .and_then(|v| v.as_str())
        .unwrap_or("config/default.json")
        .to_string();
    let console = body.get("console").and_then(|v| v.as_bool()).unwrap_or(false);
    match app.ui.launch_wp8f(&config, drag, console) {
        Ok((pid, ts)) => {
            if drag {
                json_ok(stream, &json!({
                    "ok": true,
                    "via": "gui",
                    "pids": {
                        "test_server": ts.map(|p| json!(p)).unwrap_or(Value::Null),
                        "wp8f": pid,
                    }
                }));
            } else {
                json_ok(stream, &json!({
                    "ok": true,
                    "pid": pid,
                    "via": "gui",
                    "test_server_pid": ts.map(|p| p as i64).unwrap_or(-1),
                }));
            }
        }
        Err((code, msg)) => err(stream, code, &msg),
    }
}

fn gui_window(app: &Arc<App>, stream: &mut TcpStream, req: &Request) {
    let action = req.json().get("action").and_then(|v| v.as_str()).unwrap_or("show").to_string();
    match action.as_str() {
        "close" | "hide" => {
            if !app.ui.tray_available() {
                return err(stream, 409, i18n::t("api.no_tray"));
            }
            // 「开 始」= 只关窗口：销毁 WebView 释放 WebView2，wp8f 必须继续跑
            // （kill wp8f 是「显示主窗口」的语义，别搞反）
            let ok = app.ui.close_window();
            json_ok(stream, &json!({"ok": ok, "closing": true}));
        }
        "show" => {
            let killed = app.ui.stop_wp8f();
            let ok = app.ui.show_window();
            json_ok(stream, &json!({"ok": ok, "killed": killed}));
        }
        other => err(stream, 400, &i18n::tf("api.unknown_action_x", &[("action", other)])),
    }
}

/// 录制文件列表：**只列 `.wpr`**（回放只支持 wp8f 飞行记录）。
///
/// 路由名从历史路径 `/api/acmi/list` 改成了 `/api/replay/list`（本页签是"回放"，
/// 老名字是早就删掉的 ACMI 解析器留下的；`.acmi` / `.csv` 在这里既不是输入也不是输出）。
/// `.acmi` / `.csv` 的解析与收录**已彻底删除**：它们不再出现在列表里，
/// 也不再能加载（见 [`replay_load`]）—— 不留"半支持"状态。
fn replay_list(app: &Arc<App>, stream: &mut TcpStream) {
    let dir = app.logs_dir();
    let mut out: Vec<Value> = Vec::new();
    let mut push = |path: &Path| {
        if let Ok(md) = std::fs::metadata(path) {
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.push(json!({
                "name": path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                "size": md.len(),
                "mtime": mtime,
            }));
        }
    };
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        entries.sort();
        for p in &entries {
            // 扩展名忽略大小写：upload 按小写认，X.WPR 上传后必须能在这里列出来
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if ext == wpr_ext() {
                push(p);
            }
        }
    }
    out.sort_by(|a, b| {
        let am = a.get("mtime").and_then(|v| v.as_i64()).unwrap_or(0);
        let bm = b.get("mtime").and_then(|v| v.as_i64()).unwrap_or(0);
        bm.cmp(&am)
    });
    json_ok(stream, &Value::Array(out));
}

/// 加载一份 `.wpr` 飞行记录（回放数据的唯一来源）。
///
/// 非 `.wpr` 一律明确报错（400/404），不按内容嗅探 ACMI/CSV
/// （ACMI 只剩"导出目标格式"这一层含义，见 [`replay_convert`]）。
fn replay_load(app: &Arc<App>, stream: &mut TcpStream, req: &Request) {
    let file = req.query_get("file").unwrap_or_default();
    if file.is_empty() || file.contains('/') || file.contains('\\') {
        return err(stream, 400, &i18n::tf("api.bad_filename", &[("name", &format!("{file:?}"))]));
    }
    let name = Path::new(&file)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let path = app.logs_dir().join(&name);
    if !path.is_file() {
        return err(stream, 404, &i18n::tf("api.config_not_found", &[("name", &name)]));
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext != wpr_ext() {
        return err(
            stream,
            400,
            &i18n::tf("api.wpr_only", &[("ext", &ext), ("name", &name)]),
        );
    }
    match crate::record::parse_wpr_file(&path) {
        Ok(v) => json_ok(stream, &v),
        Err(e) => err(stream, 500, &i18n::tf("api.parse_failed", &[("err", &e)])),
    }
}

/// logs 下的 `.wpr` 路径解析（`convert` 与 `map` 共用）：非法文件名 / 非 .wpr / 不存在都返回 4xx。
fn logs_wpr_path(app: &Arc<App>, req: &Request) -> Result<(String, PathBuf), (u16, String)> {
    let file = req.query_get("file").unwrap_or_default();
    if file.is_empty() || file.contains('/') || file.contains('\\') {
        return Err((400, i18n::tf("api.bad_filename", &[("name", &format!("{file:?}"))])));
    }
    let name = Path::new(&file).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let path = app.logs_dir().join(&name);
    if !path.is_file() {
        return Err((404, i18n::tf("api.file_not_found", &[("name", &name)])));
    }
    if path.extension().map(|e| !e.eq_ignore_ascii_case(wpr_ext())).unwrap_or(true) {
        return Err((404, i18n::tf("api.not_wpr", &[("name", &name)])));
    }
    Ok((name, path))
}

/// `.wpr` 内嵌的地图底图 → 原始图片字节（Content-Type 取记录里的 `img_mime`）。
/// 与 `/api/replay/load` 分开的理由见 `record` 模块注释：底图是几百 KB 的二进制，
/// 内联进 JSON 要膨胀 33% 且每次加入文件都重传；单独端点还能让浏览器按 URL 缓存。
fn replay_map(app: &Arc<App>, stream: &mut TcpStream, req: &Request) {
    let (_name, path) = match logs_wpr_path(app, req) {
        Ok(v) => v,
        Err((code, msg)) => return err(stream, code, &msg),
    };
    match crate::record::read_map_image(&path) {
        Ok((mime, bytes)) => {
            // 记录文件不可变（新录制=新文件），可以放心让浏览器缓存
            let _ = http::respond_bytes(stream, 200, &mime, &bytes, &[("Cache-Control", "max-age=3600")]);
        }
        Err(e) => err(stream, 404, &e),
    }
}

/// `.wpr` → 导出文件（写进 logs/，与录制文件同处，回放页签立刻能选到）。
/// 查询串：`file`（logs 下的 .wpr）、`format`（acmi/csv/flat）、`lat`、`lon`。
/// 这里的 `acmi` 是**导出目标格式名**（TacView 的 `text/acmi/tacview`），与旧路由名无关。
fn replay_convert(app: &Arc<App>, stream: &mut TcpStream, req: &Request) {
    let format = req.query_get("format").unwrap_or_else(|| "acmi".to_string());
    let (name, path) = match logs_wpr_path(app, req) {
        Ok(v) => v,
        Err((code, msg)) => return err(stream, code, &msg),
    };
    // 零点经纬度：缺省用日志默认（莫斯科市中心），非法值直接 400（别让它静默变成 0,0）
    let lat = match req.query_get("lat") {
        Some(v) if !v.trim().is_empty() => v.trim().parse::<f64>().unwrap_or(f64::NAN),
        _ => wp8f_logger::convert::Origin::default().lat,
    };
    let lon = match req.query_get("lon") {
        Some(v) if !v.trim().is_empty() => v.trim().parse::<f64>().unwrap_or(f64::NAN),
        _ => wp8f_logger::convert::Origin::default().lon,
    };
    if !lat.is_finite() || !lon.is_finite() || lat.abs() > 90.0 || lon.abs() > 180.0 {
        return err(stream, 400, &i18n::tf(
            "api.bad_origin",
            &[("lat", &lat.to_string()), ("lon", &lon.to_string())],
        ));
    }

    let (suffix, text) = match crate::record::convert_wpr_file(&path, &format, lat, lon) {
        Ok(v) => v,
        Err(e) => return err(stream, 400, &e),
    };
    let stem = Path::new(&name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let out = format!("{stem}{suffix}");
    let target = app.logs_dir().join(&out);
    match std::fs::write(&target, text.as_bytes()) {
        Ok(_) => json_ok(
            stream,
            &json!({"ok": true, "name": out, "size": text.len(), "format": format}),
        ),
        Err(e) => err(stream, 500, &i18n::tf("api.write_failed", &[("err", &e.to_string())])),
    }
}

/// 飞行记录扩展名（单一来源：`wp8f_logger::wpr::EXT`）。
fn wpr_ext() -> &'static str {
    wp8f_logger::wpr::EXT
}

/// 上传本地录制文件（「选择文件」用）：POST 原始字节 + `?name=<文件名>`。
/// **只接受 `.wpr`**（回放只支持飞行记录）。
fn replay_upload(app: &Arc<App>, stream: &mut TcpStream, req: &Request) {
    // 16MB：`.wpr` 里带地图底图，比纯文本录制大一圈
    const MAX: usize = 16 * 1024 * 1024;
    let raw_name = req.query_get("name").unwrap_or_default();
    let base = std::path::Path::new(&raw_name)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let lower = base.to_lowercase();
    let ext_ok = lower.ends_with(&format!(".{}", wpr_ext()));
    if base.is_empty() || base.contains('/') || base.contains('\\') || !ext_ok {
        return err(stream, 400, &i18n::tf("api.upload_name", &[("name", &format!("{raw_name:?}"))]));
    }
    // 必须用原始字节：req.body 是 lossy 转换，GBK 录制的每个非法字节都会变成
    // U+FFFD，落盘即损坏，返回的 size 也成了替换后的长度
    let body = req.body_bytes.as_slice();
    if body.is_empty() {
        return err(stream, 400, i18n::t("api.empty_file"));
    }
    if body.len() > MAX {
        return err(stream, 400, &i18n::tf("api.file_too_large", &[("n", &body.len().to_string())]));
    }
    let dir = app.logs_dir();
    // 重名不覆盖：foo.wpr → foo_1.wpr → foo_2.wpr …
    let stem = std::path::Path::new(&base)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "import".into());
    let ext = std::path::Path::new(&base)
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| wpr_ext().to_string());
    // 用 create_new 一步完成「不存在才创建」：先 exists() 再 write() 之间有竞态，
    // 两个并发上传会互相覆盖，而接口承诺的是「重名自动加序号，不覆盖」
    let mut target = dir.join(&base);
    let mut n = 1;
    let mut file = loop {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&target) {
            Ok(f) => break f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if n > 500 {
                    return err(stream, 500, i18n::t("api.too_many_same_name"));
                }
                target = dir.join(format!("{stem}_{n}.{ext}"));
                n += 1;
            }
            Err(e) => return err(stream, 500, &i18n::tf("api.write_failed", &[("err", &e.to_string())])),
        }
    };
    match file.write_all(body) {
        Ok(_) => json_ok(
            stream,
            &json!({
                "ok": true,
                "name": target.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                "size": body.len(),
            }),
        ),
        Err(e) => {
            // 半截文件留在 logs/ 里会出现在回放列表且必然解析失败，直接删掉
            let _ = std::fs::remove_file(&target);
            err(stream, 500, &i18n::tf("api.write_failed", &[("err", &e.to_string())]))
        }
    }
}

fn index(app: &Arc<App>, stream: &mut TcpStream) {
    let path = http::static_root(&app.root).join("index.html");
    match std::fs::read(&path) {
        Ok(body) => {
            // no-store：防止 WebView2 缓存旧页面/脚本（曾导致配置器读不出配置）
            let _ = http::respond_bytes(stream, 200, "text/html; charset=utf-8", &body,
                                        &[("Cache-Control", "no-store, must-revalidate")]);
        }
        Err(e) => err(stream, 500, &i18n::tf("api.index_read_failed", &[("err", &e.to_string())])),
    }
}

fn static_file(app: &Arc<App>, stream: &mut TcpStream, path: &str) {
    let rel = path.trim_start_matches("/static/");
    let base = http::static_root(&app.root);
    match http::safe_join(&base, rel) {
        Some(full) => match std::fs::read(&full) {
            Ok(body) => {
                // no-store：与首页同策略 —— 静态资源也必须禁止缓存，否则 WebView2 会
                // 按启发式复用磁盘上的 app.js/style.css 会让配置器读不出配置
                let _ = http::respond_bytes(stream, 200, http::content_type(&full), &body,
                                            &[("Cache-Control", "no-store, must-revalidate")]);
            }
            Err(e) => err(stream, 404, &i18n::tf("api.read_failed", &[("err", &e.to_string())])),
        },
        None => err(stream, 404, i18n::t("api.not_found")),
    }
}
