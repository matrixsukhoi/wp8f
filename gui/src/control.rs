//! 本地控制通道：给工具/自检用的极简 HTTP 服务（仅 127.0.0.1 + 令牌校验）。
//!
//! 合并成单进程后，这里的"特权操作"直接调用进程内函数（不再跨进程转发）；
//! 保留通道是为了端到端测试与外部脚本能查询/驱动控制台。
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use crate::api::{self, App, UiBridge};
use crate::http;
use crate::Ui;

pub struct Control {
    pub listener: TcpListener,
    pub token: String,
}

impl Control {
    pub fn bind() -> std::io::Result<Control> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        Ok(Control { listener, token: random_token() })
    }

    pub fn url(&self) -> String {
        format!(
            "http://127.0.0.1:{}",
            self.listener.local_addr().map(|a| a.port()).unwrap_or(0)
        )
    }
}

/// 启动控制通道，返回 (url, token)
pub fn serve(app: Arc<App>, ui: Arc<Ui>) -> std::io::Result<(String, String)> {
    let ctl = Control::bind()?;
    let url = ctl.url();
    let token = ctl.token.clone();
    let listener = ctl.listener.try_clone()?;
    let expected_token = token.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let app = Arc::clone(&app);
            let ui = Arc::clone(&ui);
            let expected = expected_token.clone();
            std::thread::spawn(move || {
                let _ = handle(&mut stream, &app, &ui, &expected);
            });
        }
    });
    Ok((url, token))
}

fn random_token() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut x = t ^ ((std::process::id() as u128) << 32);
    let mut s = String::new();
    for _ in 0..32 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        s.push(char::from_digit(((x >> 33) % 16) as u32, 16).unwrap());
    }
    s
}

fn form_get(body: &str, key: &str) -> Option<String> {
    for pair in body.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        let v = it.next().unwrap_or("");
        if k == key {
            return Some(http::percent_decode(v));
        }
    }
    None
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " ")
}

fn handle(
    stream: &mut TcpStream,
    app: &Arc<App>,
    ui: &Arc<Ui>,
    expected_token: &str,
) -> std::io::Result<()> {
    let req = match http::Request::read(stream) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    let token_ok = req
        .headers
        .get("x-wp8f-token")
        .map(|v| v == expected_token)
        .unwrap_or(false);
    if !token_ok {
        return http::respond_json(stream, 403, "{\"ok\":false,\"error\":\"bad token\"}");
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/control/status") => {
            // 附带页面地址，方便工具直接访问窗口 API
            let body = ui.status_json().replace(
                "\"ok\":true",
                &format!("\"ok\":true,\"window_url\":\"http://127.0.0.1:{}\"", app.port),
            );
            http::respond_json(stream, 200, &body)
        }
        ("GET", "/control/ping") => http::respond_json(stream, 200, "{\"ok\":true}"),
        ("POST", "/control/wp8f/launch") => {
            let config =
                form_get(&req.body, "config").unwrap_or_else(|| "config/default.json".into());
            let drag =
                form_get(&req.body, "drag").map(|v| v == "1" || v == "true").unwrap_or(false);
            let console =
                form_get(&req.body, "console").map(|v| v == "1" || v == "true").unwrap_or(false);
            match ui.children.launch_wp8f(&config, drag, console) {
                Ok((pid, ts)) => http::respond_json(
                    stream,
                    200,
                    &format!(
                        "{{\"ok\":true,\"pid\":{},\"test_server_pid\":{}}}",
                        pid,
                        ts.map(|p| p as i64).unwrap_or(-1)
                    ),
                ),
                Err(e) => http::respond_json(
                    stream,
                    409,
                    &format!("{{\"ok\":false,\"error\":\"{}\"}}", json_escape(&e)),
                ),
            }
        }
        ("POST", "/control/wp8f/stop") => {
            let killed = ui.children.stop_wp8f();
            http::respond_json(stream, 200, &format!("{{\"ok\":true,\"killed\":{:?}}}", killed))
        }
        ("POST", "/control/window/show") => {
            // 需求：先 kill wp8f 再显示窗口 —— 只 kill 本进程启动的那个（stop_wp8f 按自己记录的 pid）
            let killed = ui.children.stop_wp8f();
            let ok = api::UiBridge::show_window(&**ui);
            http::respond_json(
                stream,
                200,
                &format!(
                    "{{\"ok\":{},\"killed\":{:?},\"pid\":{}}}",
                    ok,
                    killed,
                    std::process::id()
                ),
            )
        }
        ("POST", "/control/window/close") => {
            let ok = api::UiBridge::close_window(&**ui);
            http::respond_json(stream, 200, &format!("{{\"ok\":{}}}", ok))
        }        ("POST", "/control/exit") => {
            http::respond_json(stream, 200, "{\"ok\":true,\"exiting\":true}")?;
            let _ = api::UiBridge::request_quit(&**ui);
            Ok(())
        }
        _ => http::respond_json(stream, 404, "{\"ok\":false,\"error\":\"unknown route\"}"),
    }
}
