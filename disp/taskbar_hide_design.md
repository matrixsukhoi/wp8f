# Windows 任务栏图标隐藏方案

## 研究结论

### winit现状
- winit 0.31.0-beta.2 内部已有`set_skip_taskbar`实现（`winit-win32/src/window.rs:1548`）
- 该函数使用Windows COM接口`ITaskbarList::DeleteTab`
- 但该函数未公开暴露给外部使用

### 可行方案

#### 方案A: 使用raw-window-handle + windows-sys调用COM接口（推荐）

```rust
#[cfg(target_os = "windows")]
fn hide_from_taskbar(hwnd: isize) {
    use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::UI::Shell::{ITaskbarList, CLSID_TaskbarList, IID_ITaskbarList};
    
    unsafe {
        // Initialize COM
        CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        
        // Create ITaskbarList instance
        let mut taskbar: *mut std::ffi::c_void = std::ptr::null_mut();
        CoCreateInstance(&CLSID_TaskbarList, None, CLSCTX_ALL, &IID_ITaskbarList, &mut taskbar);
        
        // Call DeleteTab
        let taskbar = taskbar as *mut ITaskbarList;
        ((*taskbar).lpVtbl.as_ref().unwrap().DeleteTab)(taskbar, hwnd as *mut std::ffi::c_void);
    }
}
```

**需要的windows-sys features:**
```toml
windows-sys = { version = "0.59", features = [
    "Win32_System_Com",
    "Win32_UI_Shell",
    "Win32_Foundation"
]}
```

#### 方案B: 使用WS_EX_TOOLWINDOW窗口样式

在窗口创建后设置扩展样式：

```rust
#[cfg(target_os = "windows")]
fn set_tool_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongW, SetWindowLongW, GWL_EXSTYLE};
    use windows_sys::Win32::Foundation::HWND;
    
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style | 0x00000080); // WS_EX_TOOLWINDOW = 0x00000080
    }
}
```

**优缺点对比:**

| 方案 | 优点 | 缺点 |
|------|------|------|
| 方案A (ITaskbarList) | 彻底从任务栏移除 | 需要COM初始化 |
| 方案B (WS_EX_TOOLWINDOW) | 简单直接 | 可能影响alt+tab显示 |

### 获取HWND

使用`raw-window-handle` crate：

```rust
use raw_window_handle::HasWindowHandle;

if let Ok(handle) = window.window_handle() {
    if let RawWindowHandle::Win32(win32_handle) = handle.as_raw_handle() {
        let hwnd = win32_handle.hwnd.get() as isize;
        // 调用上述函数
    }
}
```

### 下一步

1. 在`Cargo.toml`添加`windows-sys`依赖
2. 在`can_create_surfaces`中获取HWND并调用上述函数
3. 测试两种方案效果
