//! Win32 底层封装：进程枚举/探活/结束、作业对象、子进程启动、隐藏窗口辅助。

use std::ffi::c_void;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

/// WaitForSingleObject 需要它才有等待权限（windows-sys 未在 Foundation 导出）
const SYNCHRONIZE: u32 = 0x0010_0000;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
    JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_ALWAYS,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, OpenProcess, TerminateProcess, WaitForSingleObject, CREATE_NEW_CONSOLE,
    CREATE_NO_WINDOW,
    PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
    PROCESS_TERMINATE, STARTF_USESTDHANDLES, STARTUPINFOW,
};

/// UTF-8 → 以 NUL 结尾的宽字符串
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 进程名（不含路径）小写比较（PROCESSENTRY32W.szExeFile 是宽字符数组）
fn exe_name_matches(entry: &PROCESSENTRY32W, exe_name: &str) -> bool {
    let mut buf = String::new();
    for &c in entry.szExeFile.iter() {
        if c == 0 {
            break;
        }
        if let Some(ch) = char::from_u32(c as u32) {
            buf.push(ch);
        }
    }
    buf.eq_ignore_ascii_case(exe_name)
}

/// 按映像名枚举 pid（替代 tasklist 调用：更快、无控制台闪烁）
pub fn pids_of(exe_name: &str) -> Vec<u32> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            if exe_name_matches(&entry, exe_name) {
                out.push(entry.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    out
}

pub fn is_running(exe_name: &str) -> bool {
    !pids_of(exe_name).is_empty()
}

/// 进程是否还活着（句柄等待 0 超时，比 OpenProcess 成功更可靠）
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe {
        // 注意：WaitForSingleObject 需要 SYNCHRONIZE 权限，只给 QUERY_LIMITED_INFORMATION
        // 会返回 WAIT_FAILED（=-1），被误判成"进程已退出"
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid);
        if h.is_null() {
            // 权限不足或进程已退出 —— 用进程快照兜底判断
            return snapshot_has(pid);
        }
        let r = WaitForSingleObject(h, 0);
        CloseHandle(h);
        r == WAIT_TIMEOUT
    }
}

fn snapshot_has(pid: u32) -> bool {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        let mut found = false;
        while ok {
            if entry.th32ProcessID == pid {
                found = true;
                break;
            }
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        found
    }
}

pub fn kill_pid(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if h.is_null() {
            return false;
        }
        let ok = TerminateProcess(h, 1) != 0;
        CloseHandle(h);
        ok
    }
}

/// kill-on-close 作业对象：常驻进程无论怎么死（含 taskkill /F），子进程一起结束
pub struct Job(HANDLE);

unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    pub fn create() -> Option<Job> {
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                CloseHandle(h);
                return None;
            }
            Some(Job(h))
        }
    }

    pub fn assign(&self, pid: u32) -> bool {
        unsafe {
            let h = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if h.is_null() {
                return false;
            }
            let ok = AssignProcessToJobObject(self.0, h) != 0;
            CloseHandle(h);
            ok
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// 启动子进程，返回 pid。
///
/// * `log_path` 非空 → 子进程 stdout/stderr 落盘（无控制台窗口，`CREATE_NO_WINDOW`）；
/// * `new_console = true` → **给它一个自己的控制台窗口**（`CREATE_NEW_CONSOLE`，日志不落盘，
///   输出直接打在窗口里；给用户"带命令行启动"用）。
pub fn spawn(
    cmd: &[String],
    cwd: Option<&str>,
    log_path: Option<&std::path::Path>,
    new_console: bool,
) -> Result<u32, String> {
    if cmd.is_empty() {
        return Err("空命令".into());
    }
    let mut cmdline = String::new();
    for (i, a) in cmd.iter().enumerate() {
        if i > 0 {
            cmdline.push(' ');
        }
        // CreateProcessW 要求可执行文件路径带引号（路径可能含空格）
        if i == 0 || a.contains(' ') {
            cmdline.push('"');
            cmdline.push_str(&a.replace('"', "\\\""));
            cmdline.push('"');
        } else {
            cmdline.push_str(a);
        }
    }
    let mut cl = wide(&cmdline);
    let cwd_w = cwd.map(wide);
    // 子进程日志句柄。**必须可继承**：默认 CreateFileW 句柄不可继承，
    // 子进程拿到无效 stdout 后 print() 会抛 OSError 直接死（日志还是空的，极难查）。
    let mut log_handle: HANDLE = std::ptr::null_mut();
    // stdio 兜底句柄（NUL）。日志打不开时 stdout/stderr 也必须有**有效**句柄，
    // 所以它要同时可读可写：只给 GENERIC_READ 的话，往 stdout 写会 ERROR_ACCESS_DENIED，
    // 子进程照样死 —— 正是上面注释警告的那种失败。
    let nul_handle: HANDLE;
    unsafe {
        let mut sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        let nul = wide("NUL");
        let h = CreateFileW(
            nul.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &mut sa,
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        nul_handle = if h == INVALID_HANDLE_VALUE { std::ptr::null_mut() } else { h };
        if let Some(p) = log_path {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let path_w = wide(&p.to_string_lossy());
            log_handle = CreateFileW(
                path_w.as_ptr(),
                FILE_APPEND_DATA,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &mut sa,
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            );
            if log_handle == INVALID_HANDLE_VALUE {
                log_handle = std::ptr::null_mut();
            }
            if log_handle.is_null() {
                // 没有日志文件，退化为 NUL：子进程至少不会因为 stdio 无效而立刻退出
                crate::log(&format!(
                    "子进程日志打开失败（{}）：stdout/stderr 改为 NUL，本次没有子进程日志",
                    p.display()
                ));
            }
        }
    }
    unsafe {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        // 带命令行启动：**不重定向** stdio，让子进程自己的控制台窗口显示输出
        if new_console {
            si.dwFlags = 0;
        } else if !nul_handle.is_null() {
            si.dwFlags = STARTF_USESTDHANDLES;
            si.hStdInput = nul_handle;
            si.hStdOutput = if log_handle.is_null() { nul_handle } else { log_handle };
            si.hStdError = si.hStdOutput;
        }
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let ok = CreateProcessW(
            std::ptr::null(),
            cl.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            if new_console || (log_handle.is_null() && nul_handle.is_null()) { 0 } else { 1 },
            if new_console { CREATE_NEW_CONSOLE } else { CREATE_NO_WINDOW },
            std::ptr::null(),
            cwd_w.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            &si,
            &mut pi,
        );
        if !log_handle.is_null() {
            CloseHandle(log_handle);
        }
        if !nul_handle.is_null() {
            CloseHandle(nul_handle);
        }
        if ok == 0 {
            return Err(format!("CreateProcessW 失败（err={}）: {}", GetLastError(), cmdline));
        }
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        Ok(pi.dwProcessId)
    }
}
