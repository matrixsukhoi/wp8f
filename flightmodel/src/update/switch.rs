//! A/B 分区切换：**校验全部通过 + 用户确认**之后才动的三步改名 + 失败回滚（D4/D11/D31/P9）。
//!
//! ```text
//! resource/
//!   data/        ← 当前生效（在切换成功前一个字节都不动）
//!   data_new/    ← 暂存（下载 + 校验完成的另一半）
//!   data_old/    ← **上一版**（固定单槽，回滚源；用户可以直接删掉它腾空间）
//! ```
//!
//! 步骤（**固定单槽 `data_old`**，只保留上一版一份，用户可随时手动删除）：
//! 1. `resource/data_old` 已存在 → **先删掉它**（只保留上一版一份）；
//! 2. `rename(data → data_old)`；
//! 3. `rename(data_new → data)`；**这一步失败 → 把 `data_old` 改回 `data`**。
//!
//! **"删掉旧数据文件夹"就是第 2 步**（旧数据从 `resource/data` 这个位置上消失），
//! 但目录本身改名为 `data_old` 留着 —— 直接 `remove_dir_all` 就没有回滚源了，
//! 第 3 步一失败数据就彻底没了。回滚 = 把两者名字换回来（单槽方案下只有这一种情形，很简单）。
//! 再上一版不保留：**只有一份 `data_old`**，由用户随时手动删除（界面文案里写明）。
//!
//! **任一步失败都不得让 `resource/data` 处于缺失状态**：
//! * 第 1 步失败 → `data` 一个字节没动；
//! * 第 2 步失败 → rename 是原子的，`data` 原样还在；
//! * 第 3 步失败 → 把 `data_old` 改回 `data`（失败时错误消息里给出两个目录名，手工换回来即可）。
//!
//! Windows 上"目录被占用"（HUD 正在读盘 / 资源管理器开着）会让 rename 失败并报
//! `ERROR_ACCESS_DENIED`/`ERROR_SHARING_VIOLATION` → 归类成**权限不足**并原样回滚；
//! 所以调用方（`UpdateManager::apply_switch`）会**先拒绝"HUD 正在跑"的情形**，
//! 而不是替用户把 HUD 杀掉。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::fm_paths::{DATA_DIR_NAME, DATA_NEW_DIR_NAME, VERSION_FILE};
use super::net::{self, ErrorKind, UpdateError};

/// 旧目录名（**固定单槽**，只保留上一版一份，由用户随时手动删除）。
pub const OLD_DIR_NAME: &str = "data_old";

/// 一次切换的路径计划。
#[derive(Debug, Clone)]
pub struct SwitchPlan {
    /// 当前生效的数据根（`resource/data`）
    pub data_root: PathBuf,
    /// 暂存根（`resource/data_new`）
    pub staging_root: PathBuf,
    /// 上一版数据根（`resource/data_old`，固定名字）
    pub old_root: PathBuf,
}

impl SwitchPlan {
    /// 按 `resource/` 目录构造。`stamp` **已不再使用**（保留参数只为少改调用点，
    /// 单槽方案下目录名不含时间戳）。
    pub fn new(resource_dir: &Path, _stamp: u64) -> Self {
        Self {
            data_root: resource_dir.join(DATA_DIR_NAME),
            staging_root: resource_dir.join(DATA_NEW_DIR_NAME),
            old_root: resource_dir.join(OLD_DIR_NAME),
        }
    }

    /// 用当前时间构造（`stamp` 只是历史参数）。
    pub fn now(resource_dir: &Path) -> Self {
        Self::new(resource_dir, unix_now())
    }
}

/// 当前 unix 秒（拿不到系统时间时退化成 0 —— 只影响目录名，不影响正确性）。
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 数据根里的版本号（`resource/data/version`，纯文本一行）。
pub fn read_version(data_root: &Path) -> Option<String> {
    let text = fs::read_to_string(data_root.join(VERSION_FILE)).ok()?;
    super::manifest::first_line(&text)
}

/// 切换前的前置检查：**任何一条不满足都不动文件**。
pub fn preflight(plan: &SwitchPlan, expect_version: &str) -> Result<Vec<String>, UpdateError> {
    let mut notes = Vec::new();
    if !plan.staging_root.is_dir() {
        return Err(UpdateError::verify(format!(
            "暂存目录不存在：{}（先完成一次下载与校验）",
            plan.staging_root.display()
        )));
    }
    let staged = read_version(&plan.staging_root).ok_or_else(|| {
        UpdateError::verify(format!(
            "暂存目录里没有 `{VERSION_FILE}`（说明下载/校验还没全部完成）：{}",
            plan.staging_root.display()
        ))
    })?;
    if staged != expect_version {
        return Err(UpdateError::verify(format!(
            "暂存目录的版本号是 {staged}，不是本次目标 {expect_version}（拒绝切换）"
        )));
    }
    // 单槽方案：`data_old` 存在不是"冲突"，而是第 1 步要删掉的东西（只保留上一版一份）
    if plan.old_root.is_dir() {
        notes.push(format!("上一版目录 {} 会在第一步被删除", plan.old_root.display()));
    }
    notes.push(format!("暂存版本 {staged} 校验通过"));
    Ok(notes)
}

/// 切换时**不保留**的条目：当前数据根里有、暂存根里没有的顶层项。
///
/// 只有 `flightmodels` + `weapons` 两个子树是更新器负责的（D4），所以换根之后
/// 别的顶层目录（`levels/`、`templates/`、`config/`、`nm/`…）不会出现在新数据根里。
/// 旧目录会整体留在 `data_old`（回滚源），但**用户必须提前知道这件事**。
pub fn dropped_top_level(plan: &SwitchPlan) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(&plan.data_root) else {
        return out;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if !plan.staging_root.join(&name).exists() {
            out.push(name);
        }
    }
    out.sort();
    out
}

/// 执行切换（**三步**改名 + 失败回滚）。返回 `[UPDATE]` 日志行。
pub fn apply_switch(plan: &SwitchPlan, expect_version: &str) -> Result<Vec<String>, UpdateError> {
    apply_switch_inner(plan, expect_version, None)
}

/// 测试用的失败注入：`fail_at` = 在哪一步**之后**人为失败
/// （1 = 删掉旧 `data_old` 之后，2 = `data → data_old` 之后）。
#[cfg(test)]
pub fn apply_switch_failing_at(
    plan: &SwitchPlan,
    expect_version: &str,
    fail_at: u8,
) -> Result<Vec<String>, UpdateError> {
    apply_switch_inner(plan, expect_version, Some(fail_at))
}

fn apply_switch_inner(
    plan: &SwitchPlan,
    expect_version: &str,
    fail_at: Option<u8>,
) -> Result<Vec<String>, UpdateError> {
    let mut log = preflight(plan, expect_version)?;
    let had_data = plan.data_root.is_dir();

    // ---- 第 1 步：删掉上一版（只保留一份 `data_old`）----
    if plan.old_root.exists() {
        fs::remove_dir_all(&plan.old_root).map_err(|e| {
            net::classify_io(
                &e,
                &format!(
                    "无法删除上一版数据目录 {}（HUD/资源管理器/杀软可能正占着它）",
                    plan.old_root.display()
                ),
            )
        })?;
        log.push(format!("已删除上一版数据 {}", plan.old_root.display()));
    }
    if fail_at == Some(1) {
        return Err(UpdateError::new(
            ErrorKind::Io,
            "注入的切换失败（第一步之后）—— 当前数据根一个字节没动".to_string(),
        ));
    }

    // ---- 第 2 步：`data → data_old`（失败时 data 原样还在：rename 是原子的）----
    if had_data {
        fs::rename(&plan.data_root, &plan.old_root).map_err(|e| {
            let err = net::classify_io(
                &e,
                &format!(
                    "无法把 {} 改名为 {}（HUD/资源管理器可能正占着它）",
                    plan.data_root.display(),
                    plan.old_root.display()
                ),
            );
            if err.kind == ErrorKind::Permission {
                UpdateError::new(
                    ErrorKind::Permission,
                    format!("{}；请先关闭 HUD（wp8f.exe）再试", err.message),
                )
            } else {
                err
            }
        })?;
        log.push(format!("旧数据已改名为 {}", plan.old_root.display()));
    } else {
        log.push("当前没有生效的数据根（首次安装数据）".to_string());
    }

    if fail_at == Some(2) {
        // 测试注入：模拟"第三步失败"（磁盘满/权限/被杀），走回滚
        let rollback = rollback(&plan.old_root, &plan.data_root, had_data);
        return Err(UpdateError::new(
            ErrorKind::Io,
            format!(
                "注入的切换失败（第二步之后）{}",
                if rollback { "；已回滚（data_old → data）" } else { "；回滚也失败" }
            ),
        ));
    }

    // ---- 第 3 步：`data_new → data`；失败 → `data_old → data` 回滚 ----
    match fs::rename(&plan.staging_root, &plan.data_root) {
        Ok(()) => {
            log.push(format!(
                "新数据已生效：{}（版本 {expect_version}）",
                plan.data_root.display()
            ));
            log.push(format!(
                "上一版仍在 {}（确认新数据没问题后可以直接删掉它释放空间）",
                plan.old_root.display()
            ));
            Ok(log)
        }
        Err(e) => {
            let err = net::classify_io(
                &e,
                &format!(
                    "无法把 {} 改名为 {}",
                    plan.staging_root.display(),
                    plan.data_root.display()
                ),
            );
            let rolled = rollback(&plan.old_root, &plan.data_root, had_data);
            Err(UpdateError::new(
                err.kind,
                format!(
                    "切换失败：{}；{}",
                    err.message,
                    if rolled {
                        "已把上一版原样改回 resource/data，现有数据没有被破坏"
                    } else {
                        "⚠ 回滚失败：上一版还在 resource/data_old，把它改名回 resource/data 即可恢复"
                    }
                ),
            ))
        }
    }
}

/// 回滚：把 `data_old` 改回数据根。返回是否成功（没有旧目录时视为"无需回滚"= true）。
fn rollback(old_root: &Path, data_root: &Path, had_data: bool) -> bool {
    if !had_data || !old_root.is_dir() {
        return true;
    }
    fs::rename(old_root, data_root).is_ok()
}

/// 上一版数据目录是否存在（界面显示"可以直接删掉它释放空间"）。
pub fn has_old_root(resource_dir: &Path) -> bool {
    resource_dir.join(OLD_DIR_NAME).is_dir()
}

/// 上一版数据目录的路径（界面/文案里显示用）。
pub fn old_root_path(resource_dir: &Path) -> PathBuf {
    resource_dir.join(OLD_DIR_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 临时 `resource/` 目录（测试结束删干净）。
    struct TempResource(PathBuf);

    impl TempResource {
        fn new(tag: &str) -> Self {
            let n = SEQ.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("wp8f_p3_switch_{}_{tag}_{n}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).expect("建临时 resource");
            TempResource(p)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// 造一个数据根：`gamedata/flightmodels/<name>` + `version`
        fn make_root(&self, dir: &str, version: &str, marker: &str) -> PathBuf {
            let root = self.0.join(dir);
            fs::create_dir_all(root.join("gamedata/flightmodels")).expect("建目录");
            fs::write(root.join("gamedata/flightmodels/a-20g.blkx"), br#"{"model":"a_20"}"#).unwrap();
            fs::write(root.join(VERSION_FILE), format!("{version}\n")).unwrap();
            fs::write(root.join("MARKER"), marker).unwrap();
            root
        }
    }

    impl Drop for TempResource {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// **正常三步**：删旧 `data_old`（若有）→ `data → data_old` → `data_new → data`。
    #[test]
    fn switch_swaps_roots_and_keeps_old_for_rollback() {
        let t = TempResource::new("ok");
        let _old_dir = t.make_root(DATA_DIR_NAME, "2.58.0.35", "old");
        let staging = t.make_root(DATA_NEW_DIR_NAME, "2.59.0.43", "new");
        let plan = SwitchPlan::new(t.path(), 1_700_000_000);
        let log = apply_switch(&plan, "2.59.0.43").expect("切换应成功");
        assert!(log.iter().any(|l| l.contains("旧数据已改名")), "{log:?}");
        assert!(log.iter().any(|l| l.contains("新数据已生效")), "{log:?}");
        assert!(!staging.exists(), "暂存目录应已被改名");
        assert_eq!(read_version(&plan.data_root).as_deref(), Some("2.59.0.43"));
        assert_eq!(fs::read_to_string(plan.data_root.join("MARKER")).unwrap(), "new");
        // 单槽：名字固定 `data_old`（**不含时间戳**）
        assert_eq!(plan.old_root, t.path().join(OLD_DIR_NAME));
        assert_eq!(read_version(&plan.old_root).as_deref(), Some("2.58.0.35"), "上一版完整保留");
        assert_eq!(fs::read_to_string(plan.old_root.join("MARKER")).unwrap(), "old");
        assert!(has_old_root(t.path()), "状态里应报出「有上一版」");
    }

    /// **第 1 步：已存在的 `data_old` 先被删掉**（只保留上一版一份，不留时间戳目录）。
    #[test]
    fn existing_old_is_deleted_first_and_only_one_old_remains() {
        let t = TempResource::new("one-slot");
        // 上一轮切换留下的 data_old（版本更旧）+ 上一版 data
        let stale = t.make_root(OLD_DIR_NAME, "2.57.0.1", "stale");
        let _data = t.make_root(DATA_DIR_NAME, "2.58.0.35", "old");
        let _staging = t.make_root(DATA_NEW_DIR_NAME, "2.59.0.43", "new");
        let plan = SwitchPlan::new(t.path(), 1_700_000_010);
        let log = apply_switch(&plan, "2.59.0.43").expect("切换应成功");
        assert!(log.iter().any(|l| l.contains("已删除上一版数据")), "{log:?}");
        // `data_old` 这个名字下必须是**刚换下来的那一版**，更旧那份（MARKER=stale）已删
        assert_ne!(
            fs::read_to_string(plan.old_root.join("MARKER")).unwrap(),
            "stale",
            "上一轮留下的更旧那份必须被第一步删掉（同一个名字下换成刚换下来的那一版）"
        );
        let _ = &stale;
        assert_eq!(read_version(&plan.old_root).as_deref(), Some("2.58.0.35"),
                   "data_old 里只能是刚刚换下来的那一版");
        // 目录数：resource/ 下只有 data + data_old（+ 没有 data_new）
        let dirs: Vec<String> = fs::read_dir(t.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        let mut dirs = dirs;
        dirs.sort();
        assert_eq!(dirs, vec![DATA_DIR_NAME.to_string(), OLD_DIR_NAME.to_string()], "{dirs:?}");
    }

    /// **第 1 步失败**（`data_old` 删不掉）→ `data` 一个字节没动。
    #[test]
    fn failing_to_delete_old_leaves_current_data_untouched() {
        let t = TempResource::new("fail-del");
        let data = t.make_root(DATA_DIR_NAME, "2.58.0.35", "old");
        let _staging = t.make_root(DATA_NEW_DIR_NAME, "2.59.0.43", "new");
        // 让 data_old 是**文件**而不是目录：`remove_dir_all` 会失败（跨平台稳定）
        fs::write(t.path().join(OLD_DIR_NAME), b"not a dir").unwrap();
        let plan = SwitchPlan::new(t.path(), 1_700_000_011);
        let err = apply_switch(&plan, "2.59.0.43").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Io, "{err}");
        assert!(data.is_dir(), "当前数据根必须原样还在");
        assert_eq!(read_version(&plan.data_root).as_deref(), Some("2.58.0.35"));
        assert!(plan.staging_root.is_dir(), "暂存目录还在，可以再试一次切换");
    }

    /// **第 3 步失败**（注入在第 2 步之后）→ `data_old` 改回 `data`，数据可读。
    #[test]
    fn failed_third_rename_rolls_back_and_keeps_data_readable() {
        let t = TempResource::new("rollback");
        let data = t.make_root(DATA_DIR_NAME, "2.58.0.35", "old");
        let _staging = t.make_root(DATA_NEW_DIR_NAME, "2.59.0.43", "new");
        let plan = SwitchPlan::new(t.path(), 1_700_000_001);
        let err = apply_switch_failing_at(&plan, "2.59.0.43", 2).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Io, "{err}");
        // 关键断言：现有数据根**一个字节都没坏**
        assert!(data.is_dir(), "回滚后上一版必须回到 data/");
        assert_eq!(read_version(&plan.data_root).as_deref(), Some("2.58.0.35"));
        assert_eq!(fs::read_to_string(plan.data_root.join("MARKER")).unwrap(), "old");
        assert!(!plan.old_root.exists(), "回滚后不该留下 data_old");
        assert!(plan.staging_root.is_dir(), "暂存目录还在，可以再试一次切换");
    }

    /// **切换过程中 `resource/data` 从不处于缺失状态**（单槽方案的核心不变量）：
    /// 三步各注入一次失败，每次都要求"要么还是旧数据、要么已经是新数据"，不能两头都没有。
    #[test]
    fn data_root_is_never_missing_after_a_failed_switch() {
        for fail_at in [1u8, 2] {
            let t = TempResource::new(&format!("never-missing-{fail_at}"));
            let _data = t.make_root(DATA_DIR_NAME, "2.58.0.35", "old");
            let _staging = t.make_root(DATA_NEW_DIR_NAME, "2.59.0.43", "new");
            let plan = SwitchPlan::new(t.path(), 1_700_000_020 + fail_at as u64);
            let _ = apply_switch_failing_at(&plan, "2.59.0.43", fail_at);
            assert!(plan.data_root.is_dir(), "失败注入 {fail_at}：resource/data 不许缺失");
            let v = read_version(&plan.data_root);
            assert!(
                v.as_deref() == Some("2.58.0.35") || v.as_deref() == Some("2.59.0.43"),
                "失败注入 {fail_at}：data 必须是旧版或新版之一，实测 {v:?}"
            );
        }
    }

    #[test]
    fn preflight_refuses_incomplete_or_mismatched_staging() {
        let t = TempResource::new("preflight");
        let _data = t.make_root(DATA_DIR_NAME, "2.58.0.35", "old");
        let plan = SwitchPlan::new(t.path(), 1_700_000_002);
        // 暂存目录不存在
        let err = preflight(&plan, "2.59.0.43").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Verify, "{err}");
        // 暂存目录存在但没有 version（= 下载/校验没完成）
        fs::create_dir_all(plan.staging_root.join("gamedata/flightmodels")).unwrap();
        let err = preflight(&plan, "2.59.0.43").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Verify);
        // 版本号对不上
        fs::write(plan.staging_root.join(VERSION_FILE), "2.58.0.35\n").unwrap();
        let err = preflight(&plan, "2.59.0.43").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Verify, "{err}");
        // `data_old` 已存在**不是**错误（单槽方案第 1 步会删掉它），但要在 notes 里说清楚
        fs::write(plan.staging_root.join(VERSION_FILE), "2.59.0.43\n").unwrap();
        fs::create_dir_all(&plan.old_root).unwrap();
        let notes = preflight(&plan, "2.59.0.43").expect("data_old 存在不该拒绝切换");
        assert!(notes.iter().any(|n| n.contains("会在第一步被删除")), "{notes:?}");
    }

    #[test]
    fn first_install_without_data_root_works() {
        let t = TempResource::new("first");
        let _staging = t.make_root(DATA_NEW_DIR_NAME, "2.59.0.43", "new");
        let plan = SwitchPlan::new(t.path(), 1_700_000_003);
        let log = apply_switch(&plan, "2.59.0.43").expect("首次安装也应成功");
        assert!(log.iter().any(|l| l.contains("首次安装数据")), "{log:?}");
        assert_eq!(read_version(&plan.data_root).as_deref(), Some("2.59.0.43"));
        assert!(!plan.old_root.exists(), "首次安装没有上一版");
        assert!(!has_old_root(t.path()));
    }

    #[test]
    fn dropped_top_level_reports_what_the_new_root_lacks() {
        let t = TempResource::new("dropped");
        let data = t.make_root(DATA_DIR_NAME, "2.58.0.35", "old");
        // 旧数据根里还有几个 wp8f 不用的顶层目录（真实数据就是这样）
        for extra in ["levels", "templates", "nm"] {
            fs::create_dir_all(data.join(extra)).unwrap();
        }
        let staging = t.make_root(DATA_NEW_DIR_NAME, "2.59.0.43", "new");
        fs::remove_file(staging.join("MARKER")).unwrap();
        let plan = SwitchPlan::new(t.path(), 1);
        let dropped = dropped_top_level(&plan);
        assert_eq!(dropped, vec!["MARKER", "levels", "nm", "templates"]);
    }
}
