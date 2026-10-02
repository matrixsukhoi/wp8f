//! wp8f 控制台 —— 单进程：托盘 + 本地 HTTP API + wry/WebView2 窗口。
//!
//! ```text
//! wp8f-gui.exe（本进程）
//!   ├─ 系统托盘（Shell_NotifyIcon，关窗后仍在）
//!   ├─ 本地 HTTP API（页面 + 配置/FM/飞行记录）
//!   ├─ wry/WebView2 窗口（关窗时销毁 WebView → 6 个 WebView2 子进程退出；可在同一窗口重建）
//!   ├─ wp8f.exe              业务进程（本进程拉起，比窗口活得久）
//!   └─ test-server.exe       拖拽预览服务端（预览时与 wp8f 互相兜底回收）
//! ```
//!
//! 两个子进程只按本进程启动时记下的 pid 管理：用户自己开的 wp8f.exe 既不阻止启动，也不会被结束。
//!
//! 单实例靠 **HTTP 端口的独占性**判定：绑不上 `127.0.0.1:<port>` 说明该端口已有控制台在服务，
//! 于是把它的窗口叫出来再自己退出。
//!
//! 用法：
//!     wp8f-gui.exe                     # 托盘 + 打开配置窗口（正常用法）
//!     wp8f-gui.exe --no-window         # 只常驻托盘（不建窗口；供自检/CI）
//!     wp8f-gui.exe --browser           # 不开窗口，用系统浏览器打开页面
//!     wp8f-gui.exe --no-tray-promote   # 不把托盘图标提升为任务栏直接显示
#![windows_subsystem = "windows"]

mod api;
mod children;
mod control;
mod env;
mod http;
mod i18n;
mod record;
mod tray;
mod win;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
use wry::{WebContext, WebView, WebViewBuilder};

// 窗口标题 / 消息框 / 托盘菜单的文案来自 i18n（`resource/i18n/<lang>.json` 的 `gui.*` 键，
// 语言取自 config 的 `language` 键），本文件里没有硬编码的中文界面串。
//
// ⚠️ 例外是**写 stdout 的那几条诊断**（见 port_busy_exit）：探针（`scripts/tests/*.py`）按它们
// + 退出码判定"端口被谁占着"，跟着界面语言变会让判据漂移，所以那几条一律英文常量。
// 控制台日志（log() → logs/console-host.log）仍是中文：那是给维护者看的，
// 且 `tray_e2e.py` 按 `托盘已启动` 这一行断言。

/// 窗口固定 16:9；最小尺寸同样保持 16:9（1024x576），避免布局被压坏
const ASPECT: f64 = 16.0 / 9.0;
const MIN_W: f64 = 1024.0;
const MIN_H: f64 = MIN_W / ASPECT;      // 576
/// 默认占主屏可用高度/宽度的比例（留出任务栏与呼吸空间）
const FIT_H: f64 = 0.86;
const FIT_W: f64 = 0.80;

/// 主屏逻辑尺寸（拿不到就按 1920x1080 估）
fn screen_size(event_loop: &tao::event_loop::EventLoop<UserEvent>) -> (f64, f64) {
    event_loop
        .primary_monitor()
        .map(|m| {
            let s = m.size();
            let sf = m.scale_factor();
            (s.width as f64 / sf, s.height as f64 / sf)
        })
        .unwrap_or((1920.0, 1080.0))
}

/// 16:9 尺寸：按主屏可用区域自适应
fn window_size(event_loop: &tao::event_loop::EventLoop<UserEvent>) -> (f64, f64) {
    let (sw, sh) = screen_size(event_loop);
    let mut h = (sh * FIT_H).min(sw * FIT_W / ASPECT);
    let mut w = h * ASPECT;
    if w < MIN_W {
        w = MIN_W;
        h = MIN_H;
    }
    (w.round(), h.round())
}

// ---------------------------------------------------------------- 日志 --

static LOG_F: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn log(msg: &str) {
    let line = format!("[gui] {msg}\n");
    if let Ok(mut g) = LOG_F.lock() {
        if let Some(f) = g.as_mut() {
            use std::io::Write;
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
    }
}

// ------------------------------------------------------- 事件（UI 线程） --

pub enum UserEvent {
    /// 重建（或显示）窗口
    ShowWindow,
    /// 销毁 WebView（释放 WebView2）并隐藏窗口
    CloseWindow,
    /// 退出进程（托盘「退出」/ 控制通道）
    Quit,
}

// ------------------------------------------------------------ UI 状态桥 --

/// 提供给 HTTP 线程使用的 UI 桥：请求都通过事件代理投到 UI 线程执行
pub struct Ui {
    pub proxy: EventLoopProxy<UserEvent>,
    pub tray_ok: AtomicBool,
    pub window_shown: AtomicBool,
    pub webview_alive: AtomicBool,
    pub children: Arc<children::Children>,
}

impl Ui {
    fn send(&self, ev: UserEvent) -> bool {
        self.proxy.send_event(ev).is_ok()
    }
}

impl api::UiBridge for Ui {
    fn tray_available(&self) -> bool {
        self.tray_ok.load(Ordering::SeqCst)
    }
    fn window_exists(&self) -> bool {
        self.webview_alive.load(Ordering::SeqCst)
    }
    fn window_shown(&self) -> bool {
        self.window_shown.load(Ordering::SeqCst)
    }
    fn stop_wp8f(&self) -> Vec<u32> {
        self.children.stop_wp8f()
    }
    fn launch_wp8f(&self, config: &str, drag: bool, console: bool) -> Result<(u32, Option<u32>), (u16, String)> {
        // 一个控制台只允许一个自己启动的 wp8f（已启动则启动失败，500 + 原因）；
        // 不替用户杀进程 —— 预览里刚拖好的位置不该被悄悄丢掉
        let (pid, ts) = self.children.launch_wp8f(config, drag, console).map_err(|e| (500, e))?;
        // 进程创建成功不等于跑起来了：启动期失败（如参数被 clap 拒绝）要在这里拦成
        // 500 + logs/wp8f.log 末尾，否则前端会一路报「已启动」然后关窗
        self.children.verify_wp8f_alive(pid)?;
        Ok((pid, ts))
    }
    fn wp8f_running(&self) -> bool {
        self.children.wp8f_running()
    }
    fn close_window(&self) -> bool {
        self.send(UserEvent::CloseWindow)
    }
    fn show_window(&self) -> bool {
        self.send(UserEvent::ShowWindow)
    }
    fn status_json(&self) -> String {
        self.children
            .status_json(self.webview_alive.load(Ordering::SeqCst), std::process::id())
    }
    fn request_quit(&self) -> bool {
        self.send(UserEvent::Quit)
    }
}

// ---------------------------------------------------------------- 参数 --

struct Args {
    root: PathBuf,
    port: u16,
    browser: bool,
    no_window: bool,
    no_tray_promote: bool,
}

/// 仓库根标记：前端静态资源（或 Cargo.toml + config/）
fn looks_like_root(p: &std::path::Path) -> bool {
    p.join("gui").join("static").join("index.html").is_file()
        || (p.join("Cargo.toml").is_file() && p.join("config").is_dir())
}

fn parse_args() -> Args {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    let exe_dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let mut a = Args {
        root: exe_dir,
        port: 8765,
        browser: false,
        no_window: false,
        no_tray_promote: false,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let next = |i: usize| argv.get(i + 1).cloned().unwrap_or_default();
        match argv[i].as_str() {
            "--root" => { a.root = PathBuf::from(next(i)); i += 2; }
            "--port" => { a.port = next(i).parse().unwrap_or(8765); i += 2; }
            "--browser" => { a.browser = true; i += 1; }
            "--no-window" => { a.no_window = true; i += 1; }
            "--no-tray-promote" => { a.no_tray_promote = true; i += 1; }
            _ => i += 1,
        }
    }
    // 兜底：`--root "C:\path\"` 这种写法在 Windows 命令行解析里会变成 `C:\path"`，
    // 这里统一去掉尾部反斜杠/引号，任何调用方都不会再踩这个坑
    let cleaned = a
        .root
        .to_string_lossy()
        .trim_end_matches(|c| c == '"' || c == '\\' || c == '/')
        .to_string();
    if !cleaned.is_empty() {
        a.root = PathBuf::from(cleaned);
    }
    if let Ok(p) = a.root.canonicalize() {
        a.root = p;
    }
    if !looks_like_root(&a.root) {
        let mut probe = a.root.clone();
        for _ in 0..5 {
            match probe.parent() {
                Some(p) => {
                    if looks_like_root(p) {
                        a.root = p.to_path_buf();
                        break;
                    }
                    probe = p.to_path_buf();
                }
                None => break,
            }
        }
    }
    a
}

// ---------------------------------------------------------------- main --

/// 端口被占用时的退出码（探针靠"退出码 + stdout 文案"区分两种原因，见 gui/README.md）
const EXIT_PORT_TAKEN: i32 = 2; // 端口被**别的程序**占用 → 换 --port / 结束占用进程
const EXIT_HOST_STUCK: i32 = 3; // 已有控制台但无法唤醒 → 结束它再启动

fn main() {
    let args = parse_args();

    // ---- 单实例：HTTP 端口的独占性就是互斥体 ----
    // 绑得上 = 本进程就是唯一实例；绑不上 = 端口上已有服务者，要么是已有控制台（叫出它的窗口），
    // 要么是别的程序（明确报错并给建议）——两种情况的提示与退出码不同，见 port_busy_exit。
    // 代价：判据按端口而不是全机唯一，所以换 `--port` 可以并行跑第二个控制台（自检要用）。
    let listener = match http::bind_port(args.port) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => port_busy_exit(&args),
        Err(e) => {
            // 其它绑定错误（权限/端口非法/地址不可用）不能静默顺延端口：那会"看起来启动成功"
            // 却把服务开在别的端口上。stdout 用英文固定串（探针口径），弹窗文案走 i18n；
            // 此时还没走到 i18n::init，先 ensure 一次（懒初始化）。
            say(&format!("console failed to start: cannot bind 127.0.0.1:{}: {e}", args.port));
            if !args.no_window {
                i18n::ensure(&args.root);
                let port = args.port.to_string();
                let err = e.to_string();
                message_box(
                    i18n::t("gui.dlg.start_failed.title"),
                    &i18n::tf("gui.dlg.start_failed.body", &[("port", &port), ("err", &err)]),
                );
            }
            std::process::exit(1);
        }
    };

    let _ = std::fs::create_dir_all(args.root.join("logs"));
    if let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(args.root.join("logs").join("console-host.log"))
    {
        if let Ok(mut g) = LOG_F.lock() {
            *g = Some(f);
        }
    }
    log(&format!("启动 pid={} root={}", std::process::id(), args.root.display()));

    // i18n：**启动时读一次** `resource/i18n/<lang>.json`（语言取自 config 的 `language` 键，
    // `auto` = 跟随系统），构造成只读表后把 JSON 文本与中间对象立刻释放（见 i18n.rs）。
    // 界面文案的唯一来源就是那些 JSON —— 前端拿到的是同一个目录（经 /api/i18n）。
    // 日志经 take_log 取回：gui 没有 tracing 依赖，控制台日志统一走本文件的 log()。
    i18n::init(&args.root);
    for line in i18n::take_log() {
        log(&line);
    }

    // FM 数据库版本（本地飞机性能数据自带的 `resource/data/version`）：飞行模型页签上显示同一份值。
    // 数据没搬到新路径（resource/data）时这里就是"未知"，与页签上写的一致。
    log(&match crate::env::fm_db_version(&args.root) {
        Some(v) => format!("FM 数据版本 {v}"),
        None => "FM 数据版本未知（未找到本地飞机性能数据的 resource/data/version）".to_string(),
    });

    if !args.root.join("gui").join("static").join("index.html").is_file() {
        let msg = i18n::tf(
            "gui.dlg.no_static.body",
            &[("path", &args.root.display().to_string())],
        );
        log(&msg.replace('\n', " "));
        message_box(i18n::t("gui.dlg.no_static.title"), &msg);
    }

    // 子进程监管（作业对象：本进程无论怎么死，wp8f/test-server 一起结束）
    let kids = match children::Children::new(args.root.clone()) {
        Some(k) => Arc::new(k),
        None => {
            log("作业对象创建失败");
            std::process::exit(1);
        }
    };
    kids.clone().start_reaper();

    // ---- 事件循环 + 窗口（先建窗口对象，WebView 按需创建）----
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // 16:9 窗口，按主屏可用区域自适应并居中
    let (sw, sh) = screen_size(&event_loop);
    let (win_w, win_h) = window_size(&event_loop);
    let win_x = ((sw - win_w) / 2.0).max(0.0);
    let win_y = ((sh - win_h) / 2.0).max(0.0);
    log(&format!("窗口尺寸 {win_w:.0}x{win_h:.0}（16:9，居中）"));

    let window: Option<Window> = match WindowBuilder::new()
        .with_title(i18n::t("gui.window.title"))
        .with_inner_size(LogicalSize::new(win_w, win_h))
        .with_min_inner_size(LogicalSize::new(MIN_W, MIN_H))
        .with_position(tao::dpi::LogicalPosition::new(win_x, win_y))
        .with_visible(false)
        .build(&event_loop)
    {
        Ok(w) => Some(w),
        Err(e) => {
            log(&format!("建窗失败：{e}"));
            None
        }
    };

    // ---- HTTP API ----
    // 监听套接字在 main 开头就绑好了（那是单实例判据）；这里只把它交给服务线程。
    // 绑的就是 args.port：端口被占用时直接报错，不顺延。
    let port = args.port;
    let url = format!("http://127.0.0.1:{port}");

    let ui = Arc::new(Ui {
        proxy: proxy.clone(),
        tray_ok: AtomicBool::new(false),
        window_shown: AtomicBool::new(false),
        webview_alive: AtomicBool::new(false),
        children: Arc::clone(&kids),
    });
    let app = Arc::new(api::App { root: args.root.clone(), port, ui: Arc::clone(&ui) });
    let app_for_http = Arc::clone(&app);
    http::serve(listener, move |stream, req| api::handle(&app_for_http, stream, req));
    log(&format!("HTTP 服务就绪 {url}"));

    // ---- 控制通道（供工具/自检；特权操作在同一进程内直接执行）----
    match control::serve(Arc::clone(&app), Arc::clone(&ui)) {
        Ok((control_url, token)) => {
            let _ = std::fs::write(
                args.root.join("logs").join("console-host.json"),
                format!(
                    "{{\"pid\":{},\"control_url\":\"{}\",\"token\":\"{}\",\"port\":{}}}",
                    std::process::id(),
                    control_url,
                    token,
                    port
                ),
            );
            log(&format!("控制通道 {control_url}"));
        }
        Err(e) => log(&format!("控制通道启动失败：{e}")),
    }

    // ---- 托盘 ----
    let ui_tray = Arc::clone(&ui);
    let proxy_tray = proxy.clone();
    let proxy_exit = proxy.clone();
    let actions = tray::Actions {
        on_show: Box::new(move || {
            // 只结束**本进程启动的** wp8f（stop_wp8f 按自己记录的 pid 判断），
            // 用户自己开的 HUD 一律不动
            ui_tray.children.stop_wp8f();
            let _ = proxy_tray.send_event(UserEvent::ShowWindow);
        }),
        on_exit: Box::new(move || {
            let _ = proxy_exit.send_event(UserEvent::Quit);
        }),
    };
    // 托盘的提示气泡与右键菜单同样走 i18n：安装发生在 i18n::init（上面）之后，
    // 菜单项还是"右键那一刻现建"的（tray.rs），所以切了语言下次右键就是新语言。
    let tray_ok = tray::install(actions, i18n::t("gui.tray.tooltip"), !args.no_tray_promote);
    ui.tray_ok.store(tray_ok, Ordering::SeqCst);
    // 这行日志照实报**菜单项文案**（与 tray.rs 右键时 t() 的是同一对键），
    // 顺便让"托盘菜单确实接了 i18n"在日志里可查（前缀 `托盘已启动` 由 tray_e2e.py 断言）。
    if tray_ok {
        log(&format!(
            "托盘已启动 · 菜单：{} / {}",
            i18n::t("gui.tray.show"),
            i18n::t("gui.tray.exit")
        ));
    } else {
        log("托盘注册失败（WebView2 窗口仍可用）");
    }

    // WebView2 用户数据目录：默认会在 exe 旁边建 <exe>.WebView2/，这里指到 logs/ 下
    let mut web_context = WebContext::new(Some(args.root.join("logs").join("webview2-data")));

    // ---- 首个 WebView ----
    let mut webview: Option<WebView> = None;
    if args.browser {
        open_browser(&url);
        log("浏览器模式：不开窗口");
    } else if !args.no_window {
        match window.as_ref() {
            Some(w) => match WebViewBuilder::new_with_web_context(&mut web_context)
                .with_url(&url)
                .with_background_color((245, 246, 248, 255))
                .build(w)
            {
                Ok(wv) => {
                    webview = Some(wv);
                    ui.webview_alive.store(true, Ordering::SeqCst);
                    ui.window_shown.store(true, Ordering::SeqCst);
                    log(&format!("窗口已打开 {url}"));
                }
                Err(e) => {
                    log(&format!("WebView2 创建失败：{e}（退化为浏览器模式）"));
                    open_browser(&url);
                }
            },
            None => open_browser(&url),
        }
    } else {
        log("--no-window：只常驻托盘");
    }
    if let Some(w) = window.as_ref() {
        w.set_visible(ui.window_shown.load(Ordering::SeqCst));
    }

    // ---- 事件循环（run 不返回：退出清理放在 Quit 分支里）----
    let root_for_exit = args.root.clone();
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::CloseWindow) => {
                // 「开 始」/关窗：销毁 WebView（6 个 WebView2 子进程退出）并隐藏窗口
                webview = None;
                ui.webview_alive.store(false, Ordering::SeqCst);
                ui.window_shown.store(false, Ordering::SeqCst);
                if let Some(w) = window.as_ref() {
                    w.set_visible(false);
                }
                log("窗口已关闭：WebView2 资源已释放，托盘继续常驻");
            }
            Event::UserEvent(UserEvent::ShowWindow) => match window.as_ref() {
                Some(w) => {
                    if webview.is_none() {
                        match WebViewBuilder::new_with_web_context(&mut web_context)
                            .with_url(&url)
                            .with_background_color((245, 246, 248, 255))
                            .build(w)
                        {
                            Ok(wv) => {
                                webview = Some(wv);
                                ui.webview_alive.store(true, Ordering::SeqCst);
                                log("窗口已重建（WebView2 重新加载）");
                            }
                            Err(e) => log(&format!("WebView2 重建失败：{e}")),
                        }
                    }
                    w.set_visible(true);
                    w.set_focus();
                    ui.window_shown.store(true, Ordering::SeqCst);
                }
                None => {
                    log("没有窗口对象可用，改用浏览器");
                    open_browser(&url);
                }
            },
            Event::UserEvent(UserEvent::Quit) => {
                log("收到退出请求：结束子进程并移除托盘图标");
                tray::uninstall();
                let kids = Arc::clone(&ui.children);
                let killed = kids.kill_all();
                let _ = std::fs::remove_file(root_for_exit.join("logs").join("console-host.json"));
                log(&format!("退出：已结束子进程 {killed:?}"));
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                // 点 X：与「开始」一致 —— 释放 WebView，进程与托盘继续
                webview = None;
                ui.webview_alive.store(false, Ordering::SeqCst);
                ui.window_shown.store(false, Ordering::SeqCst);
                if let Some(w) = window.as_ref() {
                    w.set_visible(false);
                }
                log("窗口关闭请求：WebView2 已释放，托盘继续常驻");
            }
            _ => {}
        }
    });

}

// ------------------------------------------------------------ 小工具 --

fn open_browser(url: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let op = win::wide("open");
    let u = win::wide(url);
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), u.as_ptr(),
                      std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

fn message_box(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK};
    let t = win::wide(title);
    let x = win::wide(&text.replace('\n', "\r\n"));
    unsafe {
        MessageBoxW(std::ptr::null_mut(), x.as_ptr(), t.as_ptr(), MB_OK | MB_ICONWARNING);
    }
}

/// 一行提示写到 stdout。**不用 `println!`**：GUI 子系统（双击/计划任务）下没有控制台，
/// 写 stdout 会失败，而 `println!` 写失败就 panic —— 这里显式忽略错误即可。
fn say(msg: &str) {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{msg}");
    let _ = out.flush();
}

/// 唤醒已有实例的结果 —— 决定提示文案与退出码（见 `port_busy_exit`）
enum Wake {
    /// 已让它的窗口显示出来（带上运行态里的 pid）
    Woke(u32),
    /// 运行态文件指向本端口的控制台，但它不应答（卡死/无权访问）
    Stuck(u32),
    /// 没有"本端口的控制台"的运行态文件 → 端口多半是别的程序占的
    NotOurs,
}

/// 端口被占用时的处置：唤醒已有实例，或按原因给出可操作提示后退出（本函数不返回）
///
/// 两种失败原因必须分开（探针据此判定"实例被拒"而不是"端口被旁人占了"）：
/// * 运行态文件读不到 / 端口对不上 → 占用端口的是**别的程序**（`--port` 换端口）；
/// * 运行态文件在且指向本端口，但连不上/不应答 → **已有控制台但无法唤醒**（结束它再启动）。
fn port_busy_exit(args: &Args) -> ! {
    // 本函数在 main 的 `i18n::init` **之前**就会被调用（端口在 main 最开头就绑），
    // 而消息框文案要按当前语言取 → 先懒初始化一次（i18n::ensure 幂等）。
    i18n::ensure(&args.root);
    let port = args.port.to_string();
    // 两层文案，别混：
    //  * `line`（stdout）= **英文固定串** + 退出码 —— scripts/tests/*.py 靠它们判定
    //    "端口被谁占着"（见 gui/README.md 的退出码约定），跟着界面语言变会让判据漂移；
    //  * `hint`（消息框）= 给人看的，走 i18n。
    let (code, line, hint) = match wake_existing(&args.root, args.port) {
        Wake::Woke(pid) => (
            0,
            format!(
                "wp8f console already running: brought its window up (pid={pid}), this process exits"
            ),
            None,
        ),
        Wake::Stuck(pid) => {
            let pid_s = pid.to_string();
            (
                EXIT_HOST_STUCK,
                format!(
                    "an existing wp8f console (pid={pid}, port {port}) does not respond: could not be woken up"
                ),
                Some(i18n::tf("gui.dlg.port_stuck.body", &[("port", &port), ("pid", &pid_s)])),
            )
        }
        Wake::NotOurs => (
            EXIT_PORT_TAKEN,
            format!(
                "port {port} is occupied by another program (not a wp8f console), this process exits"
            ),
            Some(i18n::tf("gui.dlg.port_taken.body", &[("port", &port)])),
        ),
    };
    say(&line);
    // --no-window（自检/CI）下不弹模态框：无人值守时没人点它，会把脚本挂住
    if let Some(text) = hint {
        if !args.no_window {
            message_box(i18n::t("gui.window.title"), &text);
        }
    }
    std::process::exit(code);
}

/// 已有实例：读运行态文件，让它的窗口显示出来
///
/// `port` 是本次要绑的 HTTP 端口：只有运行态文件里记的端口就是它，才能断定
/// "占住端口的是控制台"；文件缺失/损坏/端口对不上时只能认为端口被别的程序占了。
fn wake_existing(root: &std::path::Path, port: u16) -> Wake {
    use std::io::{Read, Write};
    let Ok(txt) = std::fs::read_to_string(root.join("logs").join("console-host.json")) else {
        return Wake::NotOurs;
    };
    let (Some(url), Some(tok)) =
        (extract_json_str(&txt, "control_url"), extract_json_str(&txt, "token"))
    else {
        return Wake::NotOurs;
    };
    let pid = extract_json_u32(&txt, "pid").unwrap_or(0);
    // 运行态记的是别的端口：那不是本端口的服务者（并行实例），
    // 别拿它去解释"本端口被占"这件事
    if let Some(p) = extract_json_u32(&txt, "port") {
        if p != u32::from(port) {
            return Wake::NotOurs;
        }
    }
    let addr = url.trim_start_matches("http://").to_string();
    let Ok(mut s) = std::net::TcpStream::connect(&addr) else {
        return Wake::Stuck(pid);
    };
    // 卡死的控制台会"接受连接但永不回包"：没有超时的话第二个实例跟着一起挂住，
    // 也就永远走不到"无法唤醒"这条提示
    let to = Some(std::time::Duration::from_secs(3));
    let _ = s.set_read_timeout(to);
    let _ = s.set_write_timeout(to);
    let req = format!(
        "POST /control/window/show HTTP/1.1\r\nHost: {}\r\nX-WP8F-Token: {}\r\n\
         Content-Length: 0\r\nConnection: close\r\n\r\n",
        addr, tok
    );
    if s.write_all(req.as_bytes()).is_err() {
        return Wake::Stuck(pid);
    }
    let _ = s.flush();
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    if text.starts_with("HTTP/1.1 200") || text.contains("\"ok\":true") {
        Wake::Woke(pid)
    } else {
        Wake::Stuck(pid)
    }
}

fn extract_json_str(txt: &str, key: &str) -> Option<String> {
    let pat = format!("\"{}\":\"", key);
    let start = txt.find(&pat)? + pat.len();
    let end = txt[start..].find('"')? + start;
    Some(txt[start..end].to_string())
}

/// 取整数成员（运行态文件是本进程写的扁平 JSON：`{"pid":1,"port":8765,...}`）
fn extract_json_u32(txt: &str, key: &str) -> Option<u32> {
    let pat = format!("\"{key}\":");
    let start = txt.find(&pat)? + pat.len();
    let digits: String = txt[start..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Win11 默认把新托盘图标折叠进 ^ 溢出区：把本 exe 的条目置为「直接显示」
pub fn promote_tray_icon() -> Result<bool, String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
        HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_DWORD,
    };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_l = exe.to_string_lossy().to_lowercase();
    let base = win::wide("Control Panel\\NotifyIconSettings");
    unsafe {
        let mut hkey = std::mem::zeroed();
        if RegOpenKeyExW(HKEY_CURRENT_USER, base.as_ptr(), 0, KEY_READ, &mut hkey) != 0 {
            return Ok(false); // 条目还没被 shell 创建（首次运行）
        }
        let mut changed = false;
        let mut idx = 0u32;
        loop {
            let mut name = [0u16; 512];
            let mut name_len = name.len() as u32;
            if RegEnumKeyExW(hkey, idx, name.as_mut_ptr(), &mut name_len, std::ptr::null_mut(),
                             std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) != 0
            {
                break;
            }
            idx += 1;
            let sub = String::from_utf16_lossy(&name[..name_len as usize]);
            let mut skey = std::mem::zeroed();
            let path = win::wide(&format!("Control Panel\\NotifyIconSettings\\{sub}"));
            if RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ | KEY_SET_VALUE,
                             &mut skey) != 0
            {
                continue;
            }
            let val_name = win::wide("ExecutablePath");
            let mut ty = 0u32;
            let mut buf = [0u8; 1024];
            let mut size = buf.len() as u32;
            let ok = RegQueryValueExW(skey, val_name.as_ptr(), std::ptr::null_mut(), &mut ty,
                                      buf.as_mut_ptr(), &mut size) == 0;
            if ok {
                let raw: Vec<u16> = buf[..size as usize]
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|&c| c != 0)
                    .collect();
                if String::from_utf16_lossy(&raw).to_lowercase() == exe_l {
                    let promoted_name = win::wide("IsPromoted");
                    let mut cur = 0u32;
                    let mut cur_size = 4u32;
                    let has = RegQueryValueExW(skey, promoted_name.as_ptr(), std::ptr::null_mut(),
                                               &mut ty, &mut cur as *mut u32 as *mut u8,
                                               &mut cur_size) == 0;
                    if !has || cur != 1 {
                        let one: u32 = 1;
                        if RegSetValueExW(skey, promoted_name.as_ptr(), 0, REG_DWORD,
                                          &one as *const u32 as *const u8, 4) == 0
                        {
                            changed = true;
                        }
                    }
                }
            }
            RegCloseKey(skey);
        }
        RegCloseKey(hkey);
        Ok(changed)
    }
}
