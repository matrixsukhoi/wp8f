//! 更新器的离线单测（本地假源，不碰真网络；见 `mock.rs`）。
//!
//! 覆盖：版本比较、清单解析、下载/校验/续传/多源回退/取消、
//! **"失败不破坏现有 resource/data"** 回归、以及切换与回滚。
//! （版本号、清单解析、切换回滚的细项单测各自在对应模块文件里。）

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::download::{self, DownloadConfig};
use super::manifest::{self, Source, SourceKind, Subset};
use super::mock::MockSource;
use super::net::{HttpTransport, Transport};
use super::switch;
use super::{Phase, UpdateManager, UpdatePaths};
use crate::fm_paths::{DATA_DIR_NAME, DATA_NEW_DIR_NAME, VERSION_FILE};

const TAG: &str = "2.59.0.43";
const REPO: &str = "gszabi99/War-Thunder-Datamine";
/// 上游仓库根的版本文件名（与本地 `resource/data/version` 同名）。
const VERSION_PATH: &str = manifest::VERSION_PATH;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// 一个临时仓库根（`resource/data` 已按"旧数据"造好），Drop 时删干净。
struct TempRepo(PathBuf);

impl TempRepo {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("wp8f_p3_it_{}_{tag}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("建临时仓库根");
        let me = TempRepo(p);
        me.write_old_data("2.58.0.35");
        me
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn paths(&self) -> UpdatePaths {
        UpdatePaths::from_repo_root(&self.0)
    }

    /// 造一份"现有数据"：两个子集各一个文件 + version + 一个 wp8f 不用的顶层目录。
    fn write_old_data(&self, version: &str) {
        let root = self.0.join("resource").join(DATA_DIR_NAME);
        let fm = root.join("gamedata/flightmodels");
        let wp = root.join("gamedata/weapons");
        std::fs::create_dir_all(&fm).unwrap();
        std::fs::create_dir_all(&wp).unwrap();
        std::fs::write(fm.join("old-plane.blkx"), br#"{"model":"old_plane"}"#).unwrap();
        std::fs::write(wp.join("old-gun.blkx"), br#"{"model":"old_gun"}"#).unwrap();
        std::fs::write(root.join(VERSION_FILE), format!("{version}\n")).unwrap();
        std::fs::create_dir_all(root.join("levels")).unwrap();
        std::fs::write(root.join("levels/x.blk"), b"level").unwrap();
    }

    fn data_root(&self) -> PathBuf {
        self.0.join("resource").join(DATA_DIR_NAME)
    }

    fn staging_root(&self) -> PathBuf {
        self.0.join("resource").join(DATA_NEW_DIR_NAME)
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 上游文件样本：3 个 JSON 文件（名字与 `mock` 里的 URL 一一对应）。
fn upstream_files() -> Vec<(&'static str, String)> {
    vec![
        ("fm/a-20g.blkx", r#"{"model":"a_20","fmFile":"fm/a-20g.blk"}"#.to_string()),
        ("a-20g.blkx", r#"{"model":"a_20","mass":6000}"#.to_string()),
        ("weapons/gun12.blkx", r#"{"model":"gun12","mass":120}"#.to_string()),
    ]
}

/// 在假源上注册：版本文件 + 所有数据文件 + `contents`/`trees` 两个清单接口。
fn register_source(mock: &MockSource, tag: &str, files: &[(&str, String)]) {
    // 版本文件（仓库根）：github.com 的 `/raw/` 直链 与 api 两条路径都注册
    //（P9 起没有 jsDelivr 那条 `@tag` 形态了）
    mock.put(
        &format!("/{REPO}/raw/master/{VERSION_PATH}"),
        format!("{tag}\n").into_bytes(),
    );
    mock.put(
        &format!("/repos/{REPO}/contents/{VERSION_PATH}"),
        format!("{tag}\n").into_bytes(),
    );

    // 数据文件：github.com 的 raw 直链形态 + **每个文件自己的 blob sha**（api 源要用 sha 取内容，
    // 所以 sha 不能所有文件共用一个）
    for (i, (rel, body)) in files.iter().enumerate() {
        let upstream = format!("{}/{}", Subset::FlightModels.upstream_rel(), rel);
        let (upstream, sha) = if rel.starts_with("weapons/") {
            (format!("{}/{}", Subset::Weapons.upstream_rel(), rel.trim_start_matches("weapons/")),
             format!("sha_wp{i}"))
        } else {
            (upstream, format!("sha_fm{i}"))
        };
        mock.put(&format!("/{REPO}/raw/{tag}/{upstream}"), body.as_bytes().to_vec());
        mock.put(&format!("/repos/{REPO}/git/blobs/{sha}"), body.as_bytes().to_vec());
    }

    // 清单：contents（1 次）+ trees（每个子树 1 次）
    mock.put(
        &format!("/repos/{REPO}/contents/{}", manifest::UPSTREAM_GAMEDATA),
        format!(
            r#"[{{"name":"flightmodels","type":"dir","sha":"fmsha"}},
                {{"name":"weapons","type":"dir","sha":"wpsha"}}]"#
        )
        .as_bytes()
        .to_vec(),
    );
    let tree = |entries: &[(String, String, usize)]| {
        let items: Vec<String> = entries
            .iter()
            .map(|(path, sha, size)| {
                format!(r#"{{"path":"{path}","mode":"100644","type":"blob","sha":"{sha}","size":{size}}}"#)
            })
            .collect();
        format!(r#"{{"sha":"x","truncated":false,"tree":[{}]}}"#, items.join(","))
    };
    let fm_entries: Vec<(String, String, usize)> = files
        .iter()
        .enumerate()
        .filter(|(_, (r, _))| !r.starts_with("weapons/"))
        .map(|(i, (r, b))| (r.to_string(), format!("sha_fm{i}"), b.len()))
        .collect();
    let wp_entries: Vec<(String, String, usize)> = files
        .iter()
        .enumerate()
        .filter(|(_, (r, _))| r.starts_with("weapons/"))
        .map(|(i, (r, b))| (r.trim_start_matches("weapons/").to_string(), format!("sha_wp{i}"), b.len()))
        .collect();
    mock.put_json(&format!("/repos/{REPO}/git/trees/fmsha"), &tree(&fm_entries));
    mock.put_json(&format!("/repos/{REPO}/git/trees/wpsha"), &tree(&wp_entries));
}

/// 管理器：传输 = 生产实现的 http 分支（指向假源），源顺序也按生产顺序
///（P9：只有 github.com 原站 + api.github.com 两条）。
fn manager(repo: &TempRepo, mock: &MockSource) -> Arc<UpdateManager> {
    let base = mock.base();
    let sources = vec![
        Source::with_base(SourceKind::GithubRaw, base.clone()),
        Source::with_base(SourceKind::GithubApi, base.clone()),
    ];
    let m = UpdateManager::with_transport(repo.paths(), Box::new(HttpTransport::new()), sources, &base);
    m
}

/// 轮询状态直到条件满足（默认 20s）。
fn wait_status(m: &UpdateManager, want: impl Fn(&super::Status) -> bool, budget: Duration) -> super::Status {
    let deadline = Instant::now() + budget;
    loop {
        let st = m.status();
        if want(&st) {
            return st;
        }
        if Instant::now() > deadline {
            panic!(
                "等状态超时：phase={:?} files={}/{} error={:?}",
                st.phase, st.files_done, st.files_total, st.error
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn settled(m: &UpdateManager) -> super::Status {
    wait_status(m, |s| !s.phase.busy(), Duration::from_secs(20))
}

// ------------------------------------------------------------------ 全流程 --

#[test]
fn offline_full_flow_check_download_verify_switch() {
    let repo = TempRepo::new("full");
    let mock = MockSource::start();
    let files = upstream_files();
    register_source(&mock, TAG, &files);
    let m = manager(&repo, &mock);

    // 1) 版本检查：本地 2.58.0.35 < 远端 2.59.0.43
    let st = m.check().expect("检查更新应成功");
    assert_eq!(st.remote_version.as_deref(), Some(TAG));
    assert_eq!(st.local_version.as_deref(), Some("2.58.0.35"));
    assert!(st.update_available, "应当提示有新版本");

    // 2) 下载 + 校验
    m.start_download(None).expect("应能开始下载");
    let st = wait_status(&m, |s| !s.phase.busy(), Duration::from_secs(30));
    assert_eq!(st.phase, Phase::Ready, "下载完成后应进入 Ready：{:?}", st.error);
    assert_eq!(st.files_total, 3);
    assert_eq!(st.files_done, 3);
    assert_eq!(st.files_failed, 0);
    assert!(st.staging_ready);
    assert_eq!(st.staging_version.as_deref(), Some(TAG), "暂存 version 只在全部校验通过后写");
    for (rel, body) in &files {
        let rel = if let Some(r) = rel.strip_prefix("weapons/") {
            format!("gamedata/weapons/{r}")
        } else {
            format!("gamedata/flightmodels/{rel}")
        };
        let path = repo.staging_root().join(&rel);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), *body, "落盘内容应逐字节一致：{rel}");
    }
    // 现有数据一个字节都没动
    assert_eq!(
        std::fs::read_to_string(repo.data_root().join("gamedata/flightmodels/old-plane.blkx")).unwrap(),
        r#"{"model":"old_plane"}"#
    );
    assert_eq!(switch::read_version(&repo.data_root()).as_deref(), Some("2.58.0.35"));

    // 3) 二次确认 → 更新（校验通过 + HUD 没在跑）
    let dropped = st.dropped_top_level.clone();
    assert!(dropped.contains(&"levels".to_string()), "更新会丢掉没下载的顶层目录，必须能提前看到：{dropped:?}");
    m.apply_switch(false).expect("更新应成功");
    assert_eq!(switch::read_version(&repo.data_root()).as_deref(), Some(TAG));
    assert!(!repo.staging_root().exists(), "暂存目录应改名成 data");
    assert!(
        repo.data_root().join("gamedata/flightmodels/old-plane.blkx").exists() == false,
        "新数据根里不该留有旧文件"
    );
    // 单槽：上一版固定在 `resource/data_old`（只保留最近一份）
    let resource = repo.path().join("resource");
    assert!(switch::has_old_root(&resource), "应保留一份 data_old 供回滚");
    assert_eq!(switch::read_version(&switch::old_root_path(&resource)).as_deref(),
               Some("2.58.0.35"));
    assert_eq!(m.status().phase, Phase::Done);
    // 状态里要能看出"有上一版可删"
    let st = m.status();
    assert!(st.old_exists, "status.old_exists 必须为真：{st:?}");
    assert!(st.old_root.ends_with("data_old"), "{}", st.old_root);
}

// ------------------------------------------------- 更新（切换到新数据）的闸门 --

/// **HUD 在跑 → 拒绝更新**（用户口径 P6）：不杀进程、不改配置、**一个字节都不动**。
#[test]
fn switch_is_refused_while_the_hud_is_running() {
    let repo = TempRepo::new("hudrun");
    let mock = MockSource::start();
    let files = upstream_files();
    register_source(&mock, TAG, &files);
    let m = manager(&repo, &mock);
    m.check().unwrap();
    m.start_download(None).unwrap();
    let st = settled(&m);
    assert_eq!(st.phase, Phase::Ready, "{:?}", st.error);

    let before = std::fs::read(repo.data_root().join(VERSION_FILE)).unwrap();
    let err = m.apply_switch(true).expect_err("HUD 在跑时必须拒绝");
    assert_eq!(err.kind, super::net::ErrorKind::HudRunning, "{err}");
    assert_eq!(err.kind.id(), "hud_running", "前端按这个机器名选文案键");
    // 现有数据原样、暂存目录还在（关掉 HUD 就能接着更新）
    assert_eq!(std::fs::read(repo.data_root().join(VERSION_FILE)).unwrap(), before);
    assert_eq!(switch::read_version(&repo.data_root()).as_deref(), Some("2.58.0.35"));
    assert!(repo.staging_root().is_dir(), "拒绝更新不许动下载目录");
    assert!(!switch::has_old_root(&repo.path().join("resource")), "拒绝更新时不该动 data_old");
    assert_eq!(m.status().phase, Phase::Failed, "拒绝要留在失败态，让界面弹消息");

    // 关掉 HUD 之后同一次下载还能更新成功
    m.apply_switch(false).expect("HUD 关掉后应能更新");
    assert_eq!(switch::read_version(&repo.data_root()).as_deref(), Some(TAG));
}

/// **校验失败 → 不写 version、不动现有数据、保留下载目录**（用户口径 P6）。
#[test]
fn broken_file_fails_verify_without_writing_version_or_touching_data() {
    let repo = TempRepo::new("badstaging");
    let mock = MockSource::start();
    let files = upstream_files();
    register_source(&mock, TAG, &files);
    let m = manager(&repo, &mock);
    m.check().unwrap();
    m.start_download(None).unwrap();
    let st = settled(&m);
    assert_eq!(st.phase, Phase::Ready, "{:?}", st.error);

    // 模拟"下载之后文件被改坏 / 磁盘出错"：把一个暂存文件写成非法 JSON
    let victim = repo.staging_root().join("gamedata/flightmodels/a-20g.blkx");
    assert!(victim.is_file());
    std::fs::write(&victim, b"this is not json at all").unwrap();
    // 同时把"可切换"标记抹掉，才能证明"校验没过 = 不会写 version"
    std::fs::remove_file(repo.staging_root().join(VERSION_FILE)).unwrap();
    let data_before = std::fs::read(repo.data_root().join(VERSION_FILE)).unwrap();

    let err = m.apply_switch(false).expect_err("校验没过不许更新");
    assert_eq!(err.kind, super::net::ErrorKind::Verify, "{err}");
    // ① 校验没过绝不写 version
    assert!(
        switch::read_version(&repo.staging_root()).is_none(),
        "校验失败绝不能写 version（写了就等于标记成可更新）"
    );
    // ② 现有数据一个字节都没变
    assert_eq!(std::fs::read(repo.data_root().join(VERSION_FILE)).unwrap(), data_before);
    assert_eq!(switch::read_version(&repo.data_root()).as_deref(), Some("2.58.0.35"));
    assert!(repo.data_root().join("levels/x.blk").is_file());
    assert!(!switch::has_old_root(&repo.path().join("resource")), "不该留下 data_old");
    // ③ 下载目录保留（用户可以重下或「清除下载」）
    assert!(repo.staging_root().is_dir(), "校验失败要保留下载目录供重试");
}

/// 只要有一只文件被写坏，**整轮校验就停在那里**（不是抽查）—— 换成清单里最后一个文件也一样。
#[test]
fn verify_staging_checks_every_file_in_the_manifest() {
    let repo = TempRepo::new("lastfile");
    let mock = MockSource::start();
    let files = upstream_files();
    register_source(&mock, TAG, &files);
    let m = manager(&repo, &mock);
    m.check().unwrap();
    m.start_download(None).unwrap();
    settled(&m);
    assert!(m.verify_staging().is_ok(), "刚下完应当校验通过");
    // 清单里的最后一个文件（武器）写坏 → 仍然要被抓到
    let victim = repo.staging_root().join("gamedata/weapons/gun12.blkx");
    std::fs::write(&victim, b"{oops").unwrap();
    assert_eq!(m.verify_staging().unwrap_err().kind, super::net::ErrorKind::Verify);
    // 缺文件同样算校验失败
    std::fs::remove_file(&victim).unwrap();
    assert_eq!(m.verify_staging().unwrap_err().kind, super::net::ErrorKind::Verify);
}

/// **清除下载目录**：`data_new` 与 `.wp8f_update_tmp` 都删掉，现有数据不受影响。
#[test]
fn discard_clears_staging_and_tmp_but_keeps_data() {
    let repo = TempRepo::new("discard");
    let mock = MockSource::start();
    let files = upstream_files();
    register_source(&mock, TAG, &files);
    let m = manager(&repo, &mock);
    m.check().unwrap();
    m.start_download(None).unwrap();
    settled(&m);
    let tmp = repo.path().join("resource/.wp8f_update_tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("junk.bin"), b"x").unwrap();
    assert!(repo.staging_root().is_dir());

    let log = m.discard_staging().expect("清除应成功");
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(!repo.staging_root().exists(), "data_new 应被删掉");
    assert!(!tmp.exists(), "临时目录应被删掉");
    assert_eq!(switch::read_version(&repo.data_root()).as_deref(), Some("2.58.0.35"), "现有数据不动");
    assert_eq!(m.status().phase, Phase::Idle);
    assert!(m.status().staging_version.is_none());
    // 再清一次是幂等的（没有目录也不算失败）
    assert!(m.discard_staging().unwrap().is_empty());
}

#[test]
fn second_download_skips_files_that_are_already_complete() {
    let repo = TempRepo::new("resume");
    let mock = MockSource::start();
    let files = upstream_files();
    register_source(&mock, TAG, &files);
    let m = manager(&repo, &mock);
    m.check().unwrap();
    m.start_download(None).unwrap();
    let st = settled(&m);
    assert_eq!(st.phase, Phase::Ready, "{:?}", st.error);
    let before = mock.hits().len();

    // 再点一次「开始下载」：所有文件都已完整 → 不应再请求任何一个数据文件
    mock.clear_hits();
    m.start_download(None).unwrap();
    let st = settled(&m);
    assert_eq!(st.phase, Phase::Ready, "{:?}", st.error);
    assert_eq!(st.files_done, 3);
    let hits = mock.hits();
    // 数据文件 URL 一定带 tag；清单接口（contents/trees）不带 tag
    let data_hits: Vec<_> = hits.iter().filter(|h| h.path.contains(TAG)).collect();
    assert!(data_hits.is_empty(), "已完整的文件不该重新下载：{data_hits:?}");
    assert!(hits.len() < before, "第二次运行的请求应当更少");
}

#[test]
fn all_sources_failing_is_a_failure_and_never_touches_existing_data() {
    let repo = TempRepo::new("fail");
    let mock = MockSource::start();
    let files = upstream_files();
    register_source(&mock, TAG, &files);
    let m = manager(&repo, &mock);
    // 版本检查与清单都正常（只掐"数据文件"的两种 URL：raw 直链与 api 的 blob）→
    // 每个文件在两个源上都失败后本轮失败
    m.check().unwrap();
    mock.fail_matching(&[&format!("{TAG}/aces.vromfs.bin_u"), "/git/blobs/"]);
    m.start_download(None).unwrap();
    let st = settled(&m);
    assert_eq!(st.phase, Phase::Failed, "所有源都失败必须是失败");
    let err = st.error.clone().expect("要有明确错误");
    assert_eq!(err.kind, super::net::ErrorKind::Network, "{err}");
    assert_eq!(st.files_failed, 3, "三个文件都该被记为失败（不静默跳过）");
    // **回归：现有数据必须原样**
    assert_eq!(switch::read_version(&repo.data_root()).as_deref(), Some("2.58.0.35"));
    assert_eq!(
        std::fs::read_to_string(repo.data_root().join("gamedata/flightmodels/old-plane.blkx")).unwrap(),
        r#"{"model":"old_plane"}"#
    );
    assert!(repo.data_root().join("levels/x.blk").is_file());
    // 暂存目录里没有 version（= 不可切换），重试时还能续
    assert!(switch::read_version(&repo.staging_root()).is_none(), "失败绝不能写 version");
    assert!(repo.staging_root().is_dir());
    // 没有任何半截文件留在正式位置
    let leftover: Vec<_> = walk_files(&repo.staging_root())
        .into_iter()
        .filter(|p| p.extension().map(|e| e == "part").unwrap_or(false))
        .collect();
    assert!(leftover.is_empty(), "不该留下 .part：{leftover:?}");
}

/// 递归列文件（断言"不留半截文件"用）。
fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk_files(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn unreachable_first_source_falls_back_to_the_next_one() {
    let repo = TempRepo::new("fallback");
    let dead = MockSource::start();
    let live = MockSource::start();
    let files = upstream_files();
    // 死源：不注册任何东西，且一律断连（模拟被 reset 的 github 原站）
    dead.break_next(1000);
    register_source(&live, TAG, &files);
    let sources = vec![
        Source::with_base(SourceKind::GithubRaw, dead.base()),
        Source::with_base(SourceKind::GithubApi, live.base()),
    ];
    let m = UpdateManager::with_transport(repo.paths(), Box::new(HttpTransport::new()), sources, &live.base());
    m.set_source_fail_limit(2);
    // 串行下载：这样"第几个文件把死源踢掉"是确定的，可以精确断言请求次数
    m.set_concurrency(1);
    m.check().expect("第一个源断了，应当用后面的源拿到版本号");
    m.start_download(None).unwrap();
    let st = settled(&m);
    assert_eq!(st.phase, Phase::Ready, "{:?}", st.error);
    assert!(st.files_done == 3);
    let log = st.log.join("\n");
    assert!(log.contains("停用"), "死源连续失败后应当被停用并记日志：{log}");
    assert!(st.disabled_sources.iter().any(|d| d.id == "github"), "{:?}", st.disabled_sources);
    // 停用之后就不该再往死源发请求：版本检查 1 次 + 前两个文件各 1 次 = 3
    let dead_hits = dead.hits().len();
    assert_eq!(dead_hits, 3, "死源被停用后不该反复重试（实际 {dead_hits} 次）");
}

#[test]
fn wrong_size_or_broken_json_is_a_verify_failure() {
    let repo = TempRepo::new("verify");
    let mock = MockSource::start();
    let files = upstream_files();
    register_source(&mock, TAG, &files);
    // 清单说 a-20g.blkx 有 N 字节，但源上放的是别的内容（长度不同）→ 校验失败
    let body = files.iter().find(|(r, _)| *r == "a-20g.blkx").unwrap().1.clone();
    let upstream = format!("{}/a-20g.blkx", Subset::FlightModels.upstream_rel());
    // P9：只有 github.com 的 `/raw/` 形态（jsDelivr 的 `@tag` 形态已删）。
    // ⚠️ 两条源都要放坏内容：只坏 raw 的话会发现"下一源（api）拿到的是好内容"→ 下载成功，
    //    就测不到"校验失败"这条路径了（源回退是**有意**行为，不是这里要测的东西）。
    let idx = files.iter().position(|(r, _)| *r == "a-20g.blkx").expect("样本里应有 a-20g.blkx");
    mock.put(&format!("/{REPO}/raw/{TAG}/{upstream}"), b"not the same length at all".to_vec());
    mock.put(&format!("/repos/{REPO}/git/blobs/sha_fm{idx}"), b"not the same length at all".to_vec());
    let m = manager(&repo, &mock);
    m.check().unwrap();
    m.start_download(None).unwrap();
    let st = settled(&m);
    assert_eq!(st.phase, Phase::Failed);
    let err = st.error.clone().unwrap();
    assert_eq!(err.kind, super::net::ErrorKind::Verify, "根因是校验不过，不是网络：{err}");
    assert!(switch::read_version(&repo.staging_root()).is_none(), "校验没过绝不能写 version");
    assert_eq!(switch::read_version(&repo.data_root()).as_deref(), Some("2.58.0.35"));
    // 坏内容不能留在暂存目录里（否则续传会把它当已完成）
    let bad = repo.staging_root().join("gamedata/flightmodels/a-20g.blkx");
    assert!(!bad.exists(), "校验失败的文件不能留在正式位置");
    assert!(!download::part_path(&bad).exists(), "也不该留下 .part");
    // 其它两个文件应当已经下好了（失败不影响已完成的）
    assert!(repo.staging_root().join("gamedata/flightmodels/fm/a-20g.blkx").is_file());
    let _ = body;
}

#[test]
fn cancel_leaves_staging_resumable_and_data_intact() {
    let repo = TempRepo::new("cancel");
    let mock = MockSource::start();
    let files = upstream_files();
    register_source(&mock, TAG, &files);
    mock.set_sleep_ms(120); // 每个请求慢一点，好让取消落在下载中间
    let m = manager(&repo, &mock);
    m.check().unwrap();
    m.start_download(None).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    m.cancel();
    let st = wait_status(&m, |s| !s.phase.busy(), Duration::from_secs(20));
    assert!(st.error.is_none(), "取消不是失败：{:?}", st.error);
    assert!(st.files_done < 3, "应当是在下完之前被取消的（done={}）", st.files_done);
    assert!(switch::read_version(&repo.staging_root()).is_none(), "取消不能写 version");
    assert_eq!(switch::read_version(&repo.data_root()).as_deref(), Some("2.58.0.35"));

    // 取消后可以接着下（假源恢复正常速度）
    mock.set_sleep_ms(0);
    m.start_download(None).unwrap();
    let st = settled(&m);
    assert_eq!(st.phase, Phase::Ready, "{:?}", st.error);
    assert_eq!(st.files_done, 3);
}

#[test]
fn manifest_falls_back_to_local_tree_when_the_api_is_unreachable() {
    let repo = TempRepo::new("localmanifest");
    let mock = MockSource::start();
    let files = upstream_files();
    register_source(&mock, TAG, &files);
    // 只掐掉清单接口（模拟 api.github.com 限流 403/断网）：数据文件仍然可取
    mock.put(
        &format!("/repos/{REPO}/contents/{}", manifest::UPSTREAM_GAMEDATA),
        b"{\"message\":\"API rate limit exceeded\"}".to_vec(),
    );
    // 本地清单里的两个文件在"新 tag"下也要能取到（否则就是"上游删了文件"）
    for (rel, body) in [
        ("gamedata/flightmodels/old-plane.blkx", r#"{"model":"old_plane_v2"}"#),
        ("gamedata/weapons/old-gun.blkx", r#"{"model":"old_gun_v2"}"#),
    ] {
        let upstream = format!("{}/{rel}", manifest::UPSTREAM_ROOT);
        mock.put(&format!("/{REPO}/raw/{TAG}/{upstream}"), body.as_bytes().to_vec());
    }
    let m = manager(&repo, &mock);
    m.check().unwrap();
    m.start_download(None).unwrap();
    let st = settled(&m);
    let note = st.manifest_note.clone();
    assert!(!st.manifest_authoritative, "清单应当退到本地兜底");
    // 本地清单里只有旧数据那两个文件（新增文件发现不了，这是兜底的已知代价）
    assert_eq!(st.files_total, 2, "本地清单只列得出已有文件：{}", st.files_total);
    assert_eq!(st.phase, Phase::Ready, "{:?} ({note})", st.error);
    assert_eq!(
        std::fs::read_to_string(repo.staging_root().join("gamedata/flightmodels/old-plane.blkx")).unwrap(),
        r#"{"model":"old_plane_v2"}"#
    );
}

#[test]
fn downloader_reports_progress_and_human_sizes() {
    let repo = TempRepo::new("progress");
    let mock = MockSource::start();
    let files = upstream_files();
    register_source(&mock, TAG, &files);
    let m = manager(&repo, &mock);
    m.check().unwrap();
    m.start_download(None).unwrap();
    let st = settled(&m);
    assert_eq!(st.bytes_total, files.iter().map(|(_, b)| b.len() as u64).sum::<u64>());
    assert_eq!(st.bytes_done, st.bytes_total);
    assert!(st.log.iter().any(|l| l.contains("[UPDATE] 开始下载")));
    assert!(st.log.iter().any(|l| l.contains("[UPDATE] 下载完成")));
    assert!(st.log.iter().any(|l| l.contains("[UPDATE] 核对通过")));
    assert_eq!(download::human_bytes(0), "0 B");
}

// -------------------------------------------- 传输层（真实现 + 假源）细节 --

#[test]
fn ranged_resume_math_is_validated_by_the_builtin_client() {
    // mock 支持标准 206；这条锁住"将来换支持 Range 的源时能打开 resume_parts"
    let mock = MockSource::start();
    let body: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    mock.put("/data.bin", body.clone());
    let t = HttpTransport::new();
    let dir = std::env::temp_dir().join(format!("wp8f_p3_range_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let dest = dir.join("data.bin.part");
    let _ = std::fs::remove_file(&dest);
    let cancel = super::net::NeverCancel;
    let first = t
        .fetch(
            &super::net::FetchSpec { url: &format!("{}/data.bin", mock.base()), dest: &dest, range_from: 0, accept: None },
            &cancel,
        )
        .expect("第一段");
    assert_eq!(first, 4096);
    // 再要一次前 1000 字节之后的内容（客户端应当发 Range 并校验 Content-Range）
    let spec = super::net::FetchSpec {
        url: &format!("{}/data.bin", mock.base()),
        dest: &dest,
        range_from: 4096,
        accept: None,
    };
    let more = t.fetch(&spec, &cancel).expect("越界 Range 应按 416 处理");
    assert_eq!(more, 0, "已经下完时不该再拿字节");
    assert_eq!(std::fs::read(&dest).unwrap(), body, "内容必须逐字节正确");
    let hits = mock.hits();
    assert!(hits.iter().any(|h| h.range_from == Some(4096)), "{hits:?}");

    // 服务端忽略 Range（jsDelivr 实测行为）→ 内置客户端必须报错，而不是把整份追加到后面
    let dest2 = dir.join("data2.bin.part");
    std::fs::write(&dest2, &body[..1000]).unwrap();
    mock.ignore_range(true);
    let spec2 = super::net::FetchSpec {
        url: &format!("{}/data.bin", mock.base()),
        dest: &dest2,
        range_from: 1000,
        accept: None,
    };
    let err = t.fetch(&spec2, &cancel).unwrap_err();
    assert_eq!(err.kind, super::net::ErrorKind::Network, "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn download_config_defaults_match_the_documented_choices() {
    let cfg = DownloadConfig::new(REPO, TAG, PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b"));
    assert_eq!(cfg.concurrency, 6);
    assert_eq!(cfg.attempts, 3);
    assert!(!cfg.resume_parts, "实测 CDN 忽略 Range、GitHub raw 也不支持按段续传：默认关掉文件内续传");
    assert_eq!(cfg.sources.len(), 2, "P9：只有 github.com 原站 + api.github.com 两条源");
}

// ------------------------------------------------- 真网络取样（默认不跑） --
//
// 验证红线：**不要真下 361 MB**。这两条只取"版本文件 + 一个数据文件"，
// 用生产代码路径（真源顺序 + 真 curl/内置客户端 + 真校验）在真网络上取样。
// 默认 `#[ignore]`（网络会抖，不该进 test.sh）：
//     cargo test --offline --release -p wp8f-flightmodel update:: -- --ignored --nocapture

/// 真网络：版本检查（P9 起两个源按顺序试：github.com 原站预期被 reset，api.github.com 兜底）。
#[test]
#[ignore = "真网络取样：cargo test -- --ignored"]
fn real_network_fetch_version() {
    let tmp = std::env::temp_dir().join(format!("wp8f_p3_realver_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let t = HttpTransport::new();
    let got = manifest::fetch_version(
        &t,
        &manifest::default_sources(),
        manifest::REPO,
        manifest::DEFAULT_REF,
        &tmp,
        &super::net::NeverCancel,
    );
    match got {
        Ok((version, src)) => {
            println!("真网络版本号 = {version}（源：{src}）");
            let v = super::version::Version::parse(&version).expect("版本号应能解析");
            assert!(v.segments().len() >= 3, "{v:?}");
            assert!(
                super::version::compare(&version, "2.58.0.35").is_some(),
                "应与本地版本可比"
            );
        }
        Err(e) => {
            // 断网环境不算测试失败，但必须**说出来**
            println!("真网络不可用（按规格应报网络错误）：{e}");
            assert_eq!(e.kind, super::net::ErrorKind::Network);
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// 真网络：清单（api.github.com 的 trees）+ **只下 1 个文件**并校验。
#[test]
#[ignore = "真网络取样：cargo test -- --ignored"]
fn real_network_downloads_exactly_one_file() {
    let tmp = std::env::temp_dir().join(format!("wp8f_p3_realone_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::create_dir_all(&tmp);
    let t = HttpTransport::new();
    let cancel = super::net::NeverCancel;
    let (version, _src) = match manifest::fetch_version(
        &t,
        &manifest::default_sources(),
        manifest::REPO,
        manifest::DEFAULT_REF,
        &tmp,
        &cancel,
    ) {
        Ok(v) => v,
        Err(e) => {
            println!("真网络不可用，跳过取样：{e}");
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }
    };
    // 清单只取 flightmodels（另一个子树要多花一次 API 配额）
    let m = manifest::fetch_manifest_github(
        &t,
        manifest::REPO,
        &version,
        &[Subset::FlightModels],
        &tmp,
        manifest::API_BASE,
        &cancel,
    )
    .expect("清单应能取到");
    println!(
        "清单：{} 个文件 / {}（{}）",
        m.entries.len(),
        download::human_bytes(m.total_bytes),
        m.note
    );
    assert_eq!(m.source, manifest::ManifestSource::GithubTree);
    assert!(m.entries.len() > 10_000, "flightmodels 应当有一万多个文件");
    assert!(m.total_bytes > 100 * 1024 * 1024);

    // 挑一个中等大小的文件真下（50–200 KB：既不是 2 字节的退化样本，也远小于 361MB 红线）
    let smallest = m
        .entries
        .iter()
        .filter(|e| e.size.map(|s| (50_000..=200_000).contains(&s)).unwrap_or(false))
        .min_by_key(|e| e.size.unwrap_or(u64::MAX))
        .or_else(|| m.entries.iter().min_by_key(|e| e.size.unwrap_or(u64::MAX)))
        .expect("非空");
    println!("下载样本：{}（{:?} 字节）", smallest.rel, smallest.size);
    let mut cfg = DownloadConfig::new(
        manifest::REPO,
        &version,
        tmp.join("data_new"),
        tmp.join("tmp"),
    );
    cfg.concurrency = 1;
    cfg.sources = manifest::default_sources();
    let manifest_one = manifest::Manifest {
        entries: vec![smallest.clone()],
        total_bytes: smallest.size.unwrap_or(0),
        source: m.source,
        note: m.note.clone(),
    };
    let progress = std::sync::Mutex::new(download::Progress::default());
    let log = |l: String| println!("{l}");
    let out = download::download_all(&t, &cfg, &manifest_one, &progress, &log, &cancel)
        .expect("这一个文件应当能下下来并通过校验");
    assert_eq!(out.downloaded, 1);
    let landed = cfg.staging_root.join(&smallest.rel);
    assert!(landed.is_file(), "落盘路径：{}", landed.display());
    println!("落盘 {} 字节 → {}", std::fs::metadata(&landed).unwrap().len(), landed.display());
    let _ = std::fs::remove_dir_all(&tmp);
}
