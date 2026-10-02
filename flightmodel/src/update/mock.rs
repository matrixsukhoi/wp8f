//! 本地假源（**只在单测里编译**）：一个极小的 HTTP/1.1 服务端 + 故障注入。
//!
//! 为什么要它：P3 的验证红线是"不要真下 361 MB"，但下载器要测的行为全是网络行为
//! （重试、多源回退、续传、校验、取消、失败不影响现有数据）。所以单测把
//! `http://127.0.0.1:<随机端口>` 当源，走的是**生产代码那条内置 http 客户端**，
//! 只有"哪个 URL 有哪个文件"是假的。
//!
//! 故障注入：
//! * [`MockSource::fail_next`]：接下来 N 个请求回 500（HTTP 层失败，不算"主机不可达"）；
//! * [`MockSource::break_next`]：接下来 N 个请求直接断连（模拟本机实测的
//!   `raw.githubusercontent.com` 被 reset：客户端应归类成"主机不可达"）;
//! * [`MockSource::ignore_range`]：Range 请求也回 200 整份 —— **历史实测的 CDN 行为**
//!   （见 `download.rs` 模块头），用来锁住"不做文件内续传"的决定；
//! * [`MockSource::truncate_next`]：正文只发一半就断开（模拟下载中途断流）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// 一次请求的记录（断言"跳过已完成文件"用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    /// `Range: bytes=N-` 里的 N
    pub range_from: Option<u64>,
}

#[derive(Default)]
struct State {
    break_next: AtomicUsize,
    fail_next: AtomicUsize,
    truncate_next: AtomicUsize,
    sleep_ms: AtomicU64,
    ignore_range: AtomicBool,
    hits: Mutex<Vec<Hit>>,
    /// 命中这些子串的请求一律回 500（只掐某一类接口，不动别的）
    fail_matching: Mutex<Vec<String>>,
}

/// 本地假源。
pub struct MockSource {
    port: u16,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    state: Arc<State>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockSource {
    /// 起一个随机端口的假源（Drop 时停线程）。
    pub fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock source");
        let port = listener.local_addr().expect("local_addr").port();
        listener.set_nonblocking(true).expect("nonblocking");
        let files: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
        let state = Arc::new(State::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (f, s, st) = (Arc::clone(&files), Arc::clone(&state), Arc::clone(&stop));
        let handle = thread::spawn(move || {
            let mut conns: Vec<JoinHandle<()>> = Vec::new();
            while !st.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (f, s) = (Arc::clone(&f), Arc::clone(&s));
                        conns.push(thread::spawn(move || {
                            let _ = serve(stream, &f, &s);
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
            for c in conns {
                let _ = c.join();
            }
        });
        Self { port, files, state, stop, handle: Some(handle) }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// 根地址（测试里拼 URL 用）：`http://127.0.0.1:<port>`
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 放一个文件（路径要带前导 `/`）。
    pub fn put(&self, path: &str, body: impl Into<Vec<u8>>) {
        self.files
            .lock()
            .expect("lock files")
            .insert(path.to_string(), body.into());
    }

    pub fn put_json(&self, path: &str, body: &str) {
        self.put(path, body.as_bytes().to_vec());
    }

    /// 接下来 n 个请求回 500。
    pub fn fail_next(&self, n: usize) {
        self.state.fail_next.store(n, Ordering::SeqCst);
    }

    /// 接下来 n 个请求直接断连（模拟 github 被 reset）。
    pub fn break_next(&self, n: usize) {
        self.state.break_next.store(n, Ordering::SeqCst);
    }

    /// 接下来 n 个请求的正文只发一半。
    pub fn truncate_next(&self, n: usize) {
        self.state.truncate_next.store(n, Ordering::SeqCst);
    }

    /// 每个请求先睡这么久（测取消/并发）。
    pub fn set_sleep_ms(&self, ms: u64) {
        self.state.sleep_ms.store(ms, Ordering::SeqCst);
    }

    /// 路径里含这些子串的请求一律回 500（用来只掐"数据文件"或只掐"某个接口"）。
    pub fn fail_matching(&self, needles: &[&str]) {
        let mut g = self.state.fail_matching.lock().expect("lock fail_matching");
        g.clear();
        g.extend(needles.iter().map(|s| s.to_string()));
    }

    /// Range 请求也回整份（实测的 jsDelivr 行为）。
    pub fn ignore_range(&self, on: bool) {
        self.state.ignore_range.store(on, Ordering::SeqCst);
    }

    /// 到目前为止收到的请求（按顺序）。
    pub fn hits(&self) -> Vec<Hit> {
        self.state.hits.lock().expect("lock hits").clone()
    }

    /// 某个路径被请求了几次。
    pub fn hit_count(&self, path: &str) -> usize {
        self.hits().iter().filter(|h| h.path == path).count()
    }

    pub fn clear_hits(&self) {
        self.state.hits.lock().expect("lock hits").clear();
    }
}

impl Drop for MockSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn serve(
    mut stream: TcpStream,
    files: &Mutex<HashMap<String, Vec<u8>>>,
    state: &State,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut raw: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    while raw.len() < 16 * 1024 {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                raw.push(byte[0]);
                if raw.len() >= 4 && &raw[raw.len() - 4..] == b"\r\n\r\n" {
                    break;
                }
            }
            Err(_) => return Ok(()),
        }
    }
    let head = String::from_utf8_lossy(&raw).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let _method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/").to_string();
    let path = target.split('?').next().unwrap_or("/").to_string();
    let range_from = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("range:"))
        .and_then(|l| l.split_once('='))
        .and_then(|(_, v)| v.trim().trim_end_matches('-').parse::<u64>().ok());
    state
        .hits
        .lock()
        .expect("lock hits")
        .push(Hit { path: path.clone(), range_from });

    if state.break_next.load(Ordering::SeqCst) > 0 {
        state.break_next.fetch_sub(1, Ordering::SeqCst);
        // 直接关连接：客户端读到 EOF，归类为"主机不可达"
        drop(stream);
        return Ok(());
    }
    let sleep_ms = state.sleep_ms.load(Ordering::SeqCst);
    if sleep_ms > 0 {
        thread::sleep(Duration::from_millis(sleep_ms));
    }
    if state.fail_next.load(Ordering::SeqCst) > 0 {
        state.fail_next.fetch_sub(1, Ordering::SeqCst);
        let body = b"mock: injected failure".to_vec();
        write_head(&mut stream, 500, "Internal Server Error", body.len(), None)?;
        let _ = stream.write_all(&body);
        return Ok(());
    }
    {
        let needles = state.fail_matching.lock().expect("lock fail_matching");
        if needles.iter().any(|n| path.contains(n.as_str())) {
            let body = b"mock: injected failure (matched)".to_vec();
            write_head(&mut stream, 500, "Internal Server Error", body.len(), None)?;
            let _ = stream.write_all(&body);
            return Ok(());
        }
    }

    let body = files.lock().expect("lock files").get(&path).cloned();
    let Some(body) = body else {
        let msg = format!("mock: not found: {path}").into_bytes();
        write_head(&mut stream, 404, "Not Found", msg.len(), None)?;
        let _ = stream.write_all(&msg);
        return Ok(());
    };

    let total = body.len() as u64;
    let use_range = range_from.filter(|_| !state.ignore_range.load(Ordering::SeqCst));
    match use_range {
        Some(from) if from < total => {
            let slice = &body[from as usize..];
            write_head(
                &mut stream,
                206,
                "Partial Content",
                slice.len(),
                Some(&format!("bytes {from}-{}/{total}", total - 1)),
            )?;
            let _ = stream.write_all(slice);
        }
        Some(_) => {
            // 越界：416（客户端应把它当成"没有新增字节"）
            write_head(&mut stream, 416, "Range Not Satisfiable", 0, Some(&format!("bytes */{total}")))?;
        }
        None => {
            write_head(&mut stream, 200, "OK", body.len(), None)?;
            if state.truncate_next.load(Ordering::SeqCst) > 0 {
                state.truncate_next.fetch_sub(1, Ordering::SeqCst);
                let _ = stream.write_all(&body[..body.len() / 2]);
                drop(stream);
                return Ok(());
            }
            let _ = stream.write_all(&body);
        }
    }
    Ok(())
}

fn write_head(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    len: usize,
    content_range: Option<&str>,
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/octet-stream\r\n\
         Content-Length: {len}\r\nConnection: close\r\n"
    );
    if let Some(cr) = content_range {
        head.push_str(&format!("Content-Range: {cr}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())
}
