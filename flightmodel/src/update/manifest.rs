//! 下载源、版本文件与下载清单。
//!
//! # 两个源，固定顺序（用户口径 P9：**只从 github.com / api.github.com 取**）
//!
//! 1. `github.com/<repo>/raw/<ref>/…`（GitHub 原站 raw 直链，会 302 到 raw 主机）
//! 2. `api.github.com`（GitHub API，`git/blobs/<sha>` 原始输出；**吃 60 次/小时的配额**）
//!
//! 逐文件按这个顺序试，全部失败 → 报网络错误并让整轮更新**失败**
//! （不静默跳过文件、不伪造内容）。
//!
//! ⚠️ **`cdn.jsdelivr.net` 这条源已按用户要求删除**（连镜像与备用 jsDelivr 域名一起）：
//! 用户明确"宁可更新失败，也不依赖第三方 CDN"。本机实测 `github.com` /
//! `raw.githubusercontent.com` 会被 reset，之前能用的恰恰是 jsDelivr —— 所以删掉之后
//! 本机很可能**就是**走到"网络不可用"这条明确失败路径，这是**预期取舍**，不是回归。
//!
//! # 清单从哪来
//!
//! 目录清单只有 GitHub 的 git-trees 能给（`raw` 直链不能列目录）。所以：
//!
//! * 正常路径：`contents/<子树>?ref=<tag>`（1 次）→ 按 sha 取 `git/trees/<sha>?recursive=1`
//!   （每个子树 1 次）→ 拿到**每个文件的相对路径 + 字节数 + blob sha**；
//! * `truncated: true` → 直接报错（宁可不更新，也不拿半份清单去覆盖数据）；
//! * API 不可用（限流 403/429、网络不通）→ 退到**本地旧数据的文件清单**：
//!   能继续更新已有文件，但**发现不了上游新增的文件**，且拿不到权威大小
//!   （校验只剩"JSON 可解析"），这时日志里会明确写出来。

use std::path::Path;

use super::net::{self, Canceller, Transport, UpdateError};

/// 上游仓库（datamine 产物，**无许可证**：只允许本地自用，见 D7/D18）。
pub const REPO: &str = "gszabi99/War-Thunder-Datamine";
/// 版本文件在仓库根（不在 `aces.vromfs.bin_u/` 下）。
pub const VERSION_PATH: &str = "version";
/// 默认分支（版本文件、tag 名都从它对齐）。
pub const DEFAULT_REF: &str = "master";
/// 上游数据根（本地数据就是把它下面的内容整体展平到 `resource/data/` 的）。
pub const UPSTREAM_ROOT: &str = "aces.vromfs.bin_u";
/// 上游 gamedata 子树。
pub const UPSTREAM_GAMEDATA: &str = "aces.vromfs.bin_u/gamedata";

/// 下载子集（D4：只下 wp8f 真正读的两棵树）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subset {
    FlightModels,
    Weapons,
}

impl Subset {
    pub fn all() -> [Subset; 2] {
        [Subset::FlightModels, Subset::Weapons]
    }

    /// 子目录名（上游与本地同名）。
    pub fn dir(&self) -> &'static str {
        match self {
            Subset::FlightModels => "flightmodels",
            Subset::Weapons => "weapons",
        }
    }

    /// 相对数据根的目录（本地布局：`<数据根>/gamedata/<dir>`）。
    pub fn local_rel(&self) -> String {
        format!("gamedata/{}", self.dir())
    }

    /// 相对上游仓库根的目录（`aces.vromfs.bin_u/gamedata/<dir>`）。
    pub fn upstream_rel(&self) -> String {
        format!("{UPSTREAM_GAMEDATA}/{}", self.dir())
    }

    pub fn id(&self) -> &'static str {
        self.dir()
    }
}

/// 源种类（P9：**只有 GitHub 自己**，第三方 CDN 已按用户要求删除）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// `github.com/<repo>/raw/<ref>/…`（原站 raw 直链）
    GithubRaw,
    GithubApi,
}

/// 各源的默认主机前缀。
///
/// 前缀是**字段**而不是写死的字面量：离线单测要把源指向本地假源
/// （`http://127.0.0.1:<端口>`），改一个字段就够了，不用给网络层开后门。
pub fn default_base(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::GithubRaw => "https://github.com",
        SourceKind::GithubApi => "https://api.github.com",
    }
}

/// 一个下载源。
#[derive(Debug, Clone)]
pub struct Source {
    pub kind: SourceKind,
    /// 主机前缀（默认见 [`default_base`]）
    pub base: String,
}

impl Source {
    /// 用默认前缀建一个源。
    pub fn new(kind: SourceKind) -> Self {
        Self { kind, base: default_base(kind).to_string() }
    }

    /// 自定义前缀（测试/镜像）。
    pub fn with_base(kind: SourceKind, base: impl Into<String>) -> Self {
        Self { kind, base: base.into() }
    }

    pub fn id(&self) -> &'static str {
        match self.kind {
            SourceKind::GithubRaw => "github",
            SourceKind::GithubApi => "api",
        }
    }

    /// 界面上显示的名字（日志同样用它）。
    pub fn name(&self) -> &'static str {
        match self.kind {
            SourceKind::GithubRaw => "github.com（原站）",
            SourceKind::GithubApi => "api.github.com",
        }
    }

    /// 单个数据文件的下载地址。
    ///
    /// `sha` 只有 `api.github.com` 那条路要用（`git/blobs/<sha>`），
    /// 清单退到"本地旧数据"时拿不到 sha → 该源对这次下载不可用（调用方跳过并记一行日志）。
    pub fn file_url(&self, repo: &str, tag: &str, upstream_path: &str, sha: Option<&str>) -> Option<String> {
        match self.kind {
            // github.com 的 raw 直链（302 到 raw 主机；对外只有一个 github.com 域名）
            SourceKind::GithubRaw => Some(format!("{}/{repo}/raw/{tag}/{upstream_path}", self.base)),
            SourceKind::GithubApi => {
                sha.map(|s| format!("{}/repos/{repo}/git/blobs/{s}", self.base))
            }
        }
    }

    /// 版本文件（仓库根 `version`，纯文本一行）的地址。
    pub fn version_url(&self, repo: &str, ref_name: &str) -> String {
        match self.kind {
            SourceKind::GithubRaw => format!("{}/{repo}/raw/{ref_name}/{VERSION_PATH}", self.base),
            SourceKind::GithubApi => {
                format!("{}/repos/{repo}/contents/{VERSION_PATH}?ref={ref_name}", self.base)
            }
        }
    }

    /// 该源要求的 `Accept:`（`api.github.com` 要 raw 输出，否则拿到的是 base64 JSON）。
    pub fn accept(&self) -> Option<&'static str> {
        match self.kind {
            SourceKind::GithubApi => Some(RAW_ACCEPT),
            _ => None,
        }
    }
}

/// `api.github.com` 的 raw 输出（`contents`/`git/blobs` 都认这个 Accept）。
pub const RAW_ACCEPT: &str = "application/vnd.github.raw";
/// 普通 JSON 输出。
pub const JSON_ACCEPT: &str = "application/vnd.github+json";
/// `api.github.com` 的默认前缀。
pub const API_BASE: &str = "https://api.github.com";

/// 默认源顺序（**用户口径 P9，不要改**）：github.com 原站 → api.github.com。
/// 第三方 CDN（jsDelivr）已按要求删除，不再作为回退。
pub fn default_sources() -> Vec<Source> {
    vec![
        Source::new(SourceKind::GithubRaw),
        Source::new(SourceKind::GithubApi),
    ]
}

/// 清单里的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    /// 相对**数据根**的路径（`gamedata/flightmodels/fm/a-20g.blkx`）——落盘就用它。
    pub rel: String,
    /// 上游仓库里的路径（`aces.vromfs.bin_u/gamedata/flightmodels/fm/a-20g.blkx`）。
    pub upstream: String,
    /// 上游给出的字节数（本地清单退路时是 `None`）。
    pub size: Option<u64>,
    /// 上游 blob sha（`api.github.com` 源要用）。
    pub sha: Option<String>,
}

/// 清单来源（界面/日志要如实说明"清单是不是权威的"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestSource {
    /// GitHub git-trees（权威：含路径、大小、sha）
    GithubTree,
    /// 本地旧数据的目录树（兜底：发现不了新增文件、没有权威大小）
    LocalTree,
}

impl ManifestSource {
    pub fn id(self) -> &'static str {
        match self {
            ManifestSource::GithubTree => "github-tree",
            ManifestSource::LocalTree => "local-tree",
        }
    }
}

/// 一份完整清单。
#[derive(Debug, Clone)]
pub struct Manifest {
    pub entries: Vec<ManifestEntry>,
    pub total_bytes: u64,
    pub source: ManifestSource,
    /// 给日志/界面用的一句人话说明。
    pub note: String,
}

// ------------------------------------------------------------------ 版本文件 --

/// 取远端版本号：按源顺序试，全部失败 → `Err(网络不可用)`（附每个源的失败原因）。
pub fn fetch_version(
    transport: &dyn Transport,
    sources: &[Source],
    repo: &str,
    ref_name: &str,
    tmp_dir: &Path,
    cancel: &dyn Canceller,
) -> Result<(String, &'static str), UpdateError> {
    let mut failures: Vec<String> = Vec::new();
    for src in sources {
        if cancel.cancelled() {
            return Err(UpdateError::cancelled());
        }
        let url = src.version_url(repo, ref_name);
        match net::get_text(transport, &url, src.accept(), tmp_dir, 4096, cancel) {
            Ok(text) => match first_line(&text) {
                Some(v) => return Ok((v, src.id())),
                None => failures.push(format!("{}：响应为空", src.name())),
            },
            Err(e) => {
                if e.kind == net::ErrorKind::Cancelled {
                    return Err(e);
                }
                failures.push(format!("{}：{}", src.name(), e.message));
            }
        }
    }
    Err(UpdateError::network(format!(
        "所有下载源都拿不到远端版本号（{}）；{}",
        sources.iter().map(|s| s.name()).collect::<Vec<_>>().join(" → "),
        failures.join("；")
    )))
}

/// 取第一个非空行（版本文件是"纯文本一行"）。
pub fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.to_string())
}

// -------------------------------------------------------------------- 清单 --

/// 从 GitHub 取清单；失败时退到本地旧数据的目录树（见模块头）。
pub fn fetch_manifest(
    transport: &dyn Transport,
    repo: &str,
    tag: &str,
    subsets: &[Subset],
    tmp_dir: &Path,
    data_root: &Path,
    api_base: &str,
    cancel: &dyn Canceller,
) -> Result<Manifest, UpdateError> {
    match fetch_manifest_github(transport, repo, tag, subsets, tmp_dir, api_base, cancel) {
        Ok(m) => Ok(m),
        Err(e) if e.kind == net::ErrorKind::Cancelled => Err(e),
        Err(e) => {
            // API 不可用（限流/断网）不是"致命"：本地旧清单还能把已有文件更新上去
            let mut m = local_manifest(data_root, subsets)?;
            m.note = format!(
                "GitHub 清单取不到（{}），改用本地旧数据的文件清单：能更新已有文件，\
                 但**发现不了上游新增文件**，且没有权威大小（只校验 JSON 可解析）",
                e.message
            );
            Ok(m)
        }
    }
}

/// 正常路径：`contents` 定位子树 sha → `git/trees` 递归列出每个 blob。
pub fn fetch_manifest_github(
    transport: &dyn Transport,
    repo: &str,
    tag: &str,
    subsets: &[Subset],
    tmp_dir: &Path,
    api_base: &str,
    cancel: &dyn Canceller,
) -> Result<Manifest, UpdateError> {
    // 1 次 contents 拿到 gamedata 下各子树的 sha
    let contents_url =
        format!("{api_base}/repos/{repo}/contents/{UPSTREAM_GAMEDATA}?ref={tag}");
    let text = net::get_text(transport, &contents_url, Some(JSON_ACCEPT), tmp_dir, 4 << 20, cancel)?;
    let dirs = parse_contents_dirs(&text)?;

    let mut entries: Vec<ManifestEntry> = Vec::new();
    for subset in subsets {
        let sha = dirs
            .iter()
            .find(|(name, _)| name == subset.dir())
            .map(|(_, sha)| sha.clone())
            .ok_or_else(|| {
                UpdateError::protocol(format!(
                    "上游 {UPSTREAM_GAMEDATA} 下没有 `{}` 目录（tag {tag}）",
                    subset.dir()
                ))
            })?;
        let tree_url = format!("{api_base}/repos/{repo}/git/trees/{sha}?recursive=1");
        // 实测 flightmodels 的递归树 JSON 约 4.3 MB / 13.6k 条目
        let tree_text = net::get_text(transport, &tree_url, Some(JSON_ACCEPT), tmp_dir, 64 << 20, cancel)?;
        let mut got = parse_tree(&tree_text, &subset.local_rel(), &subset.upstream_rel())?;
        entries.append(&mut got);
    }
    entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    let total_bytes = entries.iter().map(|e| e.size.unwrap_or(0)).sum();
    Ok(Manifest {
        entries,
        total_bytes,
        source: ManifestSource::GithubTree,
        note: format!("GitHub git-trees（tag {tag}）：权威清单（含大小与 blob sha）"),
    })
}

/// `contents` 响应 → `(名字, sha)` 列表（只收 `type == "dir"`）。
pub fn parse_contents_dirs(text: &str) -> Result<Vec<(String, String)>, UpdateError> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| UpdateError::protocol(format!("contents 响应不是 JSON：{e}")))?;
    let arr = v
        .as_array()
        .ok_or_else(|| UpdateError::protocol("contents 响应不是数组（可能是限流错误体）"))?;
    let mut out = Vec::new();
    for item in arr {
        if item.get("type").and_then(|t| t.as_str()) != Some("dir") {
            continue;
        }
        let (Some(name), Some(sha)) = (
            item.get("name").and_then(|x| x.as_str()),
            item.get("sha").and_then(|x| x.as_str()),
        ) else {
            continue;
        };
        out.push((name.to_string(), sha.to_string()));
    }
    Ok(out)
}

/// `git/trees/<sha>?recursive=1` 响应 → 清单项（只收 `type == "blob"`）。
pub fn parse_tree(text: &str, local_prefix: &str, upstream_prefix: &str) -> Result<Vec<ManifestEntry>, UpdateError> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| UpdateError::protocol(format!("tree 响应不是 JSON：{e}")))?;
    if v.get("truncated").and_then(|t| t.as_bool()).unwrap_or(false) {
        return Err(UpdateError::protocol(
            "GitHub 的递归树被截断（truncated=true）——拒绝用半份清单覆盖本地数据",
        ));
    }
    let tree = v
        .get("tree")
        .and_then(|t| t.as_array())
        .ok_or_else(|| UpdateError::protocol("tree 响应里没有 `tree` 数组"))?;
    let mut out = Vec::with_capacity(tree.len());
    for item in tree {
        if item.get("type").and_then(|t| t.as_str()) != Some("blob") {
            continue;
        }
        let Some(path) = item.get("path").and_then(|p| p.as_str()) else {
            continue;
        };
        out.push(ManifestEntry {
            rel: format!("{local_prefix}/{path}"),
            upstream: format!("{upstream_prefix}/{path}"),
            size: item.get("size").and_then(|s| s.as_u64()),
            sha: item.get("sha").and_then(|s| s.as_str()).map(|s| s.to_string()),
        });
    }
    Ok(out)
}

/// 本地旧数据的文件清单（兜底）：扫 `<数据根>/gamedata/<subset>` 下的所有文件。
///
/// `size` 一律 `None`：本地是 legacy blkx、上游是 JSON（体积差 1.41×），拿本地大小当
/// 期望值会**必然**校验失败。
pub fn local_manifest(data_root: &Path, subsets: &[Subset]) -> Result<Manifest, UpdateError> {
    let mut entries = Vec::new();
    for subset in subsets {
        let dir = data_root.join(&subset.local_rel());
        walk(&dir, &subset.local_rel(), &subset.upstream_rel(), &mut entries)?;
    }
    entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    if entries.is_empty() {
        return Err(UpdateError::network(
            "GitHub 清单取不到，本地也没有旧数据可以当清单（首次更新必须先能访问 api.github.com）",
        ));
    }
    let count = entries.len();
    Ok(Manifest {
        entries,
        total_bytes: 0,
        source: ManifestSource::LocalTree,
        note: format!("本地旧数据清单：{count} 个文件（无权威大小）"),
    })
}

fn walk(dir: &Path, local_prefix: &str, upstream_prefix: &str, out: &mut Vec<ManifestEntry>) -> Result<(), UpdateError> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(net::classify_io(&e, &format!("读目录 {} 失败", dir.display()))),
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            walk(&path, &format!("{local_prefix}/{name}"), &format!("{upstream_prefix}/{name}"), out)?;
        } else {
            out.push(ManifestEntry {
                rel: format!("{local_prefix}/{name}"),
                upstream: format!("{upstream_prefix}/{name}"),
                size: None,
                sha: None,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subset_paths_agree_with_fm_paths() {
        assert_eq!(Subset::FlightModels.local_rel(), crate::fm_paths::FLIGHTMODELS_SUBDIR);
        assert_eq!(Subset::Weapons.local_rel(), crate::fm_paths::WEAPONS_SUBDIR);
        assert_eq!(
            Subset::FlightModels.upstream_rel(),
            "aces.vromfs.bin_u/gamedata/flightmodels"
        );
    }

    #[test]
    fn source_order_is_the_agreed_one() {
        let ids: Vec<&str> = default_sources().iter().map(|s| s.id()).collect();
        assert_eq!(ids, vec!["github", "api"],
                   "P9 用户口径：**只有 GitHub 自己**（原站 + API），第三方 CDN 不许出现在列表里");
        assert!(
            default_sources().iter().all(|s| s.name().contains("github")),
            "源的显示名只允许 github.com / api.github.com：{:?}",
            default_sources().iter().map(|s| s.name()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn source_urls_match_the_measured_endpoints() {
        let srcs = default_sources();
        let fm = Subset::FlightModels;
        let up = format!("{}/a-20g.blkx", fm.upstream_rel());
        assert_eq!(
            srcs[0].file_url(REPO, "2.58.0.35", &up, None).unwrap(),
            "https://github.com/gszabi99/War-Thunder-Datamine/raw/2.58.0.35/\
             aces.vromfs.bin_u/gamedata/flightmodels/a-20g.blkx"
        );
        // api 源必须带 blob sha（没有 sha 就没有地址）
        assert!(srcs[1].file_url(REPO, "2.58.0.35", &up, None).is_none());
        assert_eq!(
            srcs[1].file_url(REPO, "2.58.0.35", &up, Some("deadbeef")).unwrap(),
            "https://api.github.com/repos/gszabi99/War-Thunder-Datamine/git/blobs/deadbeef"
        );
        assert_eq!(
            srcs[0].version_url(REPO, DEFAULT_REF),
            "https://github.com/gszabi99/War-Thunder-Datamine/raw/master/version"
        );
        assert_eq!(
            srcs[1].version_url(REPO, DEFAULT_REF),
            "https://api.github.com/repos/gszabi99/War-Thunder-Datamine/contents/version?ref=master"
        );
    }

    #[test]
    fn contents_response_keeps_only_dirs() {
        let text = r#"[
            {"name":"flightmodels","type":"dir","sha":"d1012adfb7c9f7fb9460793d87d662c4a4d35003"},
            {"name":"weapons","type":"dir","sha":"9bf2689934b96d6113b14a717739f22185700c66"},
            {"name":"readme.md","type":"file","sha":"x"}
        ]"#;
        let dirs = parse_contents_dirs(text).expect("合法");
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs[0].0, "flightmodels");
        assert!(!parse_contents_dirs("{\"message\":\"API rate limit exceeded\"}").is_ok());
    }

    #[test]
    fn tree_response_is_mapped_to_local_layout() {
        let text = r#"{"sha":"x","truncated":false,"tree":[
            {"path":"a-20g.blkx","mode":"100644","type":"blob","sha":"93d9e4","size":98785},
            {"path":"fm","mode":"040000","type":"tree","sha":"aaa"},
            {"path":"fm/a-20g.blkx","mode":"100644","type":"blob","sha":"bbbb","size":64552}
        ]}"#;
        let got = parse_tree(text, "gamedata/flightmodels", "aces.vromfs.bin_u/gamedata/flightmodels")
            .expect("合法");
        assert_eq!(got.len(), 2, "tree 条目不进清单");
        assert_eq!(got[0].rel, "gamedata/flightmodels/a-20g.blkx");
        assert_eq!(got[1].upstream, "aces.vromfs.bin_u/gamedata/flightmodels/fm/a-20g.blkx");
        assert_eq!(got[0].size, Some(98785));
        assert_eq!(got[1].sha.as_deref(), Some("bbbb"));
    }

    #[test]
    fn truncated_tree_is_refused() {
        let text = r#"{"truncated":true,"tree":[{"path":"a","type":"blob","size":1}]}"#;
        let err = parse_tree(text, "gamedata/flightmodels", "u").unwrap_err();
        assert_eq!(err.kind, net::ErrorKind::Protocol);
    }

    #[test]
    fn version_text_takes_first_nonempty_line() {
        assert_eq!(first_line("\n 2.59.0.43 \r\n").as_deref(), Some("2.59.0.43"));
        assert_eq!(first_line("   \n\n").as_deref(), None);
    }
}
