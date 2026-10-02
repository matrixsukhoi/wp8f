//! 系统托盘：隐藏窗口 + Shell_NotifyIcon + 右键菜单（左键单击 = 显示主窗口）。
//!
//! 合并成单进程后不再自带消息循环 —— 托盘窗口的消息由 tao 的事件循环统一分发
//! （同线程上所有窗口都会被 DispatchMessage 派发），回调里通过事件代理把动作
//! 投回事件循环执行。
use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    GetCursorPos, LoadIconW, LoadImageW, PostMessageW, RegisterClassW, SetForegroundWindow,
    TrackPopupMenu, CW_USEDEFAULT, HMENU, IMAGE_ICON, LR_LOADFROMFILE, MF_STRING, TPM_BOTTOMALIGN,
    TPM_RETURNCMD, TPM_RIGHTALIGN, TPM_RIGHTBUTTON, WM_APP, WM_COMMAND, WM_DESTROY,
    WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
};

use crate::win::wide;

const TRAY_UID: u32 = 1;
const WM_TRAY: u32 = WM_APP + 1;
const CMD_SHOW: usize = 1001;
const CMD_EXIT: usize = 1002;
pub const WINDOW_CLASS: &str = "wp8f_gui_tray_wnd";

/// 托盘回调：由 main 注入（内部把动作投回事件循环）
pub struct Actions {
    pub on_show: Box<dyn Fn() + Send + Sync + 'static>,
    pub on_exit: Box<dyn Fn() + Send + Sync + 'static>,
}

struct State {
    actions: Actions,
}

static mut STATE: Option<Box<State>> = None;

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let state = &*std::ptr::addr_of!(STATE);
    match msg {
        WM_TRAY => {
            if let Some(state) = state.as_ref() {
                match lparam as u32 {
                    WM_LBUTTONUP => (state.actions.on_show)(),
                    WM_RBUTTONUP => show_menu(hwnd),
                    _ => {}
                }
            }
            0
        }
        WM_COMMAND => {
            if let Some(state) = state.as_ref() {
                match wparam & 0xFFFF {
                    CMD_SHOW => (state.actions.on_show)(),
                    CMD_EXIT => (state.actions.on_exit)(),
                    _ => {}
                }
            }
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn show_menu(hwnd: HWND) {
    unsafe {
        let menu: HMENU = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        // 菜单项在**右键这一刻**才取文案：i18n 表在启动时就绪（main 里 init 先于 tray::install），
        // 之后用户改了语言，下一次右键就是新语言，不必重建托盘。
        AppendMenuW(menu, MF_STRING, CMD_SHOW, wide(crate::i18n::t("gui.tray.show")).as_ptr());
        AppendMenuW(menu, MF_STRING, CMD_EXIT, wide(crate::i18n::t("gui.tray.exit")).as_ptr());
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        // 菜单要能正常消失：先把自己设为前台窗口，菜单后再补一条消息（MSDN 经典做法）
        SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_RIGHTALIGN,
            pt.x,
            pt.y,
            0,
            hwnd,
            std::ptr::null(),
        );
        PostMessageW(hwnd, 0, 0, 0);
        DestroyMenu(menu);
        if cmd != 0 {
            PostMessageW(hwnd, WM_COMMAND, cmd as usize, 0);
        }
    }
}

fn icon_handle() -> windows_sys::Win32::UI::WindowsAndMessaging::HICON {
    /// 与 `gui/icon.rc` 里的资源号一致（构建时由 windres 编进 exe）
    const ICON_RESOURCE_ID: usize = 1;

    // 首选：exe 里内嵌的资源图标（构建脚本编进去的），不碰磁盘
    let hinst: HINSTANCE = unsafe { GetModuleHandleW(std::ptr::null()) };
    let hicon = unsafe { LoadIconW(hinst, ICON_RESOURCE_ID as *const u16) };
    if !hicon.is_null() {
        return hicon;
    }
    // 退回：构建环境没有 windres 时 exe 里没有资源 —— 内嵌副本落临时文件再 LoadImageW
    // （比手工解析 ICONRESOURCE 稳）；句柄拿到后立刻删文件，否则每启动一次就在 %TEMP%
    // 留一个 wp8f-gui-<pid>.ico。
    let bytes: &[u8] = include_bytes!("../logo.ico");
    let path = std::env::temp_dir().join(format!("wp8f-gui-{}.ico", std::process::id()));
    if std::fs::write(&path, bytes).is_err() {
        return std::ptr::null_mut();
    }
    let p = wide(&path.to_string_lossy());
    let hicon: windows_sys::Win32::UI::WindowsAndMessaging::HICON =
        unsafe { LoadImageW(std::ptr::null_mut(), p.as_ptr(), IMAGE_ICON, 0, 0, LR_LOADFROMFILE) as _ };
    let _ = std::fs::remove_file(&path);
    hicon
}

/// 注册托盘图标（隐藏窗口 + Shell_NotifyIcon）。不跑消息循环：消息由 tao 分发。
pub fn install(actions: Actions, tooltip: &str, promote: bool) -> bool {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(std::ptr::null());
        let class_name = wide(WINDOW_CLASS);
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wnd_proc);
        wc.hInstance = hinst;
        wc.lpszClassName = class_name.as_ptr();
        RegisterClassW(&wc); // 已注册会失败，忽略

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            wide("wp8f-gui").as_ptr(),
            WS_OVERLAPPED,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return false;
        }

        let tip = wide(tooltip);
        let hicon = icon_handle();
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_UID;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = hicon;
        let n = tip.len().min(nid.szTip.len());
        nid.szTip[..n].copy_from_slice(&tip[..n]);

        STATE = Some(Box::new(State { actions }));
        if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
            STATE = None;
            return false;
        }
        // Win11 默认把新图标折叠进 ^ 溢出区：置 IsPromoted=1（首次运行需再注册一次）
        if promote && crate::promote_tray_icon().unwrap_or(false) {
            Shell_NotifyIconW(NIM_DELETE, &nid);
            std::thread::sleep(std::time::Duration::from_millis(120));
            Shell_NotifyIconW(NIM_ADD, &nid);
        }
        true
    }
}

/// 退出前移除图标并销毁托盘窗口
pub fn uninstall() {
    unsafe {
        let class_name = wide(WINDOW_CLASS);
        let hwnd = windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(
            class_name.as_ptr(),
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_UID;
        Shell_NotifyIconW(NIM_DELETE, &nid);
        DestroyWindow(hwnd);
    }
}
