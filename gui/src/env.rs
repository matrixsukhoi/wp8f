//! 本地环境探测与 flightmodel 调用（合并成单进程后不再需要跨进程 HTTP 客户端）。
use serde_json::Value;
use std::path::Path;
use std::process::Command;

// ---------------------------------------------------------------- 环境探测 --

/// HUD 主程序。**S1 布局**：仓库根只放用户双击的入口 `wp8f-gui.exe`，
/// 其余可执行文件（wp8f / flightmodel / test-server）都在 `binary/` 下。
pub fn win_exe(root: &Path) -> std::path::PathBuf {
    root.join("binary").join("wp8f.exe")
}

/// FM 解析/曲线可执行文件（与 HUD 同在 `binary/`）。
pub fn fm_exe(root: &Path) -> std::path::PathBuf {
    root.join("binary").join("flightmodel.exe")
}

/// 飞机性能数据根目录（= flightmodel 的 `--data-dir`）。
/// **用户本地数据、不在 git 里**：仓库只跟踪 `resource/fonts` 与 `resource/voice`。
/// 2026-10 起数据根**扁平化**成 **`resource/data`**：`gamedata/`、`weapons/`、`version`
/// 都直接在它下面（原 `resource/data/aces/` 整体提上来一级，更早是 `resource/fm/data`）。
/// 不存在旧路径回退 —— 留在老位置就读不到，错误信息会直接指路。
///
/// 解析走 **`wp8f_flightmodel::resolve_data_root_in`**（与 HUD 同一份实现）：
/// `resource/data` 优先，缺失时用更新器 A/B 分区的暂存根 **`resource/data_new`**。
pub fn fm_data_dir(root: &Path) -> std::path::PathBuf {
    wp8f_flightmodel::resolve_data_root_in(root)
        .unwrap_or_else(|_| root.join("resource").join("data"))
}

/// flightmodel 真正读的那一层（`<data-dir>/gamedata/flightmodels`）。
pub fn fm_dir(root: &Path) -> std::path::PathBuf {
    wp8f_flightmodel::flightmodels_dir(&fm_data_dir(root))
}

/// FM 数据库版本文件（datamine 产物自带，**纯文本一行**，如 `2.58.0.35`）。
/// 扁平化后它与 `gamedata/` 平级，直接在数据根下：`resource/data/version`
/// （A/B 分区的暂存根则是 `resource/data_new/version`）。
pub fn fm_version_file(root: &Path) -> std::path::PathBuf {
    wp8f_flightmodel::data_version_file(&fm_data_dir(root))
}

/// FM 数据库版本 = 本地数据里 `version` 文件的内容（首行非空文本）。
/// 文件不存在 / 内容不像版本号 → `None`：**宁可显示"未知"也不编造版本号**。
pub fn fm_db_version(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(fm_version_file(root)).ok()?;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let ok = line.len() <= 40
        && line
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'));
    if ok {
        Some(line.to_string())
    } else {
        None
    }
}

/// 调用 flightmodel 并抽出它混在日志里的 JSON（流式解析，不靠花括号配平）
pub fn run_flightmodel(
    root: &Path,
    aircraft: &str,
    mode: &[&str],
    fuel_pct: f64,
    extra_weight: f64,
) -> Result<Value, (u16, String)> {
    let exe = fm_exe(root);
    if !exe.is_file() {
        return Err((500, format!("flightmodel 可执行文件不存在: {}（请先构建）", exe.display())));
    }
    // 显式传 --data-dir：不再依赖子进程 cwd + CLI 默认值（两处默认值一旦漂移就会静默读错目录）。
    // 找不到时直接给指路错误，不回退。
    let data_dir = fm_data_dir(root);
    let fm_aircraft_dir = fm_dir(root);
    if !fm_aircraft_dir.is_dir() {
        return Err((
            500,
            format!(
                "飞机性能数据目录不存在: {}（它是本地数据、不在 git 里；请放到 resource/data/gamedata/ 下）",
                fm_aircraft_dir.display()
            ),
        ));
    }
    let fuel = fuel_pct.clamp(0.0, 100.0);
    let extra = extra_weight.clamp(0.0, 100000.0);
    let mut cmd = Command::new(&exe);
    cmd.arg("--aircraft").arg(aircraft).arg("--data-dir").arg(&data_dir);
    for m in mode {
        cmd.arg(m);   // 每个参数独立传（"--format json" 不能当一个参数）
    }
    cmd.arg("--fuel-pct").arg(format!("{:.1}", fuel))
        .arg("--extra-weight").arg(format!("{:.1}", extra));
    // 关键：本进程是 GUI（windows）子系统，启动控制台子程序时必须显式 CREATE_NO_WINDOW，
    // 否则 Windows 会给子进程分配一个新的控制台窗口 —— 表现就是拖动燃油条时闪黑框。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let out = match cmd.output() {
        Ok(o) => o,
        Err(e) => return Err((500, format!("flightmodel 调用失败: {e}"))),
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    match extract_json(&stdout) {
        Some(v) => Ok(v),
        None => {
            if stderr.contains("Error:") {
                Err((404, format!("机型不存在或解析失败: {aircraft}")))
            } else {
                let tail: String = stdout.chars().rev().take(500).collect::<Vec<_>>()
                    .into_iter().rev().collect();
                Err((500, format!("输出中未找到 JSON: {tail}")))
            }
        }
    }
}

/// 从混合输出里取第一个 JSON 值（后续日志被忽略）
pub fn extract_json(text: &str) -> Option<Value> {
    use serde::Deserialize;
    let start = text.find('{')?;
    let mut de = serde_json::Deserializer::from_str(&text[start..]);
    Value::deserialize(&mut de).ok()
}
