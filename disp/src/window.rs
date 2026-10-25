//! 窗口、环形缓冲的消费端、帧发布：`create_window` / 两个平台的 `run_window` /
//! `destroy_window`，以及 `FrameReader`（记录线程只取最新帧）与 `begin_frame`…`get_map_data`。
//!
//! 从 `lib.rs` 拆出的一块；对外路径不变（`lib.rs` 有 `pub use window::{...}`）。
//! 这里只做"起窗口 / 喂帧 / 发帧消息"，不决定画什么：渲染在 `hud/`、`mini_hud.rs`、
//! `circle_hud.rs`，布局快照在 `layout.rs`。`DispError` 也归这里：它只装窗口错误。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use thiserror::Error;
use winit::event_loop::EventLoopProxy;

use super::drag::DragState;
use super::layout::hud_layout;
use super::state::{DisplayData, MapDisplay};

#[derive(Error, Debug)]
pub enum DispError {
    #[error("Window error: {0}")]
    WindowError(String),
}

static WINDOW_VISIBLE_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_window_visible(visible: bool) {
    WINDOW_VISIBLE_REQUESTED.store(visible, std::sync::atomic::Ordering::Relaxed);
    if let Some(proxy) = PROXY.get() {
        let _ = proxy.wake_up();
    }
    tracing::debug!("[DISP] set_window_visible({})", visible);
}

struct DispWindow {
    running: Arc<std::sync::atomic::AtomicBool>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

type DataRingBuffer = wp8f_ring::Ring<DisplayData>;
type MapDataRingBuffer = wp8f_ring::Ring<MapDisplay>;

/// 一次轮询的统计结果（`FrameReader::poll_latest`）：`frame` 是最新一帧的**借用**
/// （直读缓冲区，零拷贝；本轮没有新帧则 `None`）；`skipped` 是被跳过的历史帧数
/// （`frame=Some` 时 = 本轮新帧数 - 1）；`overwritten` = 新帧数超过环形容量；
/// `reset` = 检测到生产端重置（帧序号断流）。
#[derive(Debug, Clone, Copy, Default)]
pub struct PollOutcome {
    pub frame: Option<&'static DisplayData>,
    pub skipped: u64,
    pub overwritten: bool,
    pub reset: bool,
}

/// 环形缓冲区的**消费端句柄**（logger 线程持有）。可 `Send`：内部只有一个 `&'static` 与游标。
///
/// 消费语义是"只取最新帧、不 drain 历史帧"：记录频率 = 轮询频率，logger 不会积压。
pub struct FrameReader {
    rb: &'static DataRingBuffer,
    cursor: wp8f_ring::Cursor,
}

impl FrameReader {
    fn new(rb: &'static DataRingBuffer) -> Self {
        // 起点与"当前"同步：本句柄只关心创建之后写入的帧（logger 每个连接新建一次，
        // 不该补记上一个连接的帧，也不该把创建前刚发生的重置报成本次连接的 reset）。
        Self { rb, cursor: rb.cursor() }
    }

    /// 只取最新一帧的**借用**（零拷贝、零锁）：不推进生产端游标、不清历史帧。
    ///
    /// 内存序：索引/计数按 `Acquire` 读、`commit()` 按 `Release` 发布；生产端只写
    /// `next_slot()` 的槽、不回写已发布的槽 → 拿到的总是已提交的完整帧。
    /// 例外：读者被延迟到生产端绕满一整圈（`ring.frames` 个槽，缺省 32 帧 ≈ 1 s @30 Hz）
    /// 后改写同一槽，
    /// 这时读到的是新旧混合的数据 —— **这是"零拷贝直读"接受的代价**（`DisplayData` 是 POD，
    /// 混读只影响这一帧）。
    pub fn poll_latest(&mut self) -> PollOutcome {
        let out = self.rb.poll(&mut self.cursor);
        if out.reset {
            // 生产端重置过（每次重连都会调 `reset_ring_buffer()`）：之前的帧序号全部失效 ——
            // 重置本身不是一帧数据
            return PollOutcome { reset: true, ..Default::default() };
        }
        match out.value {
            None => PollOutcome::default(),
            Some(frame) => PollOutcome {
                frame: Some(frame),
                skipped: out.skipped,
                overwritten: out.skipped + 1 > self.rb.slots() as u64,
                reset: false,
            },
        }
    }
}

/// 取 `DisplayData` 环形缓冲区的消费端（logger 线程用）。
/// 窗口未创建（`RING_BUFFER` 未初始化）时返回 `None`。
pub fn frame_reader() -> Option<FrameReader> {
    RING_BUFFER.get().map(|rb| FrameReader::new(rb.as_ref()))
}

pub fn reset_ring_buffer() {
    if let Some(rb) = RING_BUFFER.get() {
        rb.reset();
    }
}

static DISP_WINDOW: std::sync::Mutex<Option<DispWindow>> = std::sync::Mutex::new(None);
static RING_BUFFER: OnceLock<Arc<DataRingBuffer>> = OnceLock::new();
/// 地图快照槽（4 Hz 写、渲染线程每帧读）。载荷 `MapDisplay` 里的 `map_objects: Vec` 是堆指针，
/// 是对 `ring` 的 POD 契约的**有意例外**：读者要落后 32 个地图采样（≈8 s）才会撞上写端的
/// `clear()`，实测不可能发生（渲染线程每帧都来取），按可容许处理。
/// 底图**不在**这个槽里（见 [`MapImage`]：环形槽轮着用，进环就得每次发布重写）。
static MAP_RING_BUFFER: OnceLock<Arc<MapDataRingBuffer>> = OnceLock::new();

pub fn create_window() -> Result<(), DispError> {
    tracing::debug!("[DISP] create_window() called");

    {
        let mut disp = DISP_WINDOW.lock().unwrap();
        if let Some(old_disp) = disp.take() {
            tracing::warn!("[DISP] Window already exists, destroying first");
            old_disp
                .running
                .store(false, std::sync::atomic::Ordering::Relaxed);
            if let Some(handle) = old_disp.thread_handle {
                let _ = handle.join();
            }
        }
    }

    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    // 槽数来自配置（`ring` 段，已钳制）：这里是唯一的分配点，运行期不再分配
    let ring_buffer = Arc::new(DataRingBuffer::with_slots(hud_layout().ring.frames_clamped()));
    let ring_buffer_for_display = ring_buffer.clone();
    let ring_buffer_for_core = ring_buffer.clone();

    let thread_handle = thread::spawn({
        let running = running.clone();
        move || {
            run_window(ring_buffer_for_display, running);
        }
    });

    RING_BUFFER.set(ring_buffer_for_core).ok();
    MAP_RING_BUFFER
        .set(Arc::new(MapDataRingBuffer::with_slots(hud_layout().ring.map_samples_clamped())))
        .ok();
    {
        let mut disp = DISP_WINDOW.lock().unwrap();
        *disp = Some(DispWindow {
            running,
            thread_handle: Some(thread_handle),
        });
    }

    thread::sleep(Duration::from_millis(100));
    Ok(())
}

static WINDOW_SIZE: OnceLock<(f32, f32)> = OnceLock::new();

pub fn set_window_size(w: f32, h: f32) {
    WINDOW_SIZE.set((w, h)).ok();
}

pub(crate) fn get_window_size() -> (f32, f32) {
    *WINDOW_SIZE.get().unwrap_or(&(2560.0, 1440.0))
}

#[cfg(target_os = "windows")]
fn run_window(ring_buffer: Arc<DataRingBuffer>, running: Arc<std::sync::atomic::AtomicBool>) {
    use softbuffer::{Context, Surface};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::mem;
    use std::num::NonZeroU32;
    use winit::event_loop::EventLoop;
    use winit::platform::windows::EventLoopBuilderExtWindows;
    use winit::window::WindowId;

    let mut builder = EventLoop::builder();
    builder.with_any_thread(true);
    let event_loop = builder.build().expect("Failed to create event loop");

    PROXY.set(event_loop.create_proxy()).ok();
    tracing::debug!("[DISP] Window created successfully");

    let running = running.clone();

    thread_local! {
        static GC: RefCell<Option<GraphicsContext>> = const { RefCell::new(None) };
    }

    struct GraphicsContext {
        ctx: RefCell<Context<&'static dyn winit::window::Window>>,
        surf: HashMap<
            WindowId,
            Surface<&'static dyn winit::window::Window, &'static dyn winit::window::Window>,
        >,
    }

    impl GraphicsContext {
        fn new(w: &dyn winit::window::Window) -> Self {
            unsafe {
                Self {
                    ctx: RefCell::new(Context::new(mem::transmute(w)).expect("ctx")),
                    surf: HashMap::new(),
                }
            }
        }
        fn get_surf(
            &mut self,
            w: &dyn winit::window::Window,
        ) -> &mut Surface<&'static dyn winit::window::Window, &'static dyn winit::window::Window>
        {
            self.surf.entry(w.id()).or_insert_with(|| unsafe {
                Surface::new(&self.ctx.borrow(), mem::transmute(w)).expect("surf")
            })
        }
    }

    struct App {
        window: Option<Box<dyn winit::window::Window>>,
        ring_buffer: Arc<DataRingBuffer>,
        running: Arc<std::sync::atomic::AtomicBool>,
        last_visible: bool,
        drag_state: DragState,
    }

    impl winit::application::ApplicationHandler for App {
        fn resumed(&mut self, _: &dyn winit::event_loop::ActiveEventLoop) {}

        fn can_create_surfaces(&mut self, el: &dyn winit::event_loop::ActiveEventLoop) {
            use winit::window::WindowLevel;
            let (width, height) = get_window_size();
            let drag_mode = crate::is_drag_mode();

            let attr = winit::window::WindowAttributes::default()
                .with_title("WP8F HUD")
                .with_surface_size(winit::dpi::LogicalSize::new(width, height))
                .with_decorations(false)
                .with_transparent(true)
                .with_visible(true)
                .with_active(drag_mode)
                .with_window_level(WindowLevel::AlwaysOnTop);

            self.window = el.create_window(attr).ok();
            if let Some(ref w) = self.window {
                if !drag_mode {
                    let _ = w.set_cursor_visible(false);
                }

                #[cfg(target_os = "windows")]
                {
                    use winit::platform::windows::WindowExtWindows;
                    w.set_skip_taskbar(true);
                }

                if !drag_mode {
                    let _ = w.set_cursor_hittest(false);
                }

                let _ = w.set_outer_position(winit::dpi::Position::Physical(
                    winit::dpi::PhysicalPosition::new(0, 0),
                ));
            }
        }

        fn window_event(
            &mut self,
            tgt: &dyn winit::event_loop::ActiveEventLoop,
            _: WindowId,
            evt: winit::event::WindowEvent,
        ) {
            use winit::event::WindowEvent;
            match evt {
                WindowEvent::CloseRequested => {
                    // 预览里点窗口关闭（或 Alt+F4）也要先把拖拽结果落盘（ESC 那条早有保存）
                    if crate::is_drag_mode() {
                        crate::save_drag_state(&self.drag_state);
                    }
                    tgt.exit();
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if crate::is_drag_mode()
                        && event.state == winit::event::ElementState::Pressed
                        && event.physical_key
                            == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Escape)
                    {
                        // ESC：保存布局并直接退出进程
                        crate::save_drag_state(&self.drag_state);
                        std::process::exit(0);
                    }
                }
                WindowEvent::PointerMoved { position, .. } => {
                    if crate::is_drag_mode() {
                        if let Some(panel) = self.drag_state.active {
                            let (x, y) = (position.x as i32 - self.drag_state.off_x,
                                          position.y as i32 - self.drag_state.off_y);
                            self.drag_state.set_panel_pos(panel, crate::hud_layout().hud_type, x, y);
                        }
                    }
                }
                WindowEvent::PointerButton { state: winit::event::ElementState::Pressed, position, .. } => {
                    if crate::is_drag_mode() {
                        self.drag_state.handle_press(position.x as i32, position.y as i32);
                    }
                }
                WindowEvent::PointerButton { state: winit::event::ElementState::Released, .. } => {
                    if self.drag_state.active.is_some() {
                        crate::save_drag_state(&self.drag_state);
                    }
                    self.drag_state.active = None;
                }
                WindowEvent::RedrawRequested => {
                    GC.with(|gc| {
                        let Some(ref win) = self.window else { return };
                        let sz = win.surface_size();
                        let (Some(w), Some(h)) =
                            (NonZeroU32::new(sz.width), NonZeroU32::new(sz.height))
                        else {
                            return;
                        };
                        let ww = w.get() as usize;
                        let hh = h.get() as usize;
                        let mut gc = gc.borrow_mut();
                        let s = gc
                            .get_or_insert_with(|| GraphicsContext::new(win.as_ref()))
                            .get_surf(win.as_ref());
                        s.resize(w, h).ok();
                        let mut buf = s.buffer_mut().expect("buf");
                        crate::hud::draw_hud(&mut buf, ww, hh, self.ring_buffer.latest(), &self.drag_state);
                        buf.present().ok();
                    });
                }
                _ => {}
            }
        }

        fn about_to_wait(&mut self, tgt: &dyn winit::event_loop::ActiveEventLoop) {
            use std::sync::atomic::Ordering;
            let visible_requested =
                WINDOW_VISIBLE_REQUESTED.load(std::sync::atomic::Ordering::Relaxed);

            if let Some(ref w) = self.window {
                if visible_requested != self.last_visible {
                    self.last_visible = visible_requested;
                    let _ = w.set_visible(visible_requested);
                }
                if FRAME_READY.swap(false, Ordering::Acquire) {
                    w.request_redraw();
                }
                tgt.set_control_flow(winit::event_loop::ControlFlow::Wait);
            }
            if !self.running.load(std::sync::atomic::Ordering::Relaxed) {
                tgt.exit();
            }
        }
    }

    let _ = event_loop.run_app(App {
        window: None,
        ring_buffer,
        running,
        last_visible: true,
        drag_state: DragState::from_config(&crate::hud_layout()),
    });
}

#[cfg(not(target_os = "windows"))]
fn run_window(ring_buffer: Arc<DataRingBuffer>, running: Arc<std::sync::atomic::AtomicBool>) {
    use softbuffer::Surface;
    use std::cell::RefCell;
    use std::num::NonZeroU32;
    use winit::event_loop::EventLoop;
    use winit::window::WindowId;

    let event_loop = EventLoop::builder()
        .build()
        .expect("Failed to create event loop");
    PROXY.set(event_loop.create_proxy()).ok();
    let running = running.clone();

    thread_local! {
        static GC: RefCell<Option<GraphicsContext>> = const { RefCell::new(None) };
    }

    struct GraphicsContext {
        ctx: softbuffer::Context<&'static dyn winit::window::Window>,
    }

    impl GraphicsContext {
        fn new(w: &dyn winit::window::Window) -> Self {
            unsafe {
                Self {
                    ctx: softbuffer::Context::new(std::mem::transmute(w)).expect("ctx"),
                }
            }
        }
    }

    struct App {
        window: Option<Box<dyn winit::window::Window>>,
        ring_buffer: Arc<DataRingBuffer>,
        running: Arc<std::sync::atomic::AtomicBool>,
        last_visible: bool,
        drag_state: DragState,
    }

    impl winit::application::ApplicationHandler for App {
        fn resumed(&mut self, _: &dyn winit::event_loop::ActiveEventLoop) {}
        fn can_create_surfaces(&mut self, el: &dyn winit::event_loop::ActiveEventLoop) {
            use winit::window::WindowLevel;
            self.window = el
                .create_window(
                    winit::window::WindowAttributes::default()
                        .with_title("WP8F HUD")
                        .with_surface_size(winit::dpi::LogicalSize::new(
                            get_window_size().0,
                            get_window_size().1,
                        ))
                        .with_decorations(false)
                        .with_transparent(true)
                        .with_visible(true)
                        .with_window_level(WindowLevel::AlwaysOnTop),
                )
                .ok();
        }
        fn window_event(
            &mut self,
            tgt: &dyn winit::event_loop::ActiveEventLoop,
            _: WindowId,
            evt: winit::event::WindowEvent,
        ) {
            use winit::event::WindowEvent;
            match evt {
                WindowEvent::CloseRequested => {
                    // 预览里点窗口关闭（或 Alt+F4）也要先把拖拽结果落盘（ESC 那条早有保存）
                    if crate::is_drag_mode() {
                        crate::save_drag_state(&self.drag_state);
                    }
                    tgt.exit();
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if crate::is_drag_mode()
                        && event.state == winit::event::ElementState::Pressed
                        && event.physical_key
                            == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Escape)
                    {
                        // ESC：保存布局并直接退出进程
                        crate::save_drag_state(&self.drag_state);
                        std::process::exit(0);
                    }
                }
                WindowEvent::PointerMoved { position, .. } => {
                    if crate::is_drag_mode() {
                        if let Some(panel) = self.drag_state.active {
                            let (x, y) = (position.x as i32 - self.drag_state.off_x,
                                          position.y as i32 - self.drag_state.off_y);
                            self.drag_state.set_panel_pos(panel, crate::hud_layout().hud_type, x, y);
                        }
                    }
                }
                WindowEvent::PointerButton { state: winit::event::ElementState::Pressed, position, .. } => {
                    if crate::is_drag_mode() {
                        self.drag_state.handle_press(position.x as i32, position.y as i32);
                    }
                }
                WindowEvent::PointerButton { state: winit::event::ElementState::Released, .. } => {
                    if self.drag_state.active.is_some() {
                        crate::save_drag_state(&self.drag_state);
                    }
                    self.drag_state.active = None;
                }
                WindowEvent::RedrawRequested => {
                    GC.with(|gc| {
                        let Some(ref w) = self.window else { return };
                        let sz = w.surface_size();
                        let (Some(nw), Some(h)) =
                            (NonZeroU32::new(sz.width), NonZeroU32::new(sz.height))
                        else {
                            return;
                        };
                        let ww = nw.get() as usize;
                        let hh = h.get() as usize;
                        let mut gc = gc.borrow_mut();
                        let ctx = gc.get_or_insert_with(|| GraphicsContext::new(w.as_ref()));
                        let mut surf: softbuffer::Surface<
                            &dyn winit::window::Window,
                            &dyn winit::window::Window,
                        > = unsafe {
                            Surface::new(&ctx.ctx, std::mem::transmute(w.as_ref())).expect("surf")
                        };
                        surf.resize(nw, h).ok();
                        let mut buf = surf.buffer_mut().expect("buf");
                        crate::hud::draw_hud(&mut buf, ww, hh, self.ring_buffer.latest(), &self.drag_state);
                        buf.present().ok();
                    });
                }
                _ => {}
            }
        }
        fn about_to_wait(&mut self, tgt: &dyn winit::event_loop::ActiveEventLoop) {
            use std::sync::atomic::Ordering;
            let visible_requested =
                WINDOW_VISIBLE_REQUESTED.load(std::sync::atomic::Ordering::Relaxed);

            if let Some(ref w) = self.window {
                if visible_requested != self.last_visible {
                    self.last_visible = visible_requested;
                    let _ = w.set_visible(visible_requested);
                }
                if FRAME_READY.swap(false, Ordering::Acquire) {
                    w.request_redraw();
                }
                tgt.set_control_flow(winit::event_loop::ControlFlow::Wait);
            }
            if !self.running.load(std::sync::atomic::Ordering::Relaxed) {
                tgt.exit();
            }
        }
    }

    let _ = event_loop.run_app(App {
        window: None,
        ring_buffer,
        running,
        last_visible: true,
        drag_state: DragState::from_config(&crate::hud_layout()),
    });
}

pub fn destroy_window() {
    tracing::info!("[DISP] destroy_window() called");
    if let Ok(mut disp) = DISP_WINDOW.lock() {
        if let Some(old_disp) = disp.take() {
            old_disp
                .running
                .store(false, std::sync::atomic::Ordering::Relaxed);
            if let Some(proxy) = PROXY.get() {
                let _ = proxy.wake_up();
            }
            if let Some(handle) = old_disp.thread_handle {
                let _ = handle.join();
            }
        }
    }
    thread::sleep(Duration::from_millis(500));
}

static PROXY: OnceLock<EventLoopProxy> = OnceLock::new();
static FRAME_READY: AtomicBool = AtomicBool::new(false);

pub fn signal_redraw() {
    FRAME_READY.store(true, Ordering::Release);
}

pub fn begin_frame() -> Option<&'static mut DisplayData> {
    RING_BUFFER.get().map(|rb| rb.next_slot())
}

pub fn commit_frame() {
    if let Some(rb) = RING_BUFFER.get() {
        rb.commit();
        signal_redraw();
        if let Some(proxy) = PROXY.get() {
            let _ = proxy.wake_up();
        }
    }
}

pub fn begin_map_frame() -> Option<&'static mut MapDisplay> {
    MAP_RING_BUFFER.get().map(|rb| rb.next_slot())
}

pub fn commit_map_frame() {
    if let Some(rb) = MAP_RING_BUFFER.get() {
        rb.commit();
        signal_redraw();
        if let Some(proxy) = PROXY.get() {
            let _ = proxy.wake_up();
        }
    }
}

pub fn get_map_data() -> Option<&'static MapDisplay> {
    MAP_RING_BUFFER.get().map(|rb| rb.latest())
}

#[cfg(test)]
mod tests {
    use super::*;

    // —— 环形缓冲区消费端（logger 解耦）——

    /// 帧环槽数由配置决定（`ring.frames`）：`overwritten` 按**实际槽数**算，不再写死 32。
    #[test]
    fn frame_reader_overwrite_threshold_follows_configured_slots() {
        let rb: &'static DataRingBuffer = Box::leak(Box::new(DataRingBuffer::with_slots(4)));
        assert_eq!(rb.slots(), 4);
        let mut r = FrameReader::new(rb);
        write_frames(rb, 1, 4);
        let out = r.poll_latest();
        assert_eq!(out.frame.expect("最新帧").frame, 4);
        assert_eq!(out.skipped, 3);
        assert!(!out.overwritten, "落后 == 槽数还不算被覆盖");

        write_frames(rb, 5, 5);
        let out = r.poll_latest();
        assert_eq!(out.frame.expect("最新帧").frame, 9);
        assert!(out.overwritten, "落后 > 槽数必须报环形覆盖");
    }

    /// 往本地 `DataRingBuffer` 里提交 `n` 帧，帧号 = `start..start+n`。
    fn write_frames(rb: &DataRingBuffer, start: u64, n: u64) {
        for i in 0..n {
            rb.next_slot().frame = start + i;
            rb.commit();
        }
    }

    /// 空缓冲区 → 没有新帧；生产 N 帧后 → 只返回**最后**一帧，其余 N-1 帧记为跳过。
    #[test]
    fn frame_reader_polls_only_latest_frame() {
        let rb: &'static DataRingBuffer = Box::leak(Box::new(DataRingBuffer::new()));
        let mut r = FrameReader::new(rb);

        let out = r.poll_latest();
        assert!(out.frame.is_none(), "还没写过帧，不应返回帧");
        assert_eq!((out.skipped, out.overwritten, out.reset), (0, false, false));

        write_frames(rb, 1, 5);
        let out = r.poll_latest();
        let f = out.frame.expect("有 5 个新帧，必须返回最新一帧");
        assert_eq!(f.frame, 5, "返回的必须是最后一帧（只取最新，不 drain 历史）");
        assert_eq!(out.skipped, 4, "其余 4 帧应计入 skipped");
        assert!(!out.overwritten, "5 帧 < 容量 32，没有发生环形覆盖");

        // 没有新帧 → 不重复返回同一帧（记录频率 = 轮询频率）
        let out = r.poll_latest();
        assert!(out.frame.is_none(), "两次轮询之间没有新帧，不应重复记录");
        assert_eq!((out.skipped, out.overwritten), (0, false));

        // 只取最新帧：不 drain、不推进生产端游标
        assert_eq!(rb.latest().frame, 5, "消费端不得改动生产端可见的最新帧");
        assert_eq!(rb.writes(), 5, "消费端不得改动写计数");
        write_frames(rb, 6, 3);
        let out = r.poll_latest();
        assert_eq!(out.frame.expect("新帧").frame, 8);
        assert_eq!(out.skipped, 2, "增量按上次轮询之后的写入计数");
    }

    /// 一轮之间写入超过容量 → 报环形覆盖（有帧在被读走前就被覆盖）。
    #[test]
    fn frame_reader_flags_ring_overwrite() {
        let rb: &'static DataRingBuffer = Box::leak(Box::new(DataRingBuffer::new()));
        let mut r = FrameReader::new(rb);

        write_frames(rb, 1, DataRingBuffer::SLOTS as u64 + 8);
        let out = r.poll_latest();
        let f = out.frame.expect("必须返回最新一帧");
        assert_eq!(f.frame, (DataRingBuffer::SLOTS + 8) as u64);
        assert_eq!(out.skipped, DataRingBuffer::SLOTS as u64 + 7);
        assert!(out.overwritten, "落后 > 容量（{} 帧）必须报环形覆盖", DataRingBuffer::SLOTS);
    }

    /// `reset_ring_buffer()`（每次重连都会调）后：计数不溢出、不当成新帧，之后继续正常工作。
    #[test]
    fn frame_reader_survives_ring_reset() {
        let rb: &'static DataRingBuffer = Box::leak(Box::new(DataRingBuffer::new()));
        let mut r = FrameReader::new(rb);

        write_frames(rb, 1, 5);
        assert_eq!(r.poll_latest().frame.expect("首轮").frame, 5);

        rb.reset(); // == reset_ring_buffer()：generation 前进（消费端据此识别断流）
        let out = r.poll_latest();
        assert!(out.reset, "必须识别出这是一次生产端重置");
        assert!(out.frame.is_none(), "重置本身不是一帧数据");
        assert_eq!((out.skipped, out.overwritten), (0, false), "重置不得被算成丢帧/覆盖");

        // 重置前已写过 5 帧：若消费端不识别 generation，这里的差值会算成"倒流"而溢出
        write_frames(rb, 100, 3);
        let out = r.poll_latest();
        assert_eq!(out.frame.expect("重置后的第 3 帧").frame, 102);
        assert_eq!(out.skipped, 2, "重置后只按新的写入增量计数");
        assert!(!out.reset, "没有新的重置");
    }

    /// 反复重置也不累加丢帧
    #[test]
    fn frame_reader_repeated_resets_do_not_accumulate() {
        let rb: &'static DataRingBuffer = Box::leak(Box::new(DataRingBuffer::new()));
        let mut r = FrameReader::new(rb);
        write_frames(rb, 1, 5);
        assert_eq!(r.poll_latest().frame.expect("首轮").frame, 5);
        for _ in 0..3 {
            rb.reset();
            let out = r.poll_latest();
            assert!(out.reset);
            assert_eq!((out.skipped, out.overwritten), (0, false));
        }
    }

    /// 新建的消费端从"当前"开始：创建之前写入的帧不补记，也不产生 reset 噪声
    /// （真实时序：main 先 `reset_ring_buffer()`、再 `frame_reader()` 建 logger 线程）。
    #[test]
    fn frame_reader_created_after_writes_starts_in_sync() {
        let rb: &'static DataRingBuffer = Box::leak(Box::new(DataRingBuffer::new()));
        write_frames(rb, 1, 5);
        rb.reset(); // 重连：generation 前进
        write_frames(rb, 6, 2);

        let mut r = FrameReader::new(rb);
        let out = r.poll_latest();
        assert!(
            out.frame.is_none() && !out.reset && out.skipped == 0,
            "创建时就在同步点：历史帧不补记，创建前的重置也不算本次连接的 reset"
        );

        write_frames(rb, 8, 4);
        let out = r.poll_latest();
        assert_eq!(out.frame.expect("创建后写入的新帧").frame, 11);
        assert_eq!(out.skipped, 3, "只统计创建之后的写入");
        assert!(!out.reset);
    }
}
