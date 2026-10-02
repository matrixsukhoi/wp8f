//! 更新器的网络层：**不引入新的 cargo 依赖**（`--offline` 构建、也不想把 TLS 栈拖进 HUD）。
//!
//! * `http://` → 自己用 `std::net::TcpStream` 发一个极小的 HTTP/1.1 GET
//!   （支持 `Range:` 续传、跟随 3xx 跳转、读超时可取消）。本地假源（离线单测）走的就是这条；
//! * `https://` → 调系统自带的 `curl`（Windows 10+ 的 `System32\curl.exe`、Linux 的 `curl`），
//!   带 `--fail`（HTTP ≥400 报错）+ `-C -`（按目标文件大小续传）+ 子进程可被取消时 kill。
//!
//! 错误一律分类成 [`ErrorKind`]：网络不可用 / 校验失败 / 磁盘空间不足 / 权限不足 /
//! 本地 IO / 已取消 —— 日志与界面文案都按这个分类走（`[UPDATE] 网络不可用：…`）。

use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// `curl` 子进程的 UA（有些 CDN 对空 UA 不友好；也便于在服务端日志里认出 wp8f）。
pub const USER_AGENT: &str = "wp8f-updater/0.1 (+local use only)";
/// 连接超时（DNS/握手）
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// 单次请求的整体上限（curl 的 `--max-time` / 自实现客户端的读循环上限）
const MAX_REQUEST_TIME: Duration = Duration::from_secs(300);
/// 自实现客户端的读超时（用来周期性检查取消标志）
const READ_POLL: Duration = Duration::from_millis(500);
/// 跟随跳转的最大次数
const MAX_REDIRECTS: usize = 5;

/// 错误分类（界面文案与 `[UPDATE]` 日志的第一段就是它）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// 网络不可用（DNS/连接被重置/超时/HTTP 错误/限流）
    Network,
    /// 下载下来的内容校验不过（大小不符、JSON 解析失败、版本号对不上）
    Verify,
    /// 磁盘空间不足（ENOSPC / ERROR_DISK_FULL）
    DiskFull,
    /// 权限不足（EACCES / ERROR_ACCESS_DENIED，Windows 上"目录被占用"也常报这个）
    Permission,
    /// 其它本地 IO 错误
    Io,
    /// 用户取消（可续传，不算失败）
    Cancelled,
    /// 远端版本号/清单格式不认识
    Protocol,
    /// **HUD 正在运行**：拒绝切换数据（Windows 上被占用的目录改不了名，
    /// 而且切完 HUD 也读不到新数据 —— 用户要求"先关掉再更新"，更新器不替用户杀进程）。
    HudRunning,
}

impl ErrorKind {
    /// 分类的机器名（前端据它选 i18n 文案键；日志里也用）。
    pub fn id(self) -> &'static str {
        match self {
            ErrorKind::Network => "network",
            ErrorKind::Verify => "verify",
            ErrorKind::DiskFull => "disk",
            ErrorKind::Permission => "permission",
            ErrorKind::Io => "io",
            ErrorKind::Cancelled => "cancelled",
            ErrorKind::Protocol => "protocol",
            ErrorKind::HudRunning => "hud_running",
        }
    }

    /// 日志前缀（`[UPDATE] <这个>：<详情>`）。
    pub fn label(self) -> &'static str {
        match self {
            ErrorKind::Network => "网络不可用",
            ErrorKind::Verify => "校验失败",
            ErrorKind::DiskFull => "磁盘空间不足",
            ErrorKind::Permission => "权限不足",
            ErrorKind::Io => "本地 IO 错误",
            ErrorKind::Cancelled => "已取消",
            ErrorKind::Protocol => "远端数据格式不认识",
            ErrorKind::HudRunning => "HUD 正在运行",
        }
    }
}

/// 更新器的统一错误（分类 + 可读详情）。
#[derive(Debug, Clone)]
pub struct UpdateError {
    pub kind: ErrorKind,
    pub message: String,
    /// **主机级**失败：域名解析不了 / 连不上 / 被重置 / 超时。
    /// 只有这类失败才会让下载器把一个源整轮停用 —— HTTP 404/403（"主机是活的"）
    /// 不计入，否则上游删掉一个文件就能把健康的源一起拖下水。
    pub host_unreachable: bool,
}

impl UpdateError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), host_unreachable: false }
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Network, message)
    }

    /// 主机级网络失败（连不上/被重置/超时）。
    pub fn unreachable(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Network, message: message.into(), host_unreachable: true }
    }

    pub fn verify(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Verify, message)
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Io, message)
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Protocol, message)
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorKind::Cancelled, "用户取消了本次更新")
    }

    /// `[UPDATE] …` 那一行。
    pub fn log_line(&self) -> String {
        format!("[UPDATE] {}：{}", self.kind.label(), self.message)
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}：{}", self.kind.label(), self.message)
    }
}

impl std::error::Error for UpdateError {}

/// 把 `std::io::Error` 分类：磁盘满 / 权限 / 其它。
///
/// 用 `raw_os_error()` 的裸错误码而不是 `ErrorKind::StorageFull`：后者在稳定版
/// Rust 里还没有（需要 nightly feature），而裸码两条平台都稳定：
/// Linux `ENOSPC=28`、`EACCES=13`、`EPERM=1`；Windows `ERROR_DISK_FULL=112`、
/// `ERROR_HANDLE_DISK_FULL=39`、`ERROR_ACCESS_DENIED=5`、`ERROR_SHARING_VIOLATION=32`。
pub fn classify_io(e: &io::Error, what: &str) -> UpdateError {
    let code = e.raw_os_error();
    let kind = match code {
        Some(28) | Some(112) | Some(39) => ErrorKind::DiskFull,
        Some(13) | Some(1) | Some(5) | Some(32) => ErrorKind::Permission,
        _ => match e.kind() {
            io::ErrorKind::PermissionDenied => ErrorKind::Permission,
            io::ErrorKind::NotFound => ErrorKind::Io,
            _ => ErrorKind::Io,
        },
    };
    UpdateError::new(kind, format!("{what}：{e}"))
}

/// 取消标志（GUI 的「取消」按钮把 `AtomicBool` 置真，worker/子进程读它）。
pub trait Canceller: Send + Sync {
    fn cancelled(&self) -> bool;
}

impl Canceller for std::sync::atomic::AtomicBool {
    fn cancelled(&self) -> bool {
        self.load(Ordering::Relaxed)
    }
}

/// 永不取消（一次性调用、单测里的简单路径）。
pub struct NeverCancel;

impl Canceller for NeverCancel {
    fn cancelled(&self) -> bool {
        false
    }
}

/// 一次下载请求。
pub struct FetchSpec<'a> {
    pub url: &'a str,
    /// 目标文件；**调用方负责续传语义**（`range_from > 0` 时这个文件里已有前 `range_from` 字节）
    pub dest: &'a Path,
    /// 从该偏移开始（0 = 从头下）
    pub range_from: u64,
    /// 附加 `Accept:`（`api.github.com` 的 raw 输出要 `application/vnd.github.raw`）
    pub accept: Option<&'a str>,
}

/// 传输实现（生产 = [`HttpTransport`]，测试 = 同一个实现 + 本地假源）。
pub trait Transport: Send + Sync {
    /// 拉一次，返回**本次新增**的字节数（续传时不是整个文件大小）。
    fn fetch(&self, spec: &FetchSpec<'_>, cancel: &dyn Canceller) -> Result<u64, UpdateError>;

    /// 实现名（日志用）。
    fn name(&self) -> &'static str {
        "http"
    }
}

/// 生产实现：`http://` 走内置客户端，`https://` 走系统 `curl`。
#[derive(Debug, Default)]
pub struct HttpTransport {
    /// 强制用 `curl` 处理 `http://`（单测用来覆盖 curl 那条分支；`None` = 按协议自动选）
    pub force_curl: bool,
}

impl HttpTransport {
    pub fn new() -> Self {
        Self { force_curl: false }
    }

    /// 读环境变量 `WP8F_UPDATE_HTTP_IMPL=curl|std`（调试/对照用）。
    pub fn from_env() -> Self {
        let force_curl = std::env::var("WP8F_UPDATE_HTTP_IMPL")
            .map(|v| v.eq_ignore_ascii_case("curl"))
            .unwrap_or(false);
        Self { force_curl }
    }
}

impl Transport for HttpTransport {
    fn fetch(&self, spec: &FetchSpec<'_>, cancel: &dyn Canceller) -> Result<u64, UpdateError> {
        let https = spec.url.starts_with("https://");
        if https || self.force_curl {
            if https && which_curl().is_none() {
                return Err(UpdateError::network(format!(
                    "需要 https 但系统里找不到 curl（Windows 10+ 自带 System32\\curl.exe；\
                     Linux 需安装 curl）：{}",
                    spec.url
                )));
            }
            curl_get(spec, cancel)
        } else {
            std_get(spec, cancel)
        }
    }

    fn name(&self) -> &'static str {
        "curl/std"
    }
}

/// `curl` 可执行文件是否存在（Windows 上就是 `where curl`）。
pub fn which_curl() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "curl.exe" } else { "curl" };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(exe))
        .find(|p| p.is_file())
}

// ---------------------------------------------------------------- curl 分支 --

/// `curl` 退出码 → 分类（见 curl 手册 EXIT CODES）。
fn curl_error(code: i32, stderr: &str, url: &str) -> UpdateError {
    let tail = stderr.trim().lines().last().unwrap_or("").to_string();
    let full = format!("curl 退出码 {code}（{url}）{}{}", if tail.is_empty() { "" } else { "：" }, tail);
    match code {
        // 6 DNS / 7 连接失败 / 28 超时 / 35 SSL / 52 空回复 / 55/56 收发失败 → 主机级
        6 | 7 | 28 | 35 | 52 | 55 | 56 => UpdateError::unreachable(full),
        // 22 = --fail 看到 HTTP ≥400（限流/404 都在这里）——主机是活的
        22 => UpdateError::network(format!("HTTP 错误（{full}）")),
        // 33 = 服务端不支持 Range（断点续传不可用）
        33 => UpdateError::network(format!("服务端不支持断点续传（{full}）")),
        // 23 = 写文件失败（磁盘满/权限都会被归到这里，只能看 stderr 文本）
        23 => {
            let low = stderr.to_lowercase();
            if low.contains("no space") || low.contains("disk full") {
                UpdateError::new(ErrorKind::DiskFull, full)
            } else if low.contains("permission") || low.contains("denied") {
                UpdateError::new(ErrorKind::Permission, full)
            } else {
                UpdateError::io(full)
            }
        }
        _ => UpdateError::network(full),
    }
}

/// Windows GUI 子系统进程里跑控制台子进程必须显式 `CREATE_NO_WINDOW`，
/// 否则每次下载都会闪一个黑框（与 `gui/src/env.rs` 调 flightmodel 同一口径）。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn curl_get(spec: &FetchSpec<'_>, cancel: &dyn Canceller) -> Result<u64, UpdateError> {
    let before = fs::metadata(spec.dest).map(|m| m.len()).unwrap_or(0);
    if spec.range_from == 0 && before > 0 {
        // 从头下就必须先把旧内容清掉（curl 的 `-C -` 会按文件大小续传）
        fs::remove_file(spec.dest).map_err(|e| classify_io(&e, "删除旧的部分文件失败"))?;
    }
    let mut cmd = std::process::Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    cmd.arg("-sS")
        .arg("-L")
        .arg("--fail")
        .arg("--no-progress-meter")
        .arg("--connect-timeout")
        .arg(CONNECT_TIMEOUT.as_secs().to_string())
        .arg("--max-time")
        .arg(MAX_REQUEST_TIME.as_secs().to_string())
        .arg("--retry")
        .arg("0")
        .arg("-A")
        .arg(USER_AGENT);
    if let Some(accept) = spec.accept {
        cmd.arg("-H").arg(format!("Accept: {accept}"));
    }
    if spec.range_from > 0 {
        cmd.arg("-C").arg("-");
    }
    cmd.arg("-o").arg(spec.dest).arg(spec.url);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| classify_io(&e, "启动 curl 失败（系统里没有 curl？）"))?;
    let deadline = Instant::now() + MAX_REQUEST_TIME + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut err_text = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err_text);
                }
                if !status.success() {
                    return Err(curl_error(status.code().unwrap_or(-1), &err_text, spec.url));
                }
                let after = fs::metadata(spec.dest).map(|m| m.len()).unwrap_or(0);
                return Ok(after.saturating_sub(before));
            }
            Ok(None) => {}
            Err(e) => return Err(classify_io(&e, "等待 curl 失败")),
        }
        if cancel.cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(UpdateError::cancelled());
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(UpdateError::network(format!("curl 超过 {:?} 仍未结束", MAX_REQUEST_TIME)));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ------------------------------------------------------- 内置 http:// 客户端 --

struct Url {
    host: String,
    port: u16,
    path: String,
}

fn parse_http_url(url: &str) -> Result<Url, UpdateError> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| UpdateError::network(format!("只支持 http:// 的内置客户端：{url}")))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|_| UpdateError::network(format!("端口非法：{url}")))?,
        ),
        None => (authority.to_string(), 80),
    };
    if host.is_empty() {
        return Err(UpdateError::network(format!("主机为空：{url}")));
    }
    Ok(Url { host, port, path: path.to_string() })
}

/// 读 HTTP 响应头（逐字节读到 `\r\n\r\n`，避免把正文读进缓冲）。
fn read_head(stream: &mut TcpStream, cancel: &dyn Canceller) -> Result<String, UpdateError> {
    let mut buf: Vec<u8> = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match stream.read(&mut byte) {
            Ok(0) => {
                if buf.is_empty() {
                    return Err(UpdateError::unreachable("服务端没有返回任何数据（连接被关闭）"));
                }
                break;
            }
            Ok(_) => {
                buf.push(byte[0]);
                if buf.len() >= 4 && &buf[buf.len() - 4..] == b"\r\n\r\n" {
                    break;
                }
                if buf.len() > 64 * 1024 {
                    return Err(UpdateError::network("响应头过大（>64KB）"));
                }
            }
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                if cancel.cancelled() {
                    return Err(UpdateError::cancelled());
                }
                if Instant::now() > deadline {
                    return Err(UpdateError::unreachable("读响应头超时"));
                }
            }
            Err(e) => return Err(classify_io(&e, "读响应头失败")),
        }
    }
    String::from_utf8(buf).map_err(|_| UpdateError::network("响应头不是合法 UTF-8"))
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .skip(1)
        .find(|l| {
            l.split_once(':')
                .map(|(k, _)| k.trim().eq_ignore_ascii_case(name))
                .unwrap_or(false)
        })
        .and_then(|l| l.split_once(':').map(|(_, v)| v.trim()))
}

/// 内置 `http://` GET：支持 `Range` 续传、跟随 3xx、读循环里可取消。
fn std_get(spec: &FetchSpec<'_>, cancel: &dyn Canceller) -> Result<u64, UpdateError> {
    let mut url = spec.url.to_string();
    let range_from = spec.range_from;
    if spec.range_from == 0 && spec.dest.exists() {
        fs::remove_file(spec.dest).map_err(|e| classify_io(&e, "删除旧的部分文件失败"))?;
    }
    for _hop in 0..=MAX_REDIRECTS {
        let parsed = parse_http_url(&url)?;
        let addr = (parsed.host.as_str(), parsed.port)
            .to_socket_addrs()
            .map_err(|e| UpdateError::unreachable(format!("解析主机失败（{}）：{e}", parsed.host)))?
            .next()
            .ok_or_else(|| UpdateError::unreachable(format!("主机解析不到地址：{}", parsed.host)))?;
        let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
            .map_err(|e| UpdateError::unreachable(format!("连接 {addr} 失败：{e}")))?;
        stream
            .set_read_timeout(Some(READ_POLL))
            .map_err(|e| classify_io(&e, "设置读超时失败"))?;
        let mut req = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept-Encoding: identity\r\n\
             Connection: close\r\n",
            parsed.path, parsed.host, USER_AGENT
        );
        if let Some(accept) = spec.accept {
            req.push_str(&format!("Accept: {accept}\r\n"));
        }
        if range_from > 0 {
            req.push_str(&format!("Range: bytes={range_from}-\r\n"));
        }
        req.push_str("\r\n");
        stream
            .write_all(req.as_bytes())
            .map_err(|e| UpdateError::network(format!("写请求失败：{e}")))?;

        let head = read_head(&mut stream, cancel)?;
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| UpdateError::network(format!("响应状态行看不懂：{}", head.lines().next().unwrap_or(""))))?;

        if (300..400).contains(&status) {
            let loc = header_value(&head, "location")
                .ok_or_else(|| UpdateError::network(format!("HTTP {status} 但没有 Location 头")))?;
            url = if loc.starts_with("http://") {
                loc.to_string()
            } else {
                format!("http://{}:{}{}", parsed.host, parsed.port, loc)
            };
            continue;
        }
        if status == 416 && range_from > 0 {
            // 已经下完了（Range 越界）：当成"没有新增字节"
            return Ok(0);
        }
        if status != 200 && status != 206 {
            return Err(UpdateError::network(format!("HTTP {status}（{url}）")));
        }
        if status == 200 && range_from > 0 {
            // 服务端忽略了 Range：只能从头重来（调用方会重建 .part）
            return Err(UpdateError::network(format!(
                "服务端不支持断点续传（Range 被忽略，返回 200）：{url}"
            )));
        }
        if status == 206 {
            if let Some(cr) = header_value(&head, "content-range") {
                let start = cr
                    .rsplit_once('=')
                    .map(|(_, r)| r)
                    .unwrap_or(cr)
                    .split('-')
                    .next()
                    .and_then(|s| s.trim().parse::<u64>().ok());
                if let Some(start) = start {
                    if start != range_from {
                        return Err(UpdateError::network(format!(
                            "服务端返回的 Content-Range 起点是 {start}，期望 {range_from}"
                        )));
                    }
                }
            }
        }

        // 正文：追加（206）或新建（200）
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(spec.dest)
            .map_err(|e| classify_io(&e, &format!("打开 {} 失败", spec.dest.display())))?;
        let mut written = 0u64;
        let mut buf = vec![0u8; 64 * 1024];
        let deadline = Instant::now() + MAX_REQUEST_TIME;
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    file.write_all(&buf[..n])
                        .map_err(|e| classify_io(&e, "写入下载文件失败"))?;
                    written += n as u64;
                }
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    if cancel.cancelled() {
                        return Err(UpdateError::cancelled());
                    }
                    if Instant::now() > deadline {
                        return Err(UpdateError::unreachable("下载超时（300s 没有读完）"));
                    }
                }
                Err(e) => return Err(UpdateError::network(format!("读正文失败：{e}"))),
            }
        }
        file.flush().map_err(|e| classify_io(&e, "刷新下载文件失败"))?;
        return Ok(written);
    }
    Err(UpdateError::network(format!("跳转次数超过 {MAX_REDIRECTS}：{url}")))
}

// ------------------------------------------------------------------ 小工具 --

static TMP_SEQ: AtomicUsize = AtomicUsize::new(0);

/// 一个不会撞车的临时文件名（同进程内计数 + pid）。
pub fn unique_tmp_path(dir: &Path, tag: &str) -> PathBuf {
    let n = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("wp8f_{tag}_{}_{n}.tmp", std::process::id()))
}

/// 取一个小文本文件（版本号、清单 JSON 都走它）：下到临时文件再读，读完删掉。
///
/// `max_bytes` 是硬上限（防止把几百 MB 的响应读进内存）。
pub fn get_text(
    transport: &dyn Transport,
    url: &str,
    accept: Option<&str>,
    tmp_dir: &Path,
    max_bytes: u64,
    cancel: &dyn Canceller,
) -> Result<String, UpdateError> {
    if !tmp_dir.is_dir() {
        fs::create_dir_all(tmp_dir).map_err(|e| classify_io(&e, "建临时目录失败"))?;
    }
    let tmp = unique_tmp_path(tmp_dir, "fetch");
    let result = (|| -> Result<String, UpdateError> {
        transport.fetch(
            &FetchSpec { url, dest: &tmp, range_from: 0, accept },
            cancel,
        )?;
        let len = fs::metadata(&tmp).map_err(|e| classify_io(&e, "读临时文件大小失败"))?.len();
        if len > max_bytes {
            return Err(UpdateError::network(format!(
                "{url} 的响应 {len} 字节，超过上限 {max_bytes}"
            )));
        }
        fs::read_to_string(&tmp).map_err(|e| classify_io(&e, "读临时文件失败"))
    })();
    let _ = fs::remove_file(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_are_classified_by_raw_code() {
        let full = io::Error::from_raw_os_error(28);
        assert_eq!(classify_io(&full, "x").kind, ErrorKind::DiskFull);
        let den = io::Error::from_raw_os_error(13);
        assert_eq!(classify_io(&den, "x").kind, ErrorKind::Permission);
        let other = io::Error::from_raw_os_error(2);
        assert_eq!(classify_io(&other, "x").kind, ErrorKind::Io);
    }

    #[test]
    fn error_lines_carry_the_category() {
        let e = UpdateError::network("连接被重置");
        assert_eq!(e.log_line(), "[UPDATE] 网络不可用：连接被重置");
        assert_eq!(UpdateError::verify("大小不符").log_line(), "[UPDATE] 校验失败：大小不符");
        assert_eq!(UpdateError::new(ErrorKind::DiskFull, "写不进").kind.id(), "disk");
        assert_eq!(UpdateError::cancelled().kind, ErrorKind::Cancelled);
    }

    #[test]
    fn http_url_parsing() {
        let u = parse_http_url("http://127.0.0.1:8080/a/b?c=1").expect("合法");
        assert_eq!((u.host.as_str(), u.port, u.path.as_str()), ("127.0.0.1", 8080, "/a/b?c=1"));
        let u = parse_http_url("http://example.com").expect("合法");
        assert_eq!((u.host.as_str(), u.port, u.path.as_str()), ("example.com", 80, "/"));
        assert!(parse_http_url("https://example.com/x").is_err(), "https 不走内置客户端");
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 12\r\nLocation: /x\r\n\r\n";
        assert_eq!(header_value(head, "content-length"), Some("12"));
        assert_eq!(header_value(head, "LOCATION"), Some("/x"));
        assert_eq!(header_value(head, "missing"), None);
    }

    #[test]
    fn curl_exit_codes_are_classified() {
        assert_eq!(curl_error(7, "Failed to connect", "u").kind, ErrorKind::Network);
        assert_eq!(curl_error(22, "The requested URL returned error: 404", "u").kind, ErrorKind::Network);
        assert_eq!(curl_error(23, "No space left on device", "u").kind, ErrorKind::DiskFull);
        assert_eq!(curl_error(23, "Permission denied", "u").kind, ErrorKind::Permission);
    }

    #[test]
    fn only_host_level_failures_mark_a_source_unreachable() {
        // 连不上/被重置/超时 = 主机级（可以把源整轮停用）
        assert!(curl_error(6, "Could not resolve host", "u").host_unreachable);
        assert!(curl_error(7, "Failed to connect", "u").host_unreachable);
        assert!(curl_error(28, "Operation timed out", "u").host_unreachable);
        assert!(curl_error(35, "SSL connect error", "u").host_unreachable);
        // HTTP 404/403（主机是活的）、断点续传不支持 = 不是主机级
        assert!(!curl_error(22, "The requested URL returned error: 404", "u").host_unreachable);
        assert!(!curl_error(33, "doesn't support byte ranges", "u").host_unreachable);
        assert!(!UpdateError::verify("x").host_unreachable);
        assert!(UpdateError::unreachable("x").host_unreachable);
    }

    #[test]
    fn unique_tmp_paths_do_not_collide() {
        let a = unique_tmp_path(Path::new("/tmp"), "fetch");
        let b = unique_tmp_path(Path::new("/tmp"), "fetch");
        assert_ne!(a, b);
    }
}
