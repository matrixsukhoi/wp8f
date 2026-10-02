//! 把 `icon.rc` 里的图标资源编进 `wp8f-gui.exe`。
//!
//! 交叉编译（WSL + mingw）用 `x86_64-w64-mingw32-windres`：编出 COFF 目标文件后
//! 用 `rustc-link-arg-bins` 交给链接器（不加新依赖，也不需要 `.rc` 之外的构建脚本工具）。
//! 环境里没有 windres 时**只警告不失败** —— 图标是外观，不该拦住构建；
//! 那种情况下 exe 里没有图标资源，托盘会自动退回内嵌字节那条路（见 `src/tray.rs`）。

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=icon.rc");
    println!("cargo:rerun-if-changed=logo.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return; // 非 Windows 目标没有 PE 资源这回事
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR 由 cargo 提供"));
    let obj = out.join("wp8f-gui-icon.o");
    let Some(windres) = find_windres() else {
        println!(
            "cargo:warning=没找到 windres，wp8f-gui.exe 不带图标资源（装 mingw-w64 或设 WINDRES 环境变量）"
        );
        return;
    };
    let status = Command::new(&windres)
        .args(["-O", "coff", "--include-dir", ".", "-i", "icon.rc", "-o"])
        .arg(&obj)
        .status();
    match status {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg-bins={}", obj.display()),
        Ok(s) => println!("cargo:warning=windres 退出码 {s}，wp8f-gui.exe 不带图标资源"),
        Err(e) => println!("cargo:warning=windres 无法执行（{e}），wp8f-gui.exe 不带图标资源"),
    }
}

/// windres 的候选顺序：`WINDRES` 环境变量 → mingw 习惯名 → 裸名（PATH 里有就用）。
fn find_windres() -> Option<String> {
    if let Ok(v) = std::env::var("WINDRES") {
        if !v.is_empty() {
            return Some(v);
        }
    }
    ["x86_64-w64-mingw32-windres", "windres"]
        .into_iter()
        .find(|c| Command::new(c).arg("--version").output().is_ok_and(|o| o.status.success()))
        .map(str::to_string)
}
