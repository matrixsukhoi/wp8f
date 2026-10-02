//! FM 数据库更新器：**版本检查 → 逐文件下载 → 校验 → 写暂存 version → 用户二次确认才切换**。
//!
//! ```text
//! resource/data/            ← 当前生效（切换成功前一个字节都不动）
//! resource/data_new/        ← 暂存：下载 + 校验完成后才写 version（= "可切换"标志）
//! resource/data_old/        ← 换下来的旧数据（回滚源，只留最近一份）
//! ```
//!
//! 分工：
//! * [`version`] 版本号解析与比较（按数值分段，不假设等差）；
//! * [`manifest`] 下载源顺序、远端版本文件、下载清单（GitHub git-trees / 本地兜底）；
//! * [`net`] 网络层（内置 http 客户端 + 系统 curl，错误按"网络/校验/磁盘/权限"分类）；
//! * [`download`] 并发下载器（逐文件重试、多源回退、文件粒度续传、进度、取消）；
//! * [`switch`] A/B 分区切换（两步改名 + 失败回滚）。
//!
//! [`UpdateManager`] 是给 GUI 用的门面：异步跑检查/下载，同步跑切换，状态随时可读。
//! **合规**：上游仓库没有许可证、内容本身是 Gaijin 的版权数据 —— 只允许下载到本地自用，
//! **不随包分发**（`scripts/zip.sh` 默认就排除 `resource/data*`）。

pub mod download;
pub mod manifest;
pub mod net;
pub mod switch;
pub mod version;

#[cfg(test)]
pub mod mock;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use download::{DownloadConfig, FailedFile, Progress};
use manifest::{Manifest, ManifestSource, Source};
use net::{Canceller, HttpTransport, Transport, UpdateError};
use version::Version;

/// 日志环形缓冲的容量（界面上就是一小块滚动区域）。
pub const LOG_CAPACITY: usize = 200;

/// 更新器的三条路径（都从仓库根推出来，与 `fm_paths` 同一套常量）。
#[derive(Debug, Clone)]
pub struct UpdatePaths {
    /// `resource/`
    pub resource_dir: PathBuf,
    /// `resource/data`（当前生效）
    pub data_root: PathBuf,
    /// `resource/data_new`（暂存）
    pub staging_root: PathBuf,
    /// 临时文件目录（`resource/data_new` 之外，避免被清单/校验当成数据）
    pub tmp_dir: PathBuf,
}

impl UpdatePaths {
    pub fn from_repo_root(root: &Path) -> Self {
        let resource_dir = root.join(crate::fm_paths::RESOURCE_DIR);
        Self {
            data_root: resource_dir.join(crate::fm_paths::DATA_DIR_NAME),
            staging_root: resource_dir.join(crate::fm_paths::DATA_NEW_DIR_NAME),
            tmp_dir: resource_dir.join(".wp8f_update_tmp"),
            resource_dir,
        }
    }
}

/// 更新器当前处在哪一步（界面按它决定显示什么按钮）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 什么都没做
    Idle,
    /// 正在查远端版本号
    Checking,
    /// 正在下载（含逐文件校验）
    Downloading,
    /// 下载完了，正在做整体核对 + 写暂存 version
    Finalizing,
    /// 暂存就绪，等用户点「更新到下载新版本」
    Ready,
    /// 正在切换（改名）
    Switching,
    /// 切换完成
    Done,
    /// 失败（看 `error`）
    Failed,
}

impl Phase {
    pub fn id(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Checking => "checking",
            Phase::Downloading => "downloading",
            Phase::Finalizing => "finalizing",
            Phase::Ready => "ready",
            Phase::Switching => "switching",
            Phase::Done => "done",
            Phase::Failed => "failed",
        }
    }

    /// 正在进行中（用来禁用按钮 / 显示"取消"）。
    pub fn busy(self) -> bool {
        matches!(self, Phase::Checking | Phase::Downloading | Phase::Finalizing | Phase::Switching)
    }
}

/// 状态快照（GUI 每次轮询拿一份，转成 JSON）。
#[derive(Debug, Clone)]
pub struct Status {
    pub phase: Phase,
    pub local_version: Option<String>,
    pub remote_version: Option<String>,
    /// 远端比本地新（本地版本未知而远端已知时也算"有"——那是"还没装数据"的情形）
    pub update_available: bool,
    /// 暂存目录里的版本号（写进去了 = 下载 + 校验全部完成）
    pub staging_version: Option<String>,
    /// 暂存已就绪、可以切换
    pub staging_ready: bool,
    pub files_total: u64,
    pub files_done: u64,
    pub files_failed: u64,
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub retries: u64,
    pub failed: Vec<FailedFile>,
    pub disabled_sources: Vec<download::DisabledSource>,
    pub error: Option<UpdateError>,
    /// 清单来源说明（权威清单 / 本地兜底）
    pub manifest_note: String,
    pub manifest_authoritative: bool,
    /// 本机构建是否带 `fm-json`（上游数据是 JSON；不带就没法解析切换后的数据）
    pub json_supported: bool,
    pub log: Vec<String>,
    pub data_root: String,
    pub staging_root: String,
    /// **上一版数据根是否存在**（固定单槽 `resource/data_old`；P9 用户口径：不再有时间戳目录）。
    /// 界面据此提示"可以直接删掉它释放空间"。
    pub old_exists: bool,
    /// 上一版数据根的路径（`resource/data_old`，给界面显示用）。
    pub old_root: String,
    /// 切换后**不会保留**的顶层条目（旧数据根里有、暂存根里没有）
    pub dropped_top_level: Vec<String>,
}

impl Status {
    fn idle(paths: &UpdatePaths) -> Self {
        Self {
            phase: Phase::Idle,
            local_version: None,
            remote_version: None,
            update_available: false,
            staging_version: None,
            staging_ready: false,
            files_total: 0,
            files_done: 0,
            files_failed: 0,
            bytes_total: 0,
            bytes_done: 0,
            retries: 0,
            failed: Vec::new(),
            disabled_sources: Vec::new(),
            error: None,
            manifest_note: String::new(),
            manifest_authoritative: false,
            json_supported: crate::parser::FM_JSON_ENABLED,
            log: Vec::new(),
            data_root: paths.data_root.display().to_string(),
            staging_root: paths.staging_root.display().to_string(),
            old_exists: false,
            old_root: switch::old_root_path(&paths.resource_dir).display().to_string(),
            dropped_top_level: Vec::new(),
        }
    }
}

/// 内部可变状态（阶段 + 上一次的结果）。
#[derive(Debug, Default)]
struct Core {
    phase: Option<Phase>,
    remote_version: Option<String>,
    error: Option<UpdateError>,
    manifest: Option<Manifest>,
}

/// 更新器门面（GUI 一个进程一份；内部自带线程与取消标志）。
pub struct UpdateManager {
    paths: UpdatePaths,
    transport: Box<dyn Transport>,
    /// 下载源（顺序即回退顺序；默认三个真实源，单测里换成本地假源）
    sources: Vec<Source>,
    /// `api.github.com` 前缀（清单只能从 API 拿）
    api_base: String,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<Progress>>,
    core: Mutex<Core>,
    log: Mutex<VecDeque<String>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    /// 日志记录起点（日志行前缀 `+12.3s` 用；不依赖任何时区/时间库）。
    started: std::time::Instant,
    /// 下载目标版本（下载线程写、状态读）
    target_tag: Mutex<Option<String>>,
    /// 并发上限（运行期可调：单测调小、将来界面也可给一个"低速"档）
    concurrency: std::sync::atomic::AtomicU32,
    /// 每个文件的尝试次数上限
    attempts: std::sync::atomic::AtomicU32,
    /// 某个源连续失败多少次后本轮停用
    source_fail_limit: std::sync::atomic::AtomicU32,
}

impl UpdateManager {
    pub fn new(paths: UpdatePaths) -> Arc<Self> {
        Self::build(paths, Box::new(HttpTransport::from_env()), manifest::default_sources(), manifest::API_BASE)
    }

    /// 自定义传输 + 源（离线单测用本地假源；将来要支持镜像/代理也从这里进）。
    pub fn with_transport(
        paths: UpdatePaths,
        transport: Box<dyn Transport>,
        sources: Vec<Source>,
        api_base: &str,
    ) -> Arc<Self> {
        Self::build(paths, transport, sources, api_base)
    }

    fn build(
        paths: UpdatePaths,
        transport: Box<dyn Transport>,
        sources: Vec<Source>,
        api_base: &str,
    ) -> Arc<Self> {
        Arc::new(Self {
            paths,
            transport,
            sources,
            api_base: api_base.to_string(),
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Mutex::new(Progress::default())),
            core: Mutex::new(Core::default()),
            log: Mutex::new(VecDeque::new()),
            worker: Mutex::new(None),
            started: std::time::Instant::now(),
            target_tag: Mutex::new(None),
            concurrency: std::sync::atomic::AtomicU32::new(6),
            attempts: std::sync::atomic::AtomicU32::new(3),
            source_fail_limit: std::sync::atomic::AtomicU32::new(5),
        })
    }

    /// 并发上限（钳到 1..=32）。
    pub fn set_concurrency(&self, n: u32) {
        self.concurrency.store(n.clamp(1, 32), Ordering::Relaxed);
    }

    /// 每个文件的尝试次数上限（钳到 1..=10）。
    pub fn set_attempts(&self, n: u32) {
        self.attempts.store(n.clamp(1, 10), Ordering::Relaxed);
    }

    /// 某个源连续失败多少次后本轮停用（钳到 1..=100）。
    pub fn set_source_fail_limit(&self, n: u32) {
        self.source_fail_limit.store(n.clamp(1, 100), Ordering::Relaxed);
    }

    pub fn paths(&self) -> &UpdatePaths {
        &self.paths
    }

    /// 写一行 `[UPDATE] …`（前缀是"距本次启动的秒数"，便于看耗时）。
    pub fn push_log(&self, line: impl AsRef<str>) {
        let ms = self.started.elapsed().as_millis();
        let mut ring = self.log.lock().unwrap_or_else(|e| e.into_inner());
        if ring.len() >= LOG_CAPACITY {
            ring.pop_front();
        }
        ring.push_back(format!("[+{}.{:03}s] {}", ms / 1000, ms % 1000, line.as_ref()));
    }

    fn log_lines(&self) -> Vec<String> {
        self.log
            .lock()
            .map(|g| g.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn phase(&self) -> Phase {
        self.core
            .lock()
            .ok()
            .and_then(|c| c.phase)
            .unwrap_or(Phase::Idle)
    }

    fn set_phase(&self, phase: Phase) {
        if let Ok(mut c) = self.core.lock() {
            c.phase = Some(phase);
        }
    }

    fn set_error(&self, err: Option<UpdateError>) {
        if let Ok(mut c) = self.core.lock() {
            if err.is_some() {
                c.phase = Some(Phase::Failed);
            }
            c.error = err;
        }
    }

    fn set_manifest(&self, m: Manifest) {
        if let Ok(mut c) = self.core.lock() {
            c.manifest = Some(m);
        }
    }

    /// 当前状态快照（会做少量读盘：本地版本、旧目录清单、暂存版本）。
    pub fn status(&self) -> Status {
        let mut st = Status::idle(&self.paths);
        {
            let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            st.phase = core.phase.unwrap_or(Phase::Idle);
            st.remote_version = core.remote_version.clone();
            st.error = core.error.clone();
            if let Some(m) = core.manifest.as_ref() {
                st.manifest_note = m.note.clone();
                st.manifest_authoritative = m.source == ManifestSource::GithubTree;
            }
        }
        st.local_version = switch::read_version(&self.paths.data_root);
        st.staging_version = switch::read_version(&self.paths.staging_root);
        st.update_available = match (&st.local_version, &st.remote_version) {
            (Some(local), Some(remote)) => {
                matches!(version::compare(local, remote), Some(std::cmp::Ordering::Less) | None)
            }
            (None, Some(_)) => true,
            _ => false,
        };
        {
            let p = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            st.files_total = p.files_total;
            st.files_done = p.files_done;
            st.files_failed = p.files_failed;
            st.bytes_total = p.bytes_total;
            st.bytes_done = p.bytes_done;
            st.retries = p.retries;
            st.failed = p.failed.clone();
            st.disabled_sources = p.disabled_sources.clone();
        }
        st.staging_ready = match (&st.staging_version, &st.remote_version) {
            (Some(s), Some(r)) => s == r,
            _ => false,
        };
        st.old_exists = switch::has_old_root(&self.paths.resource_dir);
        let plan = switch::SwitchPlan::new(&self.paths.resource_dir, switch::unix_now());
        st.dropped_top_level = switch::dropped_top_level(&plan);
        st.log = self.log_lines();
        st
    }

    /// 版本检查（同步、快）：拉远端 `version` → 与本地比。
    pub fn check(&self) -> Result<Status, UpdateError> {
        if self.phase().busy() {
            return Err(UpdateError::new(
                net::ErrorKind::Io,
                "更新器正忙（下载/切换中），先取消或等它结束",
            ));
        }
        self.cancel.store(false, Ordering::Relaxed);
        self.set_phase(Phase::Checking);
        self.set_error(None);
        self.push_log("[UPDATE] 检查更新：读取上游版本号");
        let tmp = &self.paths.tmp_dir;
        let result = manifest::fetch_version(
            self.transport.as_ref(),
            &self.sources,
            manifest::REPO,
            manifest::DEFAULT_REF,
            tmp,
            self.cancel.as_ref(),
        );
        match result {
            Ok((remote, src)) => {
                self.push_log(format!("[UPDATE] 远端版本 {remote}（源：{src}）"));
                let local = switch::read_version(&self.paths.data_root);
                match &local {
                    Some(l) => self.push_log(format!("[UPDATE] 本地版本 {l}（{}）", self.paths.data_root.display())),
                    None => self.push_log("[UPDATE] 本地版本未知（没找到 resource/data/version）"),
                }
                match (&local, version::compare(local.as_deref().unwrap_or(""), &remote)) {
                    (Some(l), Some(std::cmp::Ordering::Less)) => {
                        self.push_log(format!("[UPDATE] 有新版本：{l} → {remote}"))
                    }
                    (Some(_), Some(std::cmp::Ordering::Equal)) => {
                        self.push_log("[UPDATE] 已是最新版本".to_string())
                    }
                    (Some(_), Some(std::cmp::Ordering::Greater)) => {
                        self.push_log("[UPDATE] 本地版本比上游还新（自建数据？）".to_string())
                    }
                    (_, None) => self.push_log(format!(
                        "[UPDATE] 版本号格式不认识（本地 {:?} / 远端 {remote}）：只如实显示，不判断新旧",
                        local
                    )),
                    (None, _) => self.push_log("[UPDATE] 本地还没有数据：可以下载一份".to_string()),
                }
                {
                    let mut c = self.core.lock().unwrap_or_else(|e| e.into_inner());
                    c.remote_version = Some(remote.clone());
                    c.phase = Some(Phase::Idle);
                    c.error = None;
                }
                if let Ok(mut t) = self.target_tag.lock() {
                    *t = Some(remote);
                }
                Ok(self.status())
            }
            Err(e) => {
                self.push_log(e.log_line());
                self.set_error(Some(e.clone()));
                Err(e)
            }
        }
    }

    /// 开始下载（后台线程）：清单 → 逐文件下载 + 校验 → 整体核对 → 写暂存 `version`。
    ///
    /// `tag` 为空时用最近一次 `check()` 的远端版本号。
    pub fn start_download(self: &Arc<Self>, tag: Option<String>) -> Result<(), UpdateError> {
        if self.phase().busy() {
            return Err(UpdateError::new(net::ErrorKind::Io, "更新器正忙，先取消或等它结束"));
        }
        let tag = match tag.or_else(|| self.target_tag.lock().ok().and_then(|t| t.clone())) {
            Some(t) => t,
            None => {
                return Err(UpdateError::new(
                    net::ErrorKind::Io,
                    "还不知道要下载哪个版本：先点「检查更新」",
                ))
            }
        };
        if Version::parse(&tag).is_none() {
            return Err(UpdateError::protocol(format!(
                "目标版本号 {tag:?} 不是 `2.59.0.43` 这种格式，拒绝用它当 tag 下载"
            )));
        }
        {
            let mut p = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            *p = Progress::default();
        }
        self.cancel.store(false, Ordering::Relaxed);
        self.set_error(None);
        self.set_phase(Phase::Downloading);
        self.push_log(format!("[UPDATE] 目标版本 {tag}；暂存目录 {}", self.paths.staging_root.display()));

        let me = Arc::clone(self);
        let handle = std::thread::Builder::new()
            .name("wp8f-update".to_string())
            .spawn(move || me.run_download(tag))
            .map_err(|e| UpdateError::io(format!("启动下载线程失败：{e}")))?;
        if let Ok(mut w) = self.worker.lock() {
            *w = Some(handle);
        }
        Ok(())
    }

    fn run_download(self: Arc<Self>, tag: String) {
        let result = self.download_pipeline(&tag);
        match result {
            Ok(()) => {
                self.set_phase(Phase::Ready);
                self.push_log("[UPDATE] 暂存数据已就绪：等用户点「更新到下载新版本」");
            }
            Err(e) if e.kind == net::ErrorKind::Cancelled => {
                self.push_log("[UPDATE] 已取消（已完成的文件保留，下次继续）");
                self.set_error(None);
                self.set_phase(Phase::Idle);
            }
            Err(e) => {
                self.push_log(e.log_line());
                self.set_error(Some(e));
            }
        }
        if let Ok(mut w) = self.worker.lock() {
            *w = None;
        }
    }

    fn download_pipeline(&self, tag: &str) -> Result<(), UpdateError> {
        let cfg = DownloadConfig {
            repo: manifest::REPO.to_string(),
            tag: tag.to_string(),
            staging_root: self.paths.staging_root.clone(),
            tmp_dir: self.paths.tmp_dir.clone(),
            sources: self.sources.clone(),
            concurrency: self.concurrency.load(Ordering::Relaxed) as usize,
            attempts: self.attempts.load(Ordering::Relaxed) as usize,
            source_fail_limit: self.source_fail_limit.load(Ordering::Relaxed),
            resume_parts: false,
        };
        let cancel: &dyn Canceller = self.cancel.as_ref();
        let transport: &dyn Transport = self.transport.as_ref();
        let log = |line: String| self.push_log(line);

        let m = manifest::fetch_manifest(
            transport,
            manifest::REPO,
            tag,
            &manifest::Subset::all(),
            &self.paths.tmp_dir,
            &self.paths.data_root,
            &self.api_base,
            cancel,
        )?;
        self.push_log(format!("[UPDATE] 清单：{}（{} 个文件 / {}）", m.note, m.entries.len(), download::human_bytes(m.total_bytes)));
        self.set_manifest(m.clone());

        download::download_all(transport, &cfg, &m, &self.progress, &log, cancel)?;

        self.set_phase(Phase::Finalizing);
        self.finalize_staging(&m, tag)?;
        Ok(())
    }

    /// 整体核对 + 写暂存 `version`（**只有全部通过才写**，它就是"可切换"的标志）。
    fn finalize_staging(&self, m: &Manifest, tag: &str) -> Result<(), UpdateError> {
        self.push_log("[UPDATE] 整体核对：文件数 / 总字节 / JSON 可解析性");
        let (files, bytes) = verify_against_manifest(&self.paths.staging_root, m)?;
        self.push_log(format!(
            "[UPDATE] 核对通过：{files} 个文件 / {}",
            download::human_bytes(bytes)
        ));
        let vpath = self.paths.staging_root.join(crate::fm_paths::VERSION_FILE);
        std::fs::write(&vpath, format!("{tag}\n"))
            .map_err(|e| net::classify_io(&e, &format!("写 {} 失败", vpath.display())))?;
        self.push_log(format!("[UPDATE] 已写暂存版本文件：{} = {tag}", vpath.display()));
        Ok(())
    }

    /// 拿一份能用来校验的清单：优先用本轮下载缓存的那份；
    /// 没有（进程重启过 / 换了入口）就**重新拉一次**（走同一个多源 + 本地兜底）。
    /// 两条路都不通 → 报错，**绝不"没有清单就跳过校验"**。
    fn manifest_for_verify(&self, tag: &str) -> Result<Manifest, UpdateError> {
        if let Some(m) = self
            .core
            .lock()
            .ok()
            .and_then(|c| c.manifest.clone())
            .filter(|m| !m.entries.is_empty())
        {
            return Ok(m);
        }
        self.push_log("[UPDATE] 本地没有本次下载的清单，重新拉一次（校验必须有对照）");
        let m = manifest::fetch_manifest(
            self.transport.as_ref(),
            manifest::REPO,
            tag,
            &manifest::Subset::all(),
            &self.paths.tmp_dir,
            &self.paths.data_root,
            &self.api_base,
            self.cancel.as_ref(),
        )?;
        self.set_manifest(m.clone());
        Ok(m)
    }

    /// **切换前的全量校验**（对用户可见的入口）：对照清单逐文件核对
    /// 大小 + JSON 可解析性。**只读**：任何一条不通过都返回 `Err`，调用方一个字节都不许动。
    pub fn verify_staging(&self) -> Result<Vec<String>, UpdateError> {
        let tag = self.target_tag()?;
        let m = self.manifest_for_verify(&tag)?;
        let (files, bytes) = verify_against_manifest(&self.paths.staging_root, &m)?;
        Ok(vec![format!(
            "校验通过：{files} 个文件 / {}",
            download::human_bytes(bytes)
        )])
    }

    /// 目标版本号（没查过远端就用暂存 `version` 兜底）。
    fn target_tag(&self) -> Result<String, UpdateError> {
        if let Some(t) = self.target_tag.lock().ok().and_then(|t| t.clone()) {
            return Ok(t);
        }
        switch::read_version(&self.paths.staging_root).ok_or_else(|| {
            UpdateError::new(
                net::ErrorKind::Io,
                "还不知道要更新到哪个版本：先点「检查更新」，或先完成一次下载",
            )
        })
    }

    /// 取消（下载线程在下个文件边界退出；正在下的小文件最多再等一会儿）。
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.push_log("[UPDATE] 用户取消：正在收尾（已完成的文件保留）");
    }

    /// 是否正在忙（界面禁用按钮）。
    pub fn is_busy(&self) -> bool {
        self.phase().busy()
    }

    /// **更新到下载的新版本**（用户确认之后调这里），顺序：
    /// 1. **HUD 在跑就拒绝** —— 不杀进程、不动任何文件（`hud_running` 分类）；
    /// 2. 先对下载目录做**全量校验**（对照清单：文件数 / 总字节 / 逐文件 size + JSON 可解析）；
    /// 3. 全部通过之后才：写暂存 `version` → 旧数据根改名成 `data_old`（回滚源）
    ///    → `data_new` 改名成 `data`；
    /// 4. 任一步失败：回滚，现有 `resource/data` 一个字节都不变；校验失败时**保留下载目录**
    ///    （写清是哪个文件、什么原因），用户可以重下或「清除下载」。
    pub fn apply_switch(&self, hud_running: bool) -> Result<Vec<String>, UpdateError> {
        // ① HUD 在跑：直接拒绝，一个字节都不动
        if hud_running {
            let err = UpdateError::new(
                net::ErrorKind::HudRunning,
                "HUD（wp8f.exe）正在运行：请先关闭它再更新（更新器不会替你结束它）",
            );
            self.push_log(err.log_line());
            self.set_error(Some(err.clone()));
            return Err(err);
        }
        let phase = self.phase();
        if phase == Phase::Downloading || phase == Phase::Finalizing || phase == Phase::Checking {
            return Err(UpdateError::new(net::ErrorKind::Io, "下载/校验还没结束，先等它完成"));
        }
        let tag = self.target_tag()?;
        self.set_phase(Phase::Finalizing);
        self.set_error(None);

        // ② 先校验下载目录（只读；失败 → 保留下载目录，data 不动）
        self.push_log("[UPDATE] 更新前校验：对照清单逐文件核对（大小 + JSON 可解析）");
        let verified = self.verify_staging();
        let log = match verified {
            Ok(lines) => lines,
            Err(e) => {
                self.push_log(e.log_line());
                self.push_log("[UPDATE] 校验没过：下载目录保留，现有数据一个字节都没动");
                self.set_error(Some(e.clone()));
                return Err(e);
            }
        };
        for l in &log {
            self.push_log(format!("[UPDATE] {l}"));
        }

        // ③ 校验通过才写 version（= "可以换根"的标记）
        let vpath = self.paths.staging_root.join(crate::fm_paths::VERSION_FILE);
        if let Err(e) = std::fs::write(&vpath, format!("{tag}\n")) {
            let err = net::classify_io(&e, &format!("写 {} 失败", vpath.display()));
            self.push_log(err.log_line());
            self.set_error(Some(err.clone()));
            return Err(err);
        }
        self.push_log(format!("[UPDATE] 校验全部通过，已写 {} = {tag}", vpath.display()));

        // ④ 换根（三步改名 + 失败回滚；单槽 `data_old`，P9 用户口径）
        self.set_phase(Phase::Switching);
        let plan = switch::SwitchPlan::now(&self.paths.resource_dir);
        match switch::apply_switch(&plan, &tag) {
            Ok(lines) => {
                for l in &lines {
                    self.push_log(format!("[UPDATE] {l}"));
                }
                self.push_log("[UPDATE] 更新完成：HUD 需重启后生效（数据是启动时读的）");
                {
                    let mut c = self.core.lock().unwrap_or_else(|e| e.into_inner());
                    c.phase = Some(Phase::Done);
                }
                Ok(lines)
            }
            Err(e) => {
                self.push_log(e.log_line());
                self.set_error(Some(e.clone()));
                Err(e)
            }
        }
    }

    /// **清除下载目录**：删掉 `resource/data_new` 与 `resource/.wp8f_update_tmp`，回到 Idle。
    ///
    /// 用户口径：校验没过时**保留下载目录**供重试或取消 —— 这条就是"取消"的落地。
    /// 正在下载/切换中不许清（先把下载取消掉）。
    pub fn discard_staging(&self) -> Result<Vec<String>, UpdateError> {
        if self.phase().busy() {
            return Err(UpdateError::new(
                net::ErrorKind::Io,
                "更新器正忙（下载/切换中）：先点「取消」再清除下载目录",
            ));
        }
        let mut log = Vec::new();
        for dir in [&self.paths.staging_root, &self.paths.tmp_dir] {
            if !dir.exists() {
                continue;
            }
            std::fs::remove_dir_all(dir)
                .map_err(|e| net::classify_io(&e, &format!("删除 {} 失败", dir.display())))?;
            log.push(format!("已删除 {}", dir.display()));
        }
        self.cancel.store(false, Ordering::Relaxed);
        {
            let mut p = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            *p = Progress::default();
        }
        {
            let mut c = self.core.lock().unwrap_or_else(|e| e.into_inner());
            c.phase = Some(Phase::Idle);
            c.error = None;
            c.manifest = None;
        }
        for l in &log {
            self.push_log(format!("[UPDATE] {l}"));
        }
        if log.is_empty() {
            self.push_log("[UPDATE] 没有需要清除的下载目录");
        }
        Ok(log)
    }
}

/// 对照清单逐文件核对一个数据根（**只读**）：文件数 / 总字节 / 每个文件的 size + JSON 可解析。
///
/// 下载收尾（`finalize_staging`）与更新前（`verify_staging`）用的是**同一份实现** ——
/// 两处各写一遍就会出现"下载时说通过、更新时说不通过"这种没法排查的分裂。
/// 报错一律带上**哪个文件、什么原因**（用户口径：校验失败要给明确错误）。
fn verify_against_manifest(root: &Path, m: &Manifest) -> Result<(u64, u64), UpdateError> {
    if !root.is_dir() {
        return Err(UpdateError::verify(format!(
            "下载目录不存在：{}（先完成一次下载）",
            root.display()
        )));
    }
    let mut files = 0u64;
    let mut bytes = 0u64;
    for e in &m.entries {
        let path = root.join(&e.rel);
        if !path.is_file() {
            return Err(UpdateError::verify(format!("下载目录缺少文件：{}", e.rel)));
        }
        download::verify_file(&path, e)?;
        files += 1;
        bytes += std::fs::metadata(&path).map(|md| md.len()).unwrap_or(0);
    }
    if files != m.entries.len() as u64 {
        return Err(UpdateError::verify(format!(
            "文件数不符：清单 {} 个，实际 {files} 个",
            m.entries.len()
        )));
    }
    if m.total_bytes > 0 && bytes != m.total_bytes {
        return Err(UpdateError::verify(format!(
            "总字节不符：清单 {}，实际 {}",
            download::human_bytes(m.total_bytes),
            download::human_bytes(bytes)
        )));
    }
    // 清单里没列到、下载目录里却多出来的文件也要说清楚（不会被换根，但用户该知道）
    Ok((files, bytes))
}
