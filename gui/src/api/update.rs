//! 更新器的 HTTP 接口层（P3）：`/api/update/*`。
//!
//! 这一层只做三件事：把请求转成 `wp8f_flightmodel::update` 的调用、把状态快照转成 JSON、
//! 把错误按类别（网络/校验/磁盘/权限/HUD 在跑…）原样交给前端 —— **文案在前端按 i18n 键选**，
//! 这里返回的是机器可读的 `kind` + 技术细节（`message` 是给人看的诊断串，不是 UI 文案）。
//!
//! 接口（全部返回 HTTP 200 + `{"ok": bool, ...}`：更新失败是**业务结果**，不是 HTTP 错误，
//! 前端一次读 body 就能同时拿到"失败原因"和"最新状态"）：
//!
//! | 方法 | 路径 | 作用 |
//! |---|---|---|
//! | GET  | `/api/update/status` | 状态快照（版本/进度/错误/日志） |
//! | POST | `/api/update/check`  | 检查更新（同步，一次 HTTP） |
//! | POST | `/api/update/start`  | 开始下载（后台线程；body 可带 `{"tag": "2.59.0.43"}`） |
//! | POST | `/api/update/cancel` | 取消下载（已完成的文件保留，可续） |
//! | POST | `/api/update/verify` | 只校验下载目录（对照清单；只读、不动任何文件） |
//! | POST | `/api/update/apply`  | **校验通过 + HUD 没在跑**才更新到新数据（改名 + 失败回滚） |
//! | POST | `/api/update/discard`| 清除下载目录（`data_new` + `.wp8f_update_tmp`） |
//!
//! **没有"停止 HUD"这个动作**：HUD 在跑时 `apply` 直接**拒绝**并给出
//! `kind = "hud_running"` 的失败消息，由用户自己去关 —— 更新器不杀进程。
//!
//! 单例：更新器是**进程级**的（与 HUD 的 `link_ring` 槽同思路）——一个控制台进程只有一份
//! 下载状态，`App` 里再挂一个字段会让 main.rs 也跟着改，没必要。

use std::net::TcpStream;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use serde_json::{json, Value};
use wp8f_flightmodel::update::{self, Phase, UpdateManager, UpdatePaths};

use super::{App, UiBridge};
use crate::http::{self, Request};

/// 进程级更新器（首次访问时按仓库根初始化）。
static MANAGER: OnceLock<Arc<UpdateManager>> = OnceLock::new();

fn manager(root: &Path) -> &'static Arc<UpdateManager> {
    MANAGER.get_or_init(|| UpdateManager::new(UpdatePaths::from_repo_root(root)))
}

fn respond(stream: &mut TcpStream, v: &Value) {
    let _ = http::respond_json(stream, 200, &v.to_string());
}

/// 状态快照 → JSON（前端只认这里的字段名，字段名就是它的接口）。
fn status_json(app: &Arc<App>, m: &UpdateManager) -> Value {
    let st = m.status();
    let hud_running = app.ui.wp8f_running();
    json!({
        "phase": st.phase.id(),
        "busy": st.phase.busy(),
        "local_version": st.local_version,
        "remote_version": st.remote_version,
        "update_available": st.update_available,
        "staging_version": st.staging_version,
        "staging_ready": st.staging_ready,
        "progress": {
            "files_total": st.files_total,
            "files_done": st.files_done,
            "files_failed": st.files_failed,
            "bytes_total": st.bytes_total,
            "bytes_done": st.bytes_done,
            "bytes_total_h": update::download::human_bytes(st.bytes_total),
            "bytes_done_h": update::download::human_bytes(st.bytes_done),
            "ratio": if st.files_total == 0 { 0.0 } else { (st.files_done as f64 / st.files_total as f64).min(1.0) },
            "retries": st.retries,
        },
        "failed": st.failed.iter().map(|f| json!({
            "rel": f.rel, "kind": f.kind.id(), "reason": f.reason,
        })).collect::<Vec<_>>(),
        "disabled_sources": st.disabled_sources.iter().map(|d| json!({
            "id": d.id, "name": d.name, "reason": d.reason,
        })).collect::<Vec<_>>(),
        "sources": update::manifest::default_sources().iter().map(|s| json!({
            "id": s.id(), "name": s.name(),
        })).collect::<Vec<_>>(),
        "error": st.error.as_ref().map(|e| json!({
            "kind": e.kind.id(), "label": e.kind.label(), "message": e.message,
        })),
        "manifest": { "note": st.manifest_note, "authoritative": st.manifest_authoritative },
        "json_supported": st.json_supported,
        "log": st.log,
        "data_root": st.data_root,
        "staging_root": st.staging_root,
        // 上一版数据固定单槽：状态里只有"有没有 / 在哪"两条
        "old_exists": st.old_exists,
        "old_root": st.old_root,
        "dropped_top_level": st.dropped_top_level,
        "hud_running": hud_running,
        "phase_idle": st.phase == Phase::Idle,
        "phase_ready": st.phase == Phase::Ready,
    })
}

/// 成功响应：`{"ok":true,"status":{…}}`。
fn ok_with_status(app: &Arc<App>, stream: &mut TcpStream, m: &UpdateManager) {
    respond(stream, &json!({ "ok": true, "status": status_json(app, m) }));
}

/// 失败响应：`{"ok":false,"error":{kind,label,message},"status":{…}}`（HTTP 仍是 200）。
fn fail_with_status(app: &Arc<App>, stream: &mut TcpStream, m: &UpdateManager, e: &update::net::UpdateError) {
    respond(
        stream,
        &json!({
            "ok": false,
            "error": { "kind": e.kind.id(), "label": e.kind.label(), "message": e.message },
            "status": status_json(app, m),
        }),
    );
}

/// 路由：命中 `/api/update*` 返回 `true`（调用方直接 return）。
pub fn route(app: &Arc<App>, stream: &mut TcpStream, req: &Request) -> bool {
    let path = req.path.as_str();
    if !path.starts_with("/api/update") {
        return false;
    }
    let m = manager(&app.root);
    match (req.method.as_str(), path) {
        ("GET", "/api/update/status") => {
            respond(stream, &json!({ "ok": true, "status": status_json(app, m) }));
        }
        ("POST", "/api/update/check") => match m.check() {
            Ok(_) => ok_with_status(app, stream, m),
            Err(e) => fail_with_status(app, stream, m, &e),
        },
        ("POST", "/api/update/start") => {
            // 允许显式指定 tag（默认用最近一次检查到的远端版本），但**必须**是 2.59.0.43 这种
            let body = req.json();
            let tag = body
                .get("tag")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            match m.start_download(tag) {
                Ok(()) => ok_with_status(app, stream, m),
                Err(e) => fail_with_status(app, stream, m, &e),
            }
        }
        ("POST", "/api/update/cancel") => {
            m.cancel();
            ok_with_status(app, stream, m);
        }
        ("POST", "/api/update/verify") => match m.verify_staging() {
            Ok(_) => ok_with_status(app, stream, m),
            Err(e) => fail_with_status(app, stream, m, &e),
        },
        ("POST", "/api/update/apply") => {
            // **HUD 在跑就不许换数据**：这里只拒绝 + 说清原因，
            // 绝不替用户结束 wp8f.exe。判据用 `children.rs` 现成的那套
            // （自启记录 pid + 按映像名 wp8f.exe 复核），不额外扫进程。
            let hud_running = app.ui.wp8f_running();
            if hud_running {
                m.push_log("[UPDATE] 拒绝更新：检测到 wp8f.exe 正在运行（请用户自行关闭）");
            }
            match m.apply_switch(hud_running) {
                Ok(_) => ok_with_status(app, stream, m),
                Err(e) => fail_with_status(app, stream, m, &e),
            }
        }
        ("POST", "/api/update/discard") => match m.discard_staging() {
            Ok(_) => ok_with_status(app, stream, m),
            Err(e) => fail_with_status(app, stream, m, &e),
        },
        _ => {
            respond(
                stream,
                &json!({ "ok": false, "error": { "kind": "protocol", "label": "未知动作", "message": format!("{} {}", req.method, path) } }),
            );
        }
    }
    true
}
