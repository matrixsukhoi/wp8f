//! wp8f 飞行记录（`.wpr`）：容器（写 + 读）、记录线程与（可选的）转换器。
//!
//! 记录格式是 wp8f 自己的一套（[`wpr`]：地图信息 + 底图 + 逐帧 `DisplayData`）。
//!
//! # feature
//!
//! | feature | 提供什么 | 谁用 |
//! |---|---|---|
//! | `thread`（默认） | 记录线程 [`spawn_recorder`]：需要 disp 的帧环形缓冲区与最新地图 | core |
//! | `convert` | 纯函数转换器 `convert`（`.wpr` → TacView/ACMI、TacView/CSV、FlatCSV） | 控制台 |
//! | 都不开 | 只有 `wpr` 容器读写（不依赖 disp / winit / 内嵌字体） | 控制台回放解析 |
//!
//! 控制台用 `default-features = false` 调本库解析 `.wpr`，不会把 HUD 那套重依赖带进 `wp8f-gui.exe`。

#[cfg(feature = "convert")]
pub mod convert;
#[cfg(feature = "thread")]
pub mod thread;
pub mod wpr;

#[cfg(feature = "thread")]
pub use thread::{spawn_recorder, RecordConfig, RecordHandle, RecordStats};
