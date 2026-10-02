//! 相对路径 → 真实文件的**基准候选**（单一真源）：cwd → cwd 的上一级 → exe 目录 → exe 目录的上一级。
//!
//! `font_path` 与 `hud_lang_path` 都走这里。以前两处各抄一份、顺序还是反的
//! （字体那份代码是"上一级优先"、注释却写"cwd 优先"），于是"字体能加载、语言表不能"
//! 这类问题查不出来。顺序只在本文件定。
//!
//! 仓库根直接跑时 cwd 就是根；产物在 `binary/` 时 exe 的上一级才是根。

use std::path::PathBuf;

/// 基准目录（按上面的顺序，去重）。
pub fn bases() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    if let Ok(cwd) = std::env::current_dir() {
        push(cwd.clone());
        if let Some(up) = cwd.parent() {
            push(up.to_path_buf());
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            push(dir.to_path_buf());
            if let Some(up) = dir.parent() {
                push(up.to_path_buf());
            }
        }
    }
    // 单测：cargo 的 cwd 是包目录（`disp/`），上面几条都找不到仓库根
    #[cfg(test)]
    push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cwd 优先（与模块头同口径），且去重 —— 顺序一旦反了，两处解析会各挑一个目录。
    #[test]
    fn bases_start_at_cwd_and_are_deduplicated() {
        let b = bases();
        assert!(!b.is_empty());
        assert_eq!(b[0], std::env::current_dir().unwrap(), "cwd 必须排第一");
        let mut uniq = b.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), b.len(), "基准不重复");
    }
}
