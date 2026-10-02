//! 极简 HTTP/1.1 服务（std::net 手写）：窗口页面的静态资源 + JSON API。
//!
//! 不引 HTTP 框架/异步运行时 —— 窗口进程只管本机一个页面，够用且启动快。
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: String,
    /// 原始字节。要落盘的请求体（上传录制文件）必须用它：`body` 是 lossy 转换，
    /// 非 UTF-8 的 GBK 文件会被逐字节改写成 U+FFFD，写出来的文件就坏了。
    /// `body_too_large` 时为空 —— 超大 body 依然不读进内存。
    pub body_bytes: Vec<u8>,
    /// 请求体超过 `MAX_BODY`：不读入内存，交给路由层回 413
    pub body_too_large: bool,
}

/// 请求体上限。超过即拒收（413）—— 上限判断必须发生在读取之前，否则本机任意
/// 进程或网页都能用一个巨大的 Content-Length 让控制台把内存吃光。
/// 上传接口自己的业务上限 16MB 比它更小，见 `api::replay_upload`。
pub const MAX_BODY: usize = 16 * 1024 * 1024;

impl Request {
    pub fn query_get(&self, key: &str) -> Option<String> {
        self.query.get(key).cloned()
    }

    pub fn query_f64(&self, key: &str, default: f64) -> f64 {
        self.query
            .get(key)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(default)
    }

    /// POST JSON 体（FastAPI 那边也是 JSON）
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 路径用的百分号解码：与 query 不同，路径里的 `+` 是字面量，不能当空格
pub fn percent_decode_path(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b[i]);
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn parse_pairs(s: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in s.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        map.insert(percent_decode(k), percent_decode(v));
    }
    map
}

impl Request {
    pub fn read(stream: &mut TcpStream) -> std::io::Result<Request> {
        stream.set_read_timeout(Some(std::time::Duration::from_secs(15)))?;
        let mut buf = Vec::new();
        let mut tmp = [0u8; 8192];
        let head_end;
        loop {
            let n = stream.read(&mut tmp)?;
            if n == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "空请求"));
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                head_end = pos + 4;
                break;
            }
            if buf.len() > 256 * 1024 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "头部过大"));
            }
        }
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let mut lines = head.split("\r\n");
        let req_line = lines.next().unwrap_or("");
        let mut parts = req_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("/").to_string();
        let mut headers = HashMap::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        let content_len: usize = headers
            .get("content-length")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        let body_too_large = content_len > MAX_BODY;
        let mut body_bytes = if body_too_large { Vec::new() } else { buf[head_end..].to_vec() };
        if !body_too_large {
            while body_bytes.len() < content_len {
                let n = stream.read(&mut tmp)?;
                if n == 0 {
                    break;
                }
                body_bytes.extend_from_slice(&tmp[..n]);
            }
            body_bytes.truncate(content_len);
        }

        // 路径也要做百分号解码：前端用 encodeURIComponent 拼文件名（中文/空格），
        // 不解码的话 /api/config/测试.json 会 404。注意路径里的 '+' 是字面量，
        // 不能按 query 的规则当空格。
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (percent_decode_path(p), parse_pairs(q)),
            None => (percent_decode_path(&target), HashMap::new()),
        };
        Ok(Request {
            method,
            path,
            query,
            headers,
            body: String::from_utf8_lossy(&body_bytes).to_string(),
            // 先做 lossy 转换再移动原始字节，两处都不能丢
            body_bytes,
            body_too_large,
        })
    }
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

pub fn respond_bytes(stream: &mut TcpStream, code: u16, ctype: &str, body: &[u8],
                     extra: &[(&str, &str)]) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        code,
        status_text(code),
        ctype,
        body.len()
    );
    for (k, v) in extra {
        head.push_str(&format!("{}: {}\r\n", k, v));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

pub fn respond_json(stream: &mut TcpStream, code: u16, body: &str) -> std::io::Result<()> {
    respond_bytes(stream, code, "application/json; charset=utf-8", body.as_bytes(), &[])
}

/// FastAPI 的 HTTPException 形状：{"detail": "..."}
pub fn respond_error(stream: &mut TcpStream, code: u16, detail: &str) -> std::io::Result<()> {
    let body = serde_json::json!({ "detail": detail }).to_string();
    respond_json(stream, code, &body)
}

/// 静态资源 Content-Type
pub fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

/// 静态资源根目录：<root>/gui/static（外置，改前端不用重编译）
pub fn static_root(root: &Path) -> PathBuf {
    root.join("gui").join("static")
}

/// 防目录穿越：只允许 static 根下的普通文件
pub fn safe_join(base: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim_start_matches('/');
    if rel.contains("..") || rel.contains('\0') {
        return None;
    }
    let full = base.join(rel);
    let full = full.canonicalize().ok()?;
    let base_c = base.canonicalize().ok()?;
    if full.starts_with(&base_c) && full.is_file() {
        Some(full)
    } else {
        None
    }
}

/// 每连接一线程的极简服务（响应由 handler 自己写）
pub fn serve<F>(listener: TcpListener, handler: F)
where
    F: Fn(&mut TcpStream, &Request) + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let handler = Arc::clone(&handler);
            std::thread::spawn(move || {
                if let Ok(req) = Request::read(&mut stream) {
                    handler(&mut stream, &req);
                }
            });
        }
    });
}

/// 绑定 127.0.0.1 的**指定**端口。
///
/// 这个绑定同时是控制台的**单实例判据**（见 main.rs）：绑得上就是唯一实例。
/// 因此这里不做"被占用就顺延下一个端口"的回退 —— 顺延会让第二个控制台悄悄跑起来，
/// 单实例就没了；失败原因（`AddrInUse` 等）原样交给调用方翻译成中文提示。
pub fn bind_port(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port))
}
