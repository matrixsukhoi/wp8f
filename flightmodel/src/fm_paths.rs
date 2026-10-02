//! **数据根解析 + `.blkx` 路径归一**：HUD（`wp8f-core`）与 GUI（`wp8f-gui`）共用这一份实现 ——
//! 两处各写一份时语义还会漂（一个相对 cwd、一个拼绝对路径），漂了就静默读错目录。
//!
//! # 目录布局
//!
//! ```text
//! resource/
//!   data/            ← 当前生效（绝不被就地改写）
//!     gamedata/flightmodels/*.blkx   ← FM 机型（`--data-dir` 指的就是 data/ 这一层）
//!     gamedata/weapons/*.blkx
//!     version                        ← 纯文本一行，如 2.59.0.43
//!   data_new/        ← A/B 分区的另一半：更新器下载到这里
//! ```
//!
//! `data` 与 `data_new` **都算合法数据根**（用户可能想让程序直接读刚下完的 `data_new` 去校验），
//! 顺序是 `data` 优先、`data_new` 兜底。

use std::path::{Path, PathBuf};

/// `resource/`（数据根的父目录）。
pub const RESOURCE_DIR: &str = "resource";
/// 生效数据根目录名。
pub const DATA_DIR_NAME: &str = "data";
/// A/B 分区的另一半：更新器下载目标（校验通过、用户二次确认后才换名成 `data`）。
pub const DATA_NEW_DIR_NAME: &str = "data_new";
/// 数据根下真正放 FM 的子树。
pub const GAMEDATA_DIR: &str = "gamedata";
/// 机型文件所在子目录（相对数据根：`gamedata/flightmodels`）。
pub const FLIGHTMODELS_SUBDIR: &str = "gamedata/flightmodels";
/// 武器文件所在子目录（相对数据根：`gamedata/weapons`）。
pub const WEAPONS_SUBDIR: &str = "gamedata/weapons";
/// 数据库版本文件名（与 `gamedata/` 平级，直接在数据根下）。
pub const VERSION_FILE: &str = "version";
/// 数据根覆盖用的环境变量（测试/便携版用；显式 `--data-dir` 优先于它）。
pub const DATA_ROOT_ENV: &str = "WP8F_DATA_DIR";

/// 一个目录算不算"数据根"：它下面得有 `gamedata/`。
///
/// 故意**不**要求 `gamedata/flightmodels` 存在：更新器刚建好目录、或在下载武器的中间状态下，
/// 上层也该能拿到根路径；真正要用 FM 时再报"缺 flightmodels"的错。
pub fn is_data_root(path: &Path) -> bool {
    path.join(GAMEDATA_DIR).is_dir()
}

/// 数据根 → FM 机型目录（`<root>/gamedata/flightmodels`）。
pub fn flightmodels_dir(data_root: &Path) -> PathBuf {
    data_root.join(GAMEDATA_DIR).join("flightmodels")
}

/// 数据根 → 武器目录（`<root>/gamedata/weapons`）。
pub fn weapons_dir(data_root: &Path) -> PathBuf {
    data_root.join(GAMEDATA_DIR).join("weapons")
}

/// 数据根 → 版本文件（`<root>/version`）。
pub fn version_file(data_root: &Path) -> PathBuf {
    data_root.join(VERSION_FILE)
}

/// 仓库根下的两个候选数据根：`resource/data` 优先、`resource/data_new` 兜底。
pub fn data_root_candidates_in(repo_root: &Path) -> [PathBuf; 2] {
    let resource = repo_root.join(RESOURCE_DIR);
    [
        resource.join(DATA_DIR_NAME),
        resource.join(DATA_NEW_DIR_NAME),
    ]
}

/// 在给定仓库根下解析数据根（`resource/data` 优先，缺失时退到 `resource/data_new`）。
///
/// 两个都不存在 → `Err`（可读信息，不 panic）。
pub fn resolve_data_root_in(repo_root: &Path) -> Result<PathBuf, String> {
    let candidates = data_root_candidates_in(repo_root);
    resolve_from_candidates(&candidates, &format!("仓库根 {}", repo_root.display()))
}

/// **通用数据根解析**（HUD / GUI / CLI 都用它），按顺序：
///
/// 1. `explicit`（`--data-dir`）——给了就必须能用，否则直接报错（不偷偷回退，免得读错目录）；
/// 2. 环境变量 [`DATA_ROOT_ENV`]（同上）；
/// 3. cwd 下的 `resource/data`、`resource/data_new`；
/// 4. exe 所在目录（及其上一级，兼容 `target/<triple>/release/`）下的同样两项。
///
/// 数据是用户本地文件、不在 git 里，所以找不到是常态：返回错误而不 panic、不静默用空数据。
pub fn resolve_data_root(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        if is_data_root(p) {
            return Ok(p.to_path_buf());
        }
        return Err(format!("数据根不可用：{}（没有 {GAMEDATA_DIR}/ 目录）", p.display()));
    }

    if let Some(env) = std::env::var_os(DATA_ROOT_ENV) {
        let p = PathBuf::from(env);
        if is_data_root(&p) {
            return Ok(p);
        }
        return Err(format!("{DATA_ROOT_ENV} 指向的数据根不可用：{}", p.display()));
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.extend(data_root_candidates_in(&cwd));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.extend(data_root_candidates_in(dir));
            if let Some(up) = dir.parent() {
                candidates.extend(data_root_candidates_in(up));
            }
        }
    }
    dedup(&mut candidates);

    resolve_from_candidates(&candidates, "当前工作目录 / 可执行文件附近")
}

fn dedup(paths: &mut Vec<PathBuf>) {
    let mut seen: Vec<PathBuf> = Vec::new();
    paths.retain(|p| {
        if seen.iter().any(|s| s == p) {
            false
        } else {
            seen.push(p.clone());
            true
        }
    });
}

fn resolve_from_candidates(candidates: &[PathBuf], scope: &str) -> Result<PathBuf, String> {
    for c in candidates {
        if is_data_root(c) {
            return Ok(c.clone());
        }
    }
    Err(format!(
        "找不到飞机性能数据根（{scope}）：{}",
        candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" / ")
    ))
}

// ------------------------------------------------------------------ 路径归一 --

/// `fmFile` / `weapon_presets[].blk` 这类**引用** → 磁盘上真实存在的相对路径
///（两条数据共用的唯一入口）。上游数据有两处"名不符实"，这里一并抹平：
///
/// 1. **扩展名**：引用写 `fm/a-20g.blk`、实际文件是 `.blkx` → `.blk`（或无扩展名）补成 `.blkx`；
/// 2. **大小写**：仓库里的 basename 全小写，但引用里带大写 → 整条路径转小写
///    （Windows 大小写不敏感掩盖了这件事，**Linux 上会直接 404**）；
/// 3. 前导 `/`、`\` 与反斜杠分隔符一并归一。
///
/// 这里**不**用 `Path::with_extension`：文件名里带点（如 `su-9m14m.default`）时它会把点后半段
/// 当扩展名替掉，而"补 x"只应当看结尾是不是 `.blk`/`.blkx`。
pub fn normalize_blkx_ref(reference: &str) -> String {
    let cleaned = reference.trim().replace('\\', "/");
    let cleaned = cleaned.trim_start_matches('/');
    let lowered = cleaned.to_lowercase();
    if lowered.ends_with(".blkx") {
        lowered
    } else if let Some(stem) = lowered.strip_suffix(".blk") {
        format!("{stem}.blkx")
    } else if lowered.ends_with('/') || lowered.is_empty() {
        lowered
    } else {
        format!("{lowered}.blkx")
    }
}

/// 归一后的引用去掉 `.blkx` 后缀（给"武器名 = 相对路径"这种显示/键用途，与旧行为一致）。
pub fn strip_blkx_extension(reference: &str) -> String {
    let normalized = normalize_blkx_ref(reference);
    normalized
        .strip_suffix(".blkx")
        .map(|s| s.to_string())
        .unwrap_or(normalized)
}

/// 武器引用 → 相对 `gamedata/weapons/` 的路径（`Weapon::load` 用）。
///
/// 去掉 `gameData/Weapons/`、`gameData/FlightModels/weaponPresets/` 前缀（大小写不敏感），
/// 剩下的交给 [`normalize_blkx_ref`]。
pub fn normalize_weapon_ref(reference: &str) -> String {
    let cleaned = reference.trim().replace('\\', "/");
    let lowered = cleaned.trim_start_matches('/').to_lowercase();
    let stripped = lowered
        .strip_prefix("gamedata/weapons/")
        .or_else(|| lowered.strip_prefix("gamedata/flightmodels/weaponpresets/"))
        .unwrap_or(&lowered);
    normalize_blkx_ref(stripped)
}

/// 武器引用 → 相对 `gamedata/weapons/` 的**无扩展名**路径（`Weapon.name` / 弹链键用的那套写法，
/// 与 `weapon_blk_path.trim_start_matches("gameData/Weapons/")` + 去扩展名 + 小写的旧行为一致）。
pub fn weapon_ref_stem(reference: &str) -> String {
    strip_blkx_extension(&normalize_weapon_ref(reference))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 临时目录（`%TEMP%` / `/tmp` 下）：测试结束删掉，不留垃圾。
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let n = SEQ.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!(
                "wp8f_p2_paths_{}_{}_{}",
                std::process::id(),
                tag,
                n
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("建临时目录");
            TempRoot(p)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// 造一个数据根：`<temp>/resource/<rel>` 下有 `gamedata/flightmodels` + `version`。
        ///
        /// 注意是**仓库根布局**（`resource/data`、`resource/data_new`），因为
        /// [`resolve_data_root_in`] 收的就是仓库根。
        fn make_data_root(&self, rel: &str) -> PathBuf {
            let root = self.0.join(RESOURCE_DIR).join(rel);
            std::fs::create_dir_all(flightmodels_dir(&root)).expect("建 gamedata/flightmodels");
            std::fs::write(version_file(&root), "2.58.0.35\n").expect("写 version");
            root
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // ---------------------------------------------------------- 路径归一 --

    #[test]
    fn blkx_ref_gets_x_suffix() {
        assert_eq!(normalize_blkx_ref("fm/a-20g.blk"), "fm/a-20g.blkx");
        assert_eq!(normalize_blkx_ref("fm/a-20g.blkx"), "fm/a-20g.blkx");
        assert_eq!(normalize_blkx_ref("/fm/a-20g.blk"), "fm/a-20g.blkx");
        assert_eq!(normalize_blkx_ref("fm\\a-20g.blk"), "fm/a-20g.blkx");
        assert_eq!(normalize_blkx_ref("a-20g"), "a-20g.blkx");
        assert_eq!(normalize_blkx_ref("  fm/a-20g.blk  "), "fm/a-20g.blkx");
    }

    #[test]
    fn blkx_ref_is_lowercased() {
        // G10 实测：引用写大写、仓库文件全小写（Windows 掩盖、Linux 404）
        assert_eq!(
            normalize_blkx_ref("gameData/FlightModels/weaponPresets/A-20G_default.blk"),
            "gamedata/flightmodels/weaponpresets/a-20g_default.blkx"
        );
        assert_eq!(
            normalize_weapon_ref("gameData/Weapons/cannonGSh_23L.blk"),
            "cannongsh_23l.blkx"
        );
    }

    #[test]
    fn weapon_ref_strips_both_prefixes() {
        assert_eq!(
            normalize_weapon_ref("gameData/FlightModels/weaponPresets/A-20G_default.blk"),
            "a-20g_default.blkx"
        );
        assert_eq!(
            normalize_weapon_ref("gameData/Weapons/rocketGuns/su_9m14m_default.blk"),
            "rocketguns/su_9m14m_default.blkx"
        );
        assert_eq!(normalize_weapon_ref("gunBrowning50"), "gunbrowning50.blkx");
    }

    #[test]
    fn stem_keeps_dots_inside_name() {
        // `Path::with_extension` 会把 `.default` 当扩展名换掉；这里只补结尾的 `x`
        assert_eq!(normalize_blkx_ref("guns/su-9m14m.default"), "guns/su-9m14m.default.blkx");
        assert_eq!(strip_blkx_extension("guns/su-9m14m.default"), "guns/su-9m14m.default");
    }

    #[test]
    fn strip_extension_matches_old_trim_behaviour() {
        // 旧代码：trim ".blk" 再去 ".blkx" + to_lowercase → 这里锁住等价性
        for r in [
            "gameData/Weapons/cannonGSh_23L.blk",
            "gameData/Weapons/cannonGSh_23L.blkx",
            "rocketGuns/su_9m14m_default.blk",
        ] {
            let rel = r
                .trim_start_matches("gameData/Weapons/")
                .trim_end_matches(".blk")
                .trim_end_matches(".blkx")
                .to_lowercase();
            let now = normalize_weapon_ref(r);
            let now = now.strip_suffix(".blkx").unwrap_or(&now);
            assert_eq!(now, rel, "引用 {r} 的旧/新归一结果必须一致");
        }
    }

    // ---------------------------------------------------------- 数据根 --

    #[test]
    fn data_root_accepts_data_and_data_new() {
        for name in [DATA_DIR_NAME, DATA_NEW_DIR_NAME] {
            let t = TempRoot::new("root");
            let root = t.make_data_root(name);
            assert!(is_data_root(&root), "{name} 应当算合法数据根");
            let resolved = resolve_data_root_in(t.path()).expect("应当解析成功");
            assert_eq!(resolved, root);
            assert!(flightmodels_dir(&resolved).is_dir());
            assert_eq!(version_file(&resolved), root.join("version"));
        }
    }

    #[test]
    fn data_wins_over_data_new_when_both_exist() {
        let t = TempRoot::new("both");
        let data = t.make_data_root(DATA_DIR_NAME);
        let _new = t.make_data_root(DATA_NEW_DIR_NAME);
        assert_eq!(resolve_data_root_in(t.path()).unwrap(), data);
    }

    #[test]
    fn missing_root_is_readable_error_not_panic() {
        let t = TempRoot::new("missing");
        assert!(resolve_data_root_in(t.path()).is_err());
    }

    #[test]
    fn explicit_root_must_be_usable() {
        let t = TempRoot::new("explicit");
        let root = t.make_data_root(DATA_NEW_DIR_NAME);
        // 显式给了就用它（即便 data/ 不存在）—— A/B 分区里 data_new 也要能直接跑
        assert_eq!(resolve_data_root(Some(&root)).unwrap(), root);
        // 显式给错 → 报错（不回退到别处）
        assert!(resolve_data_root(Some(&t.path().join("nope"))).is_err());
    }

    #[test]
    fn relative_candidates_are_repo_root_relative() {
        // 仓库根可以是相对的（`.`）也可以是绝对的：候选路径都只是 root 下拼出来的
        let cands = data_root_candidates_in(Path::new("."));
        assert_eq!(cands[0], Path::new(".").join(RESOURCE_DIR).join(DATA_DIR_NAME));
        assert_eq!(cands[1], Path::new(".").join(RESOURCE_DIR).join(DATA_NEW_DIR_NAME));

        let t = TempRoot::new("rel");
        let root = t.make_data_root(DATA_DIR_NAME);
        assert!(is_data_root(&root), "绝对路径也要能识别");
        assert!(resolve_data_root_in(&root).is_err(), "数据根本身不是仓库根");
    }
}
