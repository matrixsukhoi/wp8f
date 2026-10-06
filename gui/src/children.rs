//! 子进程监管：wp8f / test-server 的启动、单实例、回收与状态。
//!
//! 窗口不再单独起进程（已合并进本进程），所以这里只管业务子进程；
//! 作业对象保证本进程无论怎么死（含 taskkill /F），子进程一起结束。
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::win::{self, Job};

pub struct Children {
    pub root: PathBuf,
    job: Job,
    reg: Mutex<Vec<(String, u32)>>,
    wp8f_pid: Mutex<Option<u32>>,
    test_server_pid: Mutex<Option<u32>>,
}

impl Children {
    pub fn new(root: PathBuf) -> Option<Children> {
        let job = Job::create()?;
        Some(Children {
            root,
            job,
            reg: Mutex::new(Vec::new()),
            wp8f_pid: Mutex::new(None),
            test_server_pid: Mutex::new(None),
        })
    }

    fn record(&self, kind: &str, pid: u32) {
        if let Ok(mut reg) = self.reg.lock() {
            reg.push((kind.to_string(), pid));
            let len = reg.len();
            if len > 32 {
                reg.drain(0..len - 32);
            }
        }
    }

    fn spawn_tracked(&self, kind: &str, cmd: &[String], cwd: Option<&str>, new_console: bool) -> Result<u32, String> {
        let log = self.root.join("logs").join(format!("{}.log", kind));
        let pid = win::spawn(cmd, cwd, Some(&log), new_console)?;
        // 绑定失败不影响本次启动，但 kill-on-close 兜底会失效（本进程被杀时子进程残留），
        // 这种情况必须是可见的，不能静默吞掉
        if !self.job.assign(pid) {
            crate::log(&format!(
                "未能把 {kind}（pid={pid}）加入 Job Object：退出时可能不会连带回收"
            ));
        }
        self.record(kind, pid);
        Ok(pid)
    }

    // ---------- wp8f / test-server ----------
    //
    // 归属原则：**控制台只管理自己启动的子进程**。
    // `win::pids_of`（全机按映像名扫描）只能用于状态展示；任何 kill 与单实例判断都必须
    // 走本进程 `spawn_tracked` 时记下的 pid —— 用户自己开的 wp8f.exe 既不会阻止我们启动，
    // 也不会被我们结束。

    /// 全机按映像名扫出来的 wp8f.exe（**只给状态展示**，见上面的归属原则）
    pub fn all_wp8f_pids(&self) -> Vec<u32> {
        win::pids_of("wp8f.exe")
    }

    /// 我们自己启动的 wp8f 是否还在跑（**单实例判据**）
    pub fn wp8f_running(&self) -> bool {
        self.own_wp8f().is_some()
    }

    /// 记录里的 wp8f pid：复核"还活着 + 映像名仍是 wp8f.exe"之后才认
    fn own_wp8f(&self) -> Option<u32> {
        let recorded = self.wp8f_pid.lock().ok().and_then(|g| *g);
        let scanned = win::pids_of("wp8f.exe");
        owned_pid(recorded, &win::pid_alive, &|p| scanned.contains(&p))
    }

    /// 记录里的 test-server pid（复核口径同上）
    fn own_test_server(&self) -> Option<u32> {
        let recorded = self.test_server_pid.lock().ok().and_then(|g| *g);
        let scanned = win::pids_of("test-server.exe");
        owned_pid(recorded, &win::pid_alive, &|p| scanned.contains(&p))
    }

/// 丢掉不再成立的记录（进程已退 / pid 被复用）。
    /// 留着死 pid 会污染"预览模式两个 pid 都记着"这类判断。
    fn forget_stale_records(&self) {
        if self.own_wp8f().is_none() {
            if let Ok(mut g) = self.wp8f_pid.lock() {
                *g = None;
            }
        }
        if self.own_test_server().is_none() {
            if let Ok(mut g) = self.test_server_pid.lock() {
                *g = None;
            }
        }
    }

    /// 模拟服务端可执行文件（S1 布局：exe 在 `binary/`，场景目录仍在 `test-server/`）。
    fn test_server_exe(&self) -> PathBuf {
        self.root.join("binary").join("test-server.exe")
    }

    /// 启动本地模拟服务端（拖拽预览用）。自己起的还在跑则复用。
    fn ensure_test_server(&self) -> Option<u32> {
        self.forget_stale_records();
        if let Some(pid) = self.own_test_server() {
            return Some(pid);
        }
        if win::is_running("test-server.exe") {
            // 别人的 test-server.exe：不再起第二个（8111 端口会冲突），但它不归我们管
            // —— 不记录、不回收，也不参与 reaper 判据
            crate::log("检测到非本进程启动的 test-server.exe：复用其端口，但不回收它");
            return None;
        }
        let exe = self.test_server_exe();
        if !exe.is_file() {
            crate::log(&format!(
                "未找到模拟服务端 {}：拖拽预览没有数据源（先跑 scripts/build.sh）",
                exe.display()
            ));
            return None;
        }
        let cmd = vec![
            exe.to_string_lossy().to_string(),
            "--scenario".into(),
            "scenarios/cruise.toml".into(),
        ];
        let cwd = self.root.join("test-server");
        match self.spawn_tracked("test-server", &cmd, Some(&cwd.to_string_lossy()), false) {
            Ok(pid) => {
                if let Ok(mut g) = self.test_server_pid.lock() {
                    *g = Some(pid);
                }
                crate::log(&format!("已启动自己管理的 test-server pid={pid}（拖拽预览）"));
                Some(pid)
            }
            Err(_) => None,
        }
    }

    /// 启动 wp8f（参数口径同控制台里的启动按钮）。
    ///
    /// **一个控制台只允许一个自己启动的 wp8f**（外加预览时的一个 test-server）：
    /// 已经有一个在跑就直接失败（500 + 原因），不再"先杀掉上一批再起新的" —— 那会让
    /// 用户以为点了没反应，也把预览里刚拖好的位置连同进程一起丢掉。
    /// 要换配置/换模式：先用托盘的「停止 HUD」把它停掉（或直接在 HUD 窗口按 ESC 退出）。
    /// 用户自己开的 wp8f.exe 一律不动（判据见 [`Self::stop_wp8f`]）。
    ///
    /// **预览用的模拟服务端会先让位**：非预览启动（点「开 始」）时，若自己启动的 test-server
    /// 还在跑，先结束它再起 wp8f —— 它占着 8111，HUD 一连上去读到的就是模拟数据而不是真游戏。
    /// 预览启动（`drag`）则相反：那个 test-server 正是数据源，复用/新起，不收。
    pub fn launch_wp8f(&self, config: &str, drag: bool, console: bool) -> Result<(u32, Option<u32>), String> {
        if let Some(pid) = self.own_wp8f() {
            return Err(format!(
                "已经有一个由本控制台启动的 wp8f.exe 在运行（pid={pid}）：\
                 先停止它再启动（托盘右键 → 停止 HUD，或在 HUD 窗口按 ESC 退出）。"
            ));
        }
        if drag {
            if let Some(pid) = self.own_test_server() {
                return Err(format!(
                    "已经有一个由本控制台启动的 test-server.exe 在运行（pid={pid}）：\
                     先停止它再启动预览。"
                ));
            }
        }
        let exe = self.root.join("binary").join("wp8f.exe");
        if !exe.is_file() {
            return Err(format!("wp8f 可执行文件不存在: {}", exe.display()));
        }
        // 非预览启动：先把预览服务端收掉（只收自己启动的那个，别人的一律不动）。
        // 放在 exe 检查之后：缺件这种必失败的路径上不该顺手改进程状态。
        if let Some(pid) = preview_server_to_stop(drag, self.own_test_server()) {
            if self.kill_test_server() == Some(pid) {
                crate::log(&format!(
                    "启动 HUD 前先结束自己启动的 test-server pid={pid}\
                     （它占着 8111，会让 HUD 读到模拟数据）"
                ));
            }
        }
        let test_server = if drag { self.ensure_test_server() } else { None };
        let cfg = if config.is_empty() {
            "config/default.json".to_string()
        } else {
            config.to_string()
        };
        // **不传 `--hud-type`**：命令行会盖掉配置文件里的选择（曾经硬编码成 circle，
        // 于是在 GUI 里改成 MiniHUD 也没用）。画哪种 HUD 一律由配置的 `hud_type` 决定。
        let mut cmd = vec![
            exe.to_string_lossy().to_string(),
            "--port".into(),
            "8111".into(),
            "--layoutconfig".into(),
            cfg,
        ];
        if drag {
            cmd.push("--drag".into());
        }
        let pid = self.spawn_tracked("wp8f", &cmd, Some(&self.root.to_string_lossy()), console)?;
        if let Ok(mut g) = self.wp8f_pid.lock() {
            *g = Some(pid);
        }
        crate::log(&format!("已启动自己管理的 wp8f pid={pid}（drag={drag} console={console}）"));
        Ok((pid, test_server))
    }

    /// 启动后存活复核：`spawn` 成功 ≠ wp8f 跑起来了。参数被 clap 拒绝（例如
    /// wp8f.exe 与控制台版本/参数不一致）会立刻 exit(2)，而前端只要拿到
    /// 200 就报「wp8f 已启动」并关窗，用户看不到任何原因 —— 这里等一小会儿复核，
    /// 已退出就返回 500 + 日志末尾。
    pub fn verify_wp8f_alive(&self, pid: u32) -> Result<(), (u16, String)> {
        // 250ms 覆盖「启动期就失败」的情况，又不至于把正常启动误判成失败
        std::thread::sleep(Duration::from_millis(250));
        if win::pid_alive(pid) {
            return Ok(());
        }
        let log = self.root.join("logs").join("wp8f.log");
        Err((
            500,
            format!(
                "wp8f 启动后立即退出（pid={pid}）：参数被拒绝或初始化失败。\
                 可能是 wp8f.exe 与控制台版本/参数不一致（clap 拒绝未知参数会直接以 \
                 exit(2) 退出）。\n\
                 {} 末尾：\n{}",
                log.display(),
                log_tail(&log, 15)
            ),
        ))
    }

    /// 结束 wp8f（点托盘「显示主窗口」/关闭窗口前先做这件事）。
    ///
    /// **只结束自己启动的那一个**：用户自己开的 wp8f.exe 一律不动
/// 只结束自己启动的那些进程。
    pub fn stop_wp8f(&self) -> Vec<u32> {
        let mut killed = Vec::new();
        if let Some(pid) = self.own_wp8f() {
            if self.kill_own_wp8f(pid) {
                killed.push(pid);
            }
        }
        if let Ok(mut g) = self.wp8f_pid.lock() {
            *g = None;
        }
        if let Some(ts) = self.kill_test_server() {
            killed.push(ts);
        }
        killed.sort_unstable();
        killed.dedup();
        killed
    }

    /// 结束一个"自己启动的" wp8f：**kill 前再复核一次归属**（见 owned_pid）。
    /// 归属不成立就什么都不做 —— 宁可漏杀也不误杀。
    fn kill_own_wp8f(&self, pid: u32) -> bool {
        if self.own_wp8f() != Some(pid) || !win::kill_pid(pid) {
            return false;
        }
        crate::log(&format!("结束自己启动的 wp8f pid={pid}"));
        if let Ok(mut g) = self.wp8f_pid.lock() {
            if *g == Some(pid) {
                *g = None;
            }
        }
        true
    }

    /// 结束自己启动的 test-server（用户自己开的不动：只看记录里的那一个 pid）
    fn kill_test_server(&self) -> Option<u32> {
        let pid = self.own_test_server()?;
        if !win::kill_pid(pid) {
            return None;
        }
        crate::log(&format!("结束自己启动的 test-server pid={pid}"));
        if let Ok(mut g) = self.test_server_pid.lock() {
            *g = None;
        }
        Some(pid)
    }

    /// 预览模式的双向回收：自己启动的 wp8f 与 test-server **任一退出，另一个一起收**。
/// 判据只用自己记录的 pid：全机扫描会把用户手开的 wp8f.exe 当成"还活着"，
    /// "wp8f 还活着"，导致 test-server 留成僵尸；test-server 先死那条也一并处理。
    pub fn start_reaper(self: std::sync::Arc<Self>) {
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(3));
            self.reap_once();
        });
    }

    /// 单轮回收（3 s 一次）：两个 pid 都记着（预览模式）才互相兜底；
    /// 只有一边有记录（非预览模式）→ 什么都不做。
    fn reap_once(&self) {
        // "记录里有没有"用原始记录判断；"还活着"必须用**归属复核**后的结果，
        // 不能用裸 pid_alive —— 过期记录（进程早退了、pid 被别人复用）会被误判成还在跑
        let rec_wp8f = self.wp8f_pid.lock().ok().and_then(|g| *g);
        let rec_ts = self.test_server_pid.lock().ok().and_then(|g| *g);
        let ours_wp8f = self.own_wp8f();
        let ours_ts = self.own_test_server();
        let alive = |p: u32| ours_wp8f == Some(p) || ours_ts == Some(p);
        match reap_decision(rec_wp8f, rec_ts, &alive) {
            Reap::Idle => {}
            Reap::KillWp8f(pid) => {
                if self.kill_own_wp8f(pid) {
                    crate::log(&format!(
                        "预览：test-server 已退出，连带回收自己启动的 wp8f pid={pid}"
                    ));
                }
            }
            Reap::KillTestServer(pid) => {
                if self.kill_test_server() == Some(pid) {
                    crate::log(&format!(
                        "预览：wp8f 已退出，连带回收自己启动的 test-server pid={pid}"
                    ));
                }
            }
        }
    }

    /// 结束全部子进程（幂等）；作业对象是最终兜底
    pub fn kill_all(&self) -> Vec<u32> {
        let kids: Vec<(String, u32)> = self.reg.lock().map(|g| g.clone()).unwrap_or_default();
        let mut killed = Vec::new();
        for (_kind, pid) in kids {
            if win::pid_alive(pid) && win::kill_pid(pid) {
                killed.push(pid);
            }
        }
        if let Ok(mut g) = self.reg.lock() {
            g.clear();
        }
        for g in [&self.wp8f_pid, &self.test_server_pid] {
            if let Ok(mut g) = g.lock() {
                *g = None;
            }
        }
        killed
    }

    /// 控制通道/API 用的状态 JSON（窗口信息由 UI 层传入）
    ///
    /// `wp8f_pids` 是**全机扫描**（展示用：能看出机器上还有别人的 wp8f），
    /// `wp8f_running` 是**我们自己启动的**实例是否还在跑 —— 两个字段名都是
    /// api_parity 契约（`scripts/tests/api_parity.py` 的 VOLATILE_KEYS），不能改名。
    pub fn status_json(&self, webview_alive: bool, pid: u32) -> String {
        let wp8f = self.all_wp8f_pids();
        // test_server_pid：只报**自己启动的**那一个（-1 = 没有），字段名是 api_parity 契约
        let ts = self.own_test_server();
        format!(
            "{{\"ok\":true,\"pid\":{},\"window_pid\":{},\"window_exists\":{},\"wp8f_running\":{},\
             \"wp8f_pids\":{:?},\"test_server_pid\":{}}}",
            pid,
            pid,
            webview_alive,
            self.wp8f_running(),
            wp8f,
            ts.map(|p| p as i64).unwrap_or(-1)
        )
    }
}

// ---------------- 归属与回收的纯逻辑（无 Win32 依赖，单测直接覆盖） ----------------

/// 归属判定：`recorded` 是启动时记下的 pid，`alive` 探活，`is_exe` 复核映像名。
/// 任一环节不成立都返回 None —— **不确定就不动它**。
/// 复核映像名是为了防 pid 复用：子进程退出后 pid 会被系统分给别的进程，
/// 只按 pid 记录去 kill 就会误杀无关进程（对"只管理自己启动的进程"这条原则是必须的兜底）。
fn owned_pid(
    recorded: Option<u32>,
    alive: &dyn Fn(u32) -> bool,
    is_exe: &dyn Fn(u32) -> bool,
) -> Option<u32> {
    let pid = recorded?;
    if alive(pid) && is_exe(pid) {
        Some(pid)
    } else {
        None
    }
}

/// 预览模式的单轮回收动作
#[derive(Debug, PartialEq, Eq)]
enum Reap {
    /// 不动作：非预览模式 / 两个都活着 / 两个都已退出
    Idle,
    /// test-server 先退出 → 收掉配套的 wp8f
    KillWp8f(u32),
    /// wp8f 先退出 → 收掉配套的 test-server
    KillTestServer(u32),
}

/// 双向回收决策（纯函数）：**两个 pid 都是自己记录的**（预览模式）才互相兜底，
/// 一边退出就杀另一边；都活着、或只有一边有记录（非预览模式）→ 不动作。
/// 判据只用自己记录的 pid：拿全机扫描当"wp8f 是否还活着"，用户手动开的 wp8f.exe
/// 会让判据永远为真，预览的 test-server 就留成僵尸了。
fn reap_decision(wp8f: Option<u32>, test_server: Option<u32>, alive: &dyn Fn(u32) -> bool) -> Reap {
    let (Some(w), Some(t)) = (wp8f, test_server) else {
        return Reap::Idle;
    };
    match (alive(w), alive(t)) {
        (false, true) => Reap::KillTestServer(t),
        (true, false) => Reap::KillWp8f(w),
        // 都活着 = 正常预览中；都已退出 = 没有需要保护的另一半
        _ => Reap::Idle,
    }
}

/// 启动 wp8f 之前要不要先收掉预览用的模拟服务端？（纯函数，便于单测）
///
/// 非预览启动（点「开 始」）要让 HUD 连**真游戏**的 8111；自己启动的 test-server 还占着那个
/// 端口的话，HUD 读到的是模拟数据 ⇒ 先收掉它、再起 wp8f。预览启动则相反：那个 test-server
/// 正是数据源（复用/新起见 `ensure_test_server`），绝不能收。
fn preview_server_to_stop(drag: bool, own_test_server: Option<u32>) -> Option<u32> {
    if drag {
        None
    } else {
        own_test_server
    }
}

/// 读日志末尾若干行：只从文件尾部读固定字节数，不把整个日志读进内存
/// （logs/wp8f.log 跑久了能到几十 MB）
fn log_tail(path: &Path, max_lines: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    const TAIL_BYTES: u64 = 4096;
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        // 常见于子进程还没来得及写日志：给一句明确说明，别让错误信息在这里断掉
        Err(e) => return format!("（读取失败：{e}）"),
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len > TAIL_BYTES {
        let _ = f.seek(SeekFrom::Start(len - TAIL_BYTES));
    }
    let mut buf = Vec::new();
    let _ = f.take(TAIL_BYTES).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().rev().take(max_lines).collect();
    if lines.is_empty() {
        return "（日志为空）".into();
    }
    lines.into_iter().rev().collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 假探活：只有列出来的 pid 还活着（单测不碰 Win32）
    fn alive(ids: &[u32]) -> impl Fn(u32) -> bool + '_ {
        move |p| ids.contains(&p)
    }
    /// 假全机扫描：只有列出来的 pid 现在真的是 wp8f.exe / test-server.exe
    fn is_exe(known: &[u32]) -> impl Fn(u32) -> bool + '_ {
        move |p| known.contains(&p)
    }

    /// "只杀自己启动的"：归属只在记录 pid 上成立，全机扫描到的别人的 pid 一律不认
    #[test]
    fn owned_pid_only_accepts_our_own_recorded_pid() {
        // 全机扫描到 100（我们的）/200（用户自己开的）；我们记录的是 100 → 认 100
        assert_eq!(owned_pid(Some(100), &alive(&[100, 200]), &is_exe(&[100, 200])), Some(100));
        // 记录的是别人的 pid（不是我们 spawn 出来的）→ 不认
        assert_eq!(owned_pid(Some(200), &alive(&[100, 200]), &is_exe(&[100])), None);
        // 没记录过（本进程没启动过 wp8f）→ 谁都不认，绝不可能去 kill
        assert_eq!(owned_pid(None, &alive(&[100, 200]), &is_exe(&[100, 200])), None);
// 自己那个已退出 → 不认（不反复 kill 不存在的 pid）
        assert_eq!(owned_pid(Some(100), &alive(&[200]), &is_exe(&[200])), None);
        // pid 被复用：记录 100，100 现在活着但已不是 wp8f.exe → 不认（防误杀无关进程）
        assert_eq!(owned_pid(Some(100), &alive(&[100, 200]), &is_exe(&[200])), None);
    }

    /// 预览模式：两个 pid 任一退出就收另一个（双向）
    #[test]
    fn reap_decision_is_two_way_in_drag_mode() {
        // 都活着 → 不动作
        assert_eq!(reap_decision(Some(1), Some(2), &alive(&[1, 2])), Reap::Idle);
        // wp8f 先退出 → 收 test-server（旧行为，保留）
        assert_eq!(reap_decision(Some(1), Some(2), &alive(&[2])), Reap::KillTestServer(2));
        // test-server 先退出 → 收 wp8f（本轮新增方向）
        assert_eq!(reap_decision(Some(1), Some(2), &alive(&[1])), Reap::KillWp8f(1));
        // 两个都已退出 → 没有要保护的另一半
        assert_eq!(reap_decision(Some(1), Some(2), &alive(&[])), Reap::Idle);
    }

    /// 非预览模式（只有一个记录）→ reaper 不动作
    #[test]
    fn reap_decision_idle_without_both_recorded() {
        assert_eq!(reap_decision(Some(1), None, &alive(&[1])), Reap::Idle);
        assert_eq!(reap_decision(Some(1), None, &alive(&[])), Reap::Idle);
        assert_eq!(reap_decision(None, Some(2), &alive(&[2])), Reap::Idle);
        assert_eq!(reap_decision(None, None, &alive(&[])), Reap::Idle);
    }

    /// 点「开 始」（非预览）要先收掉自己启动的预览服务端：它占着 8111，
    /// HUD 连上去读到的会是模拟数据；预览启动则必须留着它当数据源。
    #[test]
    fn preview_server_is_stopped_before_a_plain_launch_only() {
        assert_eq!(preview_server_to_stop(false, Some(7)), Some(7), "非预览：先收掉它");
        assert_eq!(preview_server_to_stop(false, None), None, "本来就没在跑");
        assert_eq!(preview_server_to_stop(true, Some(7)), None, "预览：它正是数据源，不能收");
        assert_eq!(preview_server_to_stop(true, None), None);
    }

/// reaper 的判据不看"全机还有没有 wp8f.exe"（看就会漏杀）。
    #[test]
    fn reap_decision_ignores_foreign_wp8f() {
        // 我们自己没启动过（记录为 None），机器上有用户开的 wp8f（9）→ 依然不动作
        assert_eq!(reap_decision(None, Some(2), &alive(&[9])), Reap::Idle);
        // 自己记录的 1 已退出，机器上还有别人的 wp8f（9）+ 自己的 test-server（2）
        // → 按"自己的 wp8f 已退出"处理：收掉自己的 test-server（别人的 9 不参与判断）
        assert_eq!(reap_decision(Some(1), Some(2), &alive(&[9, 2])), Reap::KillTestServer(2));
    }
}
