//! 逐文件下载器：并发有上限、逐文件重试 + 多源回退、可取消、进度可读。
//!
//! # 续传口径（**实测决定**，见下）
//!
//! * **文件粒度**是主口径：已经下完且校验通过的文件直接跳过 —— 清单来自上游，
//!   同 tag 同内容，所以"按文件跳过"就是最稳的续传（D6 的结论）。
//! * **`.part` 文件**用于"下到一半被打断"：文件先落到 `<名字>.part`，校验通过才改名成
//!   正式名字。所以数据根里**永远不出现半截文件**；下一次运行看到同名 `.part` 会重新下这一个文件。
//! * **不使用 HTTP Range 在文件内部续传**（`resume_parts` 默认 `false`）：实测第三方 CDN
//!   （历史上用过的 `cdn.jsdelivr.net`，P9 已删）对本仓库的文件**忽略 Range**——
//!   `Range: bytes=0-59` 返回的 60 字节与整份文件的前 60 字节**不一致**（是别的编码），
//!   `Range: bytes=100000-`（越界）直接回**整份**（200），`curl -C -` 续传后文件停在 50000 字节
//!   且 curl 退出码为 0 —— 也就是说"信任续传"会静默产出坏文件。
//!   单文件最大 408 KB、实测 0.46–0.53 s/文件，重下整份的代价远小于静默损坏的代价。
//!   （`net` 层仍保留 Range 能力：内置客户端会校验 `Content-Range` 起点、遇到 200 直接报错，
//!   单测里有 mock 服务端覆盖；将来确认源支持 Range，把 `resume_parts` 打开即可。）

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::manifest::{Manifest, ManifestEntry, Source};
use super::net::{self, Canceller, ErrorKind, FetchSpec, Transport, UpdateError};

/// 一次下载的配置。
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    pub repo: String,
    /// 目标版本号（= 上游 tag 名）
    pub tag: String,
    /// 暂存数据根（`resource/data_new`）
    pub staging_root: PathBuf,
    /// 中间文件目录（临时文件，`staging_root` 之外更好：清单/校验不会把它当成数据）
    pub tmp_dir: PathBuf,
    pub sources: Vec<Source>,
    /// 并发上限（实测上游/CDN 对并发敏感：太高会成片 TLS reset）
    pub concurrency: usize,
    /// 每个文件的尝试次数上限（每次都会按源顺序重新试）
    pub attempts: usize,
    /// 某个源连续失败多少次后本轮停用（永远保留最后一个可用源）
    pub source_fail_limit: u32,
    /// 是否在文件内部用 Range 续传（默认否，理由见模块头）
    pub resume_parts: bool,
}

impl DownloadConfig {
    pub fn new(repo: &str, tag: &str, staging_root: PathBuf, tmp_dir: PathBuf) -> Self {
        Self {
            repo: repo.to_string(),
            tag: tag.to_string(),
            staging_root,
            tmp_dir,
            sources: super::manifest::default_sources(),
            concurrency: 6,
            attempts: 3,
            source_fail_limit: 5,
            resume_parts: false,
        }
    }
}

/// 单个文件的失败记录（**不静默跳过**：只要有一条，本轮就算失败）。
#[derive(Debug, Clone)]
pub struct FailedFile {
    pub rel: String,
    pub kind: ErrorKind,
    pub reason: String,
}

/// 被停用的源。
#[derive(Debug, Clone)]
pub struct DisabledSource {
    pub id: &'static str,
    pub name: &'static str,
    pub reason: String,
}

/// 实时进度（GUI 轮询读它；`[UPDATE]` 日志按节流打点）。
#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub files_total: u64,
    pub files_done: u64,
    pub files_failed: u64,
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub failed: Vec<FailedFile>,
    pub disabled_sources: Vec<DisabledSource>,
    pub retries: u64,
    /// 最近一次进度日志的文件数（节流用）
    pub last_log_files: u64,
    pub last_log_at: Option<Instant>,
}

impl Progress {
    /// 已完成比例（0.0–1.0）；总数未知时按 0 处理。
    pub fn ratio(&self) -> f64 {
        if self.files_total == 0 {
            0.0
        } else {
            (self.files_done as f64 / self.files_total as f64).clamp(0.0, 1.0)
        }
    }
}

/// 下载结果汇总。
#[derive(Debug, Clone, Default)]
pub struct DownloadOutcome {
    pub downloaded: u64,
    pub skipped: u64,
    pub bytes: u64,
}

/// 并发下载清单里的全部文件。
///
/// * 任一文件在所有源上都失败 → 返回 `Err`（本轮更新失败；已下好的文件留在暂存根里，可续传）；
/// * 取消 → `Err(Cancelled)`（不算失败，暂存根保留）；
/// * 磁盘满/权限不足 → 立刻中止（继续跑只会刷满日志）。
pub fn download_all(
    transport: &dyn Transport,
    cfg: &DownloadConfig,
    manifest: &Manifest,
    progress: &Mutex<Progress>,
    log: &(dyn Fn(String) + Send + Sync),
    cancel: &dyn Canceller,
) -> Result<DownloadOutcome, UpdateError> {
    let total_files = manifest.entries.len() as u64;
    {
        let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
        p.files_total = total_files;
        p.bytes_total = manifest.total_bytes;
    }
    log(format!(
        "[UPDATE] 开始下载：{total_files} 个文件 / {}（并发 {}，源顺序 {}{}）",
        human_bytes(manifest.total_bytes),
        cfg.concurrency.max(1),
        cfg.sources.iter().map(|s| s.name()).collect::<Vec<_>>().join(" → "),
        if cfg.resume_parts { "，文件内 Range 续传已开" } else { "" }
    ));
    if matches!(manifest.source, super::manifest::ManifestSource::LocalTree) {
        log(format!("[UPDATE] {}", manifest.note));
    }

    let next = AtomicUsize::new(0);
    let fatal: Mutex<Option<UpdateError>> = Mutex::new(None);
    let fail_streak: Mutex<Vec<u32>> = Mutex::new(vec![0; cfg.sources.len()]);
    let disabled: Mutex<Vec<Option<String>>> = Mutex::new(vec![None; cfg.sources.len()]);
    let counters = Mutex::new(DownloadOutcome::default());
    let workers = cfg.concurrency.clamp(1, 32).min(manifest.entries.len().max(1));

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                worker_loop(
                    transport, cfg, manifest, &next, progress, &fatal, &fail_streak, &disabled,
                    &counters, log, cancel,
                );
            });
        }
    });

    if let Some(e) = fatal.into_inner().unwrap_or_else(|e| e.into_inner()) {
        return Err(e);
    }
    if cancel.cancelled() {
        return Err(UpdateError::cancelled());
    }
    let outcome = counters.into_inner().unwrap_or_else(|e| e.into_inner());
    let (failed, done) = {
        let p = progress.lock().unwrap_or_else(|e| e.into_inner());
        (p.failed.clone(), p.files_done)
    };
    if !failed.is_empty() {
        let mut detail: Vec<String> = failed
            .iter()
            .take(5)
            .map(|f| format!("{}（{}）", f.rel, f.reason))
            .collect();
        if failed.len() > 5 {
            detail.push(format!("… 另有 {} 个", failed.len() - 5));
        }
        let kind = failed.iter().map(|f| f.kind).find(|k| *k == ErrorKind::Verify).unwrap_or(ErrorKind::Network);
        return Err(UpdateError::new(
            kind,
            format!(
                "{} 个文件在所有下载源上都失败（已完成 {done} 个）：{}",
                failed.len(),
                detail.join("；")
            ),
        ));
    }
    log(format!(
        "[UPDATE] 下载完成：{} 个文件 / {}（新下 {} 个，跳过已完整 {} 个，重试 {} 次）",
        done,
        human_bytes(outcome.bytes),
        outcome.downloaded,
        outcome.skipped,
        {
            let p = progress.lock().unwrap_or_else(|e| e.into_inner());
            p.retries
        }
    ));
    Ok(outcome)
}

#[allow(clippy::too_many_arguments)]
fn worker_loop(
    transport: &dyn Transport,
    cfg: &DownloadConfig,
    manifest: &Manifest,
    next: &AtomicUsize,
    progress: &Mutex<Progress>,
    fatal: &Mutex<Option<UpdateError>>,
    fail_streak: &Mutex<Vec<u32>>,
    disabled: &Mutex<Vec<Option<String>>>,
    counters: &Mutex<DownloadOutcome>,
    log: &(dyn Fn(String) + Send + Sync),
    cancel: &dyn Canceller,
) {
    loop {
        if cancel.cancelled() {
            return;
        }
        if fatal.lock().map(|g| g.is_some()).unwrap_or(false) {
            return;
        }
        let idx = next.fetch_add(1, Ordering::SeqCst);
        if idx >= manifest.entries.len() {
            return;
        }
        let entry = &manifest.entries[idx];
        match fetch_one(transport, cfg, entry, progress, fail_streak, disabled, log, cancel) {
            Ok(fetched) => {
                {
                    let mut c = counters.lock().unwrap_or_else(|e| e.into_inner());
                    c.bytes += fetched.bytes;
                    if fetched.skipped {
                        c.skipped += 1;
                    } else {
                        c.downloaded += 1;
                    }
                }
                let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
                p.files_done += 1;
                p.bytes_done += fetched.bytes;
                log_progress_throttled(&mut p, log);
            }
            Err(e) if e.kind == ErrorKind::Cancelled => return,
            Err(e) if is_fatal(&e.kind) => {
                let mut slot = fatal.lock().unwrap_or_else(|x| x.into_inner());
                if slot.is_none() {
                    *slot = Some(e);
                }
                return;
            }
            Err(e) => {
                let mut p = progress.lock().unwrap_or_else(|x| x.into_inner());
                p.files_failed += 1;
                p.files_done += 1;
                p.failed.push(FailedFile { rel: entry.rel.clone(), kind: e.kind, reason: e.message.clone() });
                drop(p);
                log(format!("[UPDATE] {}：{}（{}）", e.kind.label(), entry.rel, e.message));
            }
        }
    }
}

/// 中止整轮的类别：继续跑下去只会把同一个错误刷 1.8 万遍。
fn is_fatal(kind: &ErrorKind) -> bool {
    matches!(kind, ErrorKind::DiskFull | ErrorKind::Permission | ErrorKind::Io)
}

fn log_progress_throttled(p: &mut Progress, log: &(dyn Fn(String) + Send + Sync)) {
    let now = Instant::now();
    let by_files = p.files_done >= p.last_log_files + 1000;
    let by_time = p.last_log_at.map(|t| now.duration_since(t) >= Duration::from_secs(30)).unwrap_or(true);
    if by_files || by_time {
        p.last_log_files = p.files_done;
        p.last_log_at = Some(now);
        log(format!(
            "[UPDATE] 已下载 {}/{} 个文件（{} / {}，{:.1}%）",
            p.files_done,
            p.files_total,
            human_bytes(p.bytes_done),
            human_bytes(p.bytes_total),
            p.ratio() * 100.0
        ));
    }
}

/// 下一个可用的源顺序：跳过已停用的；若全被停用（不该发生），退回"全部源"。
fn source_order(cfg: &DownloadConfig, disabled: &Mutex<Vec<Option<String>>>) -> Vec<usize> {
    let guard = disabled.lock().unwrap_or_else(|e| e.into_inner());
    let usable: Vec<usize> = (0..cfg.sources.len()).filter(|i| guard[*i].is_none()).collect();
    if usable.is_empty() {
        (0..cfg.sources.len()).collect()
    } else {
        usable
    }
}

/// 记录一次**主机级**失败（连不上/被重置/超时）——只有这类失败才可能停用整个源。
///
/// HTTP 404/403 这类"这台主机是活的、只是这个文件/这次配额不行"的失败**不**计入，
/// 否则某个文件在上游被删掉会把健康的源一起拖下水。
fn mark_source_failure(
    cfg: &DownloadConfig,
    idx: usize,
    err: &UpdateError,
    fail_streak: &Mutex<Vec<u32>>,
    disabled: &Mutex<Vec<Option<String>>>,
    progress: &Mutex<Progress>,
    log: &(dyn Fn(String) + Send + Sync),
) {
    let mut streak = fail_streak.lock().unwrap_or_else(|e| e.into_inner());
    if idx >= streak.len() {
        return;
    }
    streak[idx] += 1;
    let hit_limit = streak[idx] >= cfg.source_fail_limit;
    let enabled = disabled.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|d| d.is_none()).count();
    if hit_limit && enabled > 1 {
        let src = &cfg.sources[idx];
        let reason = err.message.clone();
        let mut guard = disabled.lock().unwrap_or_else(|e| e.into_inner());
        if guard[idx].is_none() {
            guard[idx] = Some(reason.clone());
            drop(guard);
            log(format!(
                "[UPDATE] 源 {} 连续 {} 次连不上，本轮停用（{}）",
                src.name(),
                streak[idx],
                reason
            ));
            if let Ok(mut p) = progress.lock() {
                if !p.disabled_sources.iter().any(|d| d.id == src.id()) {
                    p.disabled_sources.push(DisabledSource {
                        id: src.id(),
                        name: src.name(),
                        reason,
                    });
                }
            }
        }
    }
}

fn mark_source_success(fail_streak: &Mutex<Vec<u32>>, idx: usize) {
    if let Ok(mut streak) = fail_streak.lock() {
        if idx < streak.len() {
            streak[idx] = 0;
        }
    }
}

/// 一个文件的结果。
struct Fetched {
    /// 该文件的总字节（跳过时按清单大小算）
    bytes: u64,
    /// 本次是"已存在且校验通过、直接跳过"
    skipped: bool,
}

/// 下载并落盘一个文件：每次尝试都按源顺序**逐个试过去**，校验通过才改名成正式文件。
fn fetch_one(
    transport: &dyn Transport,
    cfg: &DownloadConfig,
    entry: &ManifestEntry,
    progress: &Mutex<Progress>,
    fail_streak: &Mutex<Vec<u32>>,
    disabled: &Mutex<Vec<Option<String>>>,
    log: &(dyn Fn(String) + Send + Sync),
    cancel: &dyn Canceller,
) -> Result<Fetched, UpdateError> {
    let final_path = cfg.staging_root.join(&entry.rel);
    let part = part_path(&final_path);
    // 已完成且校验通过 → 跳过（**文件粒度续传**的主路径）
    if final_path.is_file() {
        if let Ok(()) = verify_file(&final_path, entry) {
            let _ = fs::remove_file(&part);
            let bytes = entry
                .size
                .unwrap_or_else(|| fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0));
            return Ok(Fetched { bytes, skipped: true });
        }
    }
    if let Some(parent) = final_path.parent() {
        fs::create_dir_all(parent).map_err(|e| net::classify_io(&e, &format!("建目录 {} 失败", parent.display())))?;
    }
    let mut last: Option<UpdateError> = None;
    let mut verify_err: Option<UpdateError> = None;
    let mut net_failures: Vec<String> = Vec::new();
    for attempt in 0..cfg.attempts.max(1) {
        if cancel.cancelled() {
            return Err(UpdateError::cancelled());
        }
        let order = source_order(cfg, disabled);
        for si in order {
            if cancel.cancelled() {
                return Err(UpdateError::cancelled());
            }
            let src = &cfg.sources[si];
            let url = match src.file_url(&cfg.repo, &cfg.tag, &entry.upstream, entry.sha.as_deref()) {
                Some(u) => u,
                None => {
                    // 清单退到本地旧数据时没有 blob sha → api 源对这次下载不可用
                    let msg = format!("{} 没有可用地址（清单里缺 blob sha）", src.name());
                    if !net_failures.contains(&msg) {
                        net_failures.push(msg.clone());
                    }
                    last = Some(UpdateError::network(msg));
                    continue;
                }
            };
            // 文件内 Range 续传（默认关；理由见模块头：实测的服务端会忽略 Range 并回整份）
            let range_from = if cfg.resume_parts {
                fs::metadata(&part).map(|m| m.len()).unwrap_or(0)
            } else {
                let _ = fs::remove_file(&part);
                0
            };
            let spec = FetchSpec { url: &url, dest: &part, range_from, accept: src.accept() };
            match transport.fetch(&spec, cancel) {
                Ok(_) => {
                    mark_source_success(fail_streak, si);
                    match verify_file(&part, entry) {
                        Ok(()) => {
                            replace_file(&part, &final_path)?;
                            let bytes = entry
                                .size
                                .unwrap_or_else(|| fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0));
                            return Ok(Fetched { bytes, skipped: false });
                        }
                        Err(e) => {
                            // 内容不对：这次的字节不能留（不然续传会把坏内容拼进去）
                            let _ = fs::remove_file(&part);
                            log(format!(
                                "[UPDATE] 校验失败：{}（{}，第 {} 次尝试 · {}）",
                                entry.rel,
                                e.message,
                                attempt + 1,
                                src.name()
                            ));
                            // 校验失败是**根因**，报错时优先于"某个源 404 了"这类噪声
                            if verify_err.is_none() {
                                verify_err = Some(e);
                            }
                        }
                    }
                }
                Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
                Err(e) => {
                    if e.host_unreachable {
                        mark_source_failure(cfg, si, &e, fail_streak, disabled, progress, log);
                    }
                    if is_fatal(&e.kind) {
                        return Err(e);
                    }
                    if let Ok(mut p) = progress.lock() {
                        p.retries += 1;
                    }
                    let msg = format!("{}：{}", src.name(), e.message);
                    if !net_failures.contains(&msg) {
                        net_failures.push(msg);
                    }
                    last = Some(e);
                }
            }
        }
    }
    // 报错优先级：校验失败（说明内容不对） > 网络失败（说明够不着）
    if let Some(v) = verify_err {
        let extra = if net_failures.is_empty() {
            String::new()
        } else {
            format!("（同时有网络失败：{}）", net_failures.join(" / "))
        };
        return Err(UpdateError::new(v.kind, format!("{}{extra}", v.message)));
    }
    if let Some(e) = last {
        let detail = if net_failures.len() > 1 {
            format!("；各源：{}", net_failures.join(" / "))
        } else {
            String::new()
        };
        return Err(UpdateError::new(e.kind, format!("{}{detail}", e.message)));
    }
    Err(UpdateError::network(format!("{}：没有可用的下载源", entry.rel)))
}

/// `<文件>.part`（**追加**后缀，不用 `with_extension`：`su-9m14m.default` 这种名字会被它改坏）。
pub fn part_path(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

fn replace_file(part: &Path, final_path: &Path) -> Result<(), UpdateError> {
    let _ = fs::remove_file(final_path);
    fs::rename(part, final_path).map_err(|e| {
        net::classify_io(&e, &format!("把 {} 改名为 {} 失败", part.display(), final_path.display()))
    })
}

/// 校验：大小（清单有权威大小时）+ JSON 可解析（`.blkx` 都是 JSON，见 D6）。
pub fn verify_file(path: &Path, entry: &ManifestEntry) -> Result<(), UpdateError> {    let len = fs::metadata(path)
        .map_err(|e| net::classify_io(&e, &format!("读 {} 大小失败", path.display())))?
        .len();
    if let Some(want) = entry.size {
        if len != want {
            return Err(UpdateError::verify(format!(
                "{} 大小不符（期望 {want} 字节，实得 {len} 字节）",
                entry.rel
            )));
        }
    }
    if entry.rel.ends_with(".blkx") {
        let text = fs::read_to_string(path)
            .map_err(|e| net::classify_io(&e, &format!("读 {} 失败", path.display())))?;
        let text = text.trim_start_matches('\u{feff}');
        serde_json::from_str::<serde_json::Value>(text).map_err(|e| {
            UpdateError::verify(format!("{} 不是合法 JSON：{e}", entry.rel))
        })?;
    }
    Ok(())
}

/// 人类可读字节数（日志与界面都用它，避免两处各写一份）。
pub fn human_bytes(n: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if n >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", n as f64 / (MB * 1024.0))
    } else if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / MB)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_path_appends_instead_of_replacing_extension() {
        assert_eq!(part_path(Path::new("/x/a-20g.blkx")), PathBuf::from("/x/a-20g.blkx.part"));
        assert_eq!(
            part_path(Path::new("/x/guns/su-9m14m.default.blkx")),
            PathBuf::from("/x/guns/su-9m14m.default.blkx.part"),
            "带点的文件名不能走 with_extension"
        );
    }

    #[test]
    fn human_bytes_is_stable() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.00 GB");
    }

    #[test]
    fn progress_ratio_is_bounded() {
        let mut p = Progress { files_total: 4, files_done: 1, ..Progress::default() };
        assert!((p.ratio() - 0.25).abs() < 1e-9);
        p.files_done = 99;
        assert_eq!(p.ratio(), 1.0, "超过总数也不能超过 100%");
        let empty = Progress::default();
        assert_eq!(empty.ratio(), 0.0, "总数未知时不能除零");
    }

    #[test]
    fn verify_detects_size_and_json_problems() {
        let dir = std::env::temp_dir().join(format!("wp8f_p3_verify_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let good = dir.join("good.blkx");
        fs::write(&good, br#"{"a": 1}"#).unwrap();
        let entry = ManifestEntry {
            rel: "gamedata/flightmodels/good.blkx".into(),
            upstream: "u".into(),
            size: Some(8),
            sha: None,
        };
        assert!(verify_file(&good, &entry).is_ok());

        let bad_size = ManifestEntry { size: Some(7), ..entry.clone() };
        assert_eq!(verify_file(&good, &bad_size).unwrap_err().kind, ErrorKind::Verify);

        let bad_json = dir.join("bad.blkx");
        fs::write(&bad_json, b"model:t = \"a_20\"\n").unwrap();
        let e2 = ManifestEntry { size: Some(17), ..entry.clone() };
        assert_eq!(verify_file(&bad_json, &e2).unwrap_err().kind, ErrorKind::Verify);

        // 非 .blkx 只校大小
        let other = dir.join("x.txt");
        fs::write(&other, b"hello").unwrap();
        let e3 = ManifestEntry { rel: "gamedata/flightmodels/x.txt".into(), size: Some(5), ..entry.clone() };
        assert!(verify_file(&other, &e3).is_ok());

        // BOM 容忍
        let bom = dir.join("bom.blkx");
        fs::write(&bom, "\u{feff}{\"a\":1}".as_bytes()).unwrap();
        let e4 = ManifestEntry { rel: "gamedata/flightmodels/bom.blkx".into(), size: None, ..entry };
        assert!(verify_file(&bom, &e4).is_ok());

        let _ = fs::remove_dir_all(&dir);
    }
}
