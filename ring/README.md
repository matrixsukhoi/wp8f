# ring —— 无锁「最新值」环形缓冲区（`wp8f-ring`）

## 职责

给「一个生产端不停写、多个消费端只关心最新一版」的场景提供共享内存原语：HUD 的帧缓冲区
（`wp8f-disp`）与数据链状态槽（`wp8f-datalink`）共用它。零依赖、无锁、无 `Arc`、
运行期不分配 —— 槽在构造时一次分配完，消费端读的是槽的借用而不是拷贝。

## 实现原理

* **环形 + 单调计数**。槽数组只有两个游标：`write_index`（最新已发布槽的下标，在
  `0..slots` 内回绕）与 `writes`（单调写计数，只增不减）。回绕判断只能靠 `writes`：
  下标会绕回，计数不会。
* **单生产端**。`next_slot()` 返回下一个待写槽（`write_index + 1`），`commit()` 才把
  `write_index` 指向它。生产端独占「待写槽」，所以写的过程中读者看到的仍是上一版，
  不存在半成品可见的问题。
* **读者只借最新槽**。`latest()` / `poll()` 直接返回 `&T`，不拷贝、不加锁。代价是
  生产者绕满一圈后会改写读者正持有的那一槽 —— 读者跨过一整圈就会读到新旧混合的值。
  这一版作废，但**不会**读到驻留堆内存的悬挂引用（见下面的载荷约束）。
* **发布顺序（Release/Acquire 配对）**。`commit()` 先 `store(write_index, Release)`
  再 `fetch_add(writes, Release)`；`poll()` 先读下标、后读计数。看到 `writes = N+1`
  就必然能看到第 N+1 版内容；反过来会出现「新计数 + 旧下标」，把上一版重复交付一次。
* **代际（generation）**。生产端重连／重建时 `reset()` 让代数 +1，消费端下次 `poll()`
  拿到 `reset = true` 并把当前版本当作重置后的第一版（`skipped = 0`），避免把重置前
  看不到的版本算成「跳过」而在极端时序下下溢。

## 结构图

```
            生产端（唯一）                        消费端（任意多个，各持一个 Cursor）
   ┌──────────────────────────────┐        ┌────────────────────────────────────────┐
   │ next_slot() → &mut T         │        │ poll(&mut Cursor) -> Poll { value,      │
   │   写 write_index + 1 号槽     │        │        skipped, reset }                │
   │ commit()                     │        │   · 先读 write_index，后读 writes       │
   │   write_index ← Release      │        │   · value   = &槽[write_index]（借用）  │
   │   writes      ← Release +1   │        │   · skipped = writes - seen - 1         │
   └──────────────┬───────────────┘        └───────────────┬────────────────────────┘
                  │                                        │
                  ▼                                        ▼
        ┌─────────────────────────────────────────────────────────┐
        │ slots: Box<[UnsafeCell<T>]>   0   1   2  ...  N-1        │
        │   ▲ 已发布（读者可借）    ▲ 待写（生产端独占）            │
        └─────────────────────────────────────────────────────────┘
          N = slots（默认 32；config `ring.frames` / `ring.map_samples` 可配，钳在 2..=256）
```

发布与轮询的配对时序：

```
生产端                                消费端
  commit():                             poll():
    write_index.store(next, Release) ─┐   idx   = write_index.load(Acquire)   ← 先
    writes.fetch_add(1, Release)      │   total = writes.load(Acquire)        ← 后
                                      └─► total != cursor.seen ⇒ 有新版本
                                          value = &slots[idx]（内容必然已是第 idx 版）
```

## 代码目录

| 路径 | 内容 |
|---|---|
| `src/lib.rs` | 全部实现：`Ring<T>`、`Cursor`、`Poll<T>` 与 6 个单元测试 |

单文件 crate，没有子模块 —— 原语足够小，拆开只会让契约与实现分家。

## 关键函数 Specification

### `pub fn with_slots(slots: usize) -> Ring<T>` / `pub fn new() -> Ring<T>`
* 输入：`slots` —— 槽数，`new()` 用默认值 `SLOTS = 32`（30 Hz 下 ≈ 1 秒覆盖层）。
* 输出：槽全部为 `T::default()` 的环，写计数与代数均为 0。
* 前置：`T: Default`（要一次分配出全部槽）。
* 后置：`slots() == slots.max(MIN_SLOTS)`；此后**不再分配**。
* 错误：无。低于下限不报错，静默钳到 `MIN_SLOTS = 2`（1 个槽时「下一个待写槽」就是
  正在被读的那个，环的语义不成立）。
* 副作用：一次 `Vec → Box<[_]>` 分配。

### `pub fn latest(&self) -> &T`
* 输入：无。
* 输出：当前最新已发布槽的借用；从未发布过时是槽 0 的 `T::default()`。
* 前置：单生产端（与 `commit()` 的发布顺序配对）。
* 后置：无 —— 不推进任何游标，重复调用返回同一版。
* 错误：无。
* 副作用：无（只读两个原子量 + 一次 `&*`）。

### `pub fn next_slot(&self) -> &mut T`
* 输入：无。
* 输出：下一个待发布槽的可变借用（`write_index + 1` 号，回绕）。
* 前置：**调用方必须是唯一生产端**；两次 `next_slot()` 之间必须 `commit()`。
* 后置：该槽在 `commit()` 前对读者不可见。
* 错误：无。
* 副作用：无 —— 纯粹把「待写槽」交出去。

### `pub fn publish(&self, value: T)` / `pub fn commit(&self)`
* 输入：`publish` 取一版新值；`commit` 无输入。
* 输出：无。
* 前置：`commit()` 只能发布刚由 `next_slot()` 写好的那一槽。
* 后置：`writes` +1、`write_index` 前移；两者按 Release 顺序发布。
* 错误：无。
* 副作用：改写一个槽的内容（`publish`）+ 发布两个原子量。

### `pub fn cursor(&self) -> Cursor` / `pub fn reset(&self)`
* `cursor()`：在生产端**当前**状态处取游标 —— 历史版本不补发，新建消费者（如重连后
  的 logger）从此刻开始收。输入无、错误无、副作用无。
* `reset()`：代数 +1，消费端下次 `poll()` 得到 `reset = true`。
  **不清空值**：要不要顺带 `publish()` 一份空值由调用方决定。错误无。

### `pub fn poll<'a>(&'a self, cursor: &mut Cursor) -> Poll<'a, T>`
* 输入：`cursor` —— 该消费者独占的进度（`seen` 写计数 + `generation` 代数）。
* 输出：`Poll { value, skipped, reset }`；自上次轮询以来没有新版本时 `value = None`。
* 前置：`cursor` 由调用线程独占（`Cursor: Copy`，可随意重置或丢弃）。
* 后置：有新版本时 `cursor.seen` 推进到当前写计数；检测到 `reset` 时同时刷新代数。
* 错误：无 —— 不返回 `Result`、不 panic（差值一律 `saturating_sub`）。
* 副作用：只写 `cursor`；不触碰环的原子量与槽内容。

### 诊断量
* `writes() -> u64`：历史总写入次数（单调）。消费者据此上报丢帧率。
* `generation() -> u64`：重置次数。
* `slots() -> usize`：槽数 —— `disp` 的读者用它判断「落后几版算被覆盖」
  （`skipped + 1 > slots()`）。

## 约束与已知取舍

* **载荷必须是 POD**。消费端拿的是槽的借用，生产端绕满一圈会改写同一槽：值类型只会读到
  新旧混合的数据（这一版作废，不影响下一版）；`Vec`/`String` 这类带堆指针的类型会被
  读者在脚下释放（use-after-free），**禁止入环**。真实载荷：`DisplayData`、`OwnState`、
  `Snap`。
* **唯一例外**：`disp` 的**地图快照槽** `Ring<MapDisplay>`（`disp/src/lib.rs` 的
  `MAP_RING_BUFFER`）—— 载荷里的 `map_objects: Vec<MapObjectDisplay>` 是堆指针。它按地图
  采样频率写（约 4 Hz），读者要落后 `map_samples` 个采样（缺省 32 ≈ 8 s）才会撞上写端改写，
  实测不可能发生，按可容许处理（例外在槽声明处就地注明）。底图 `MapImage` **不在**环里：
  它走 `OnceLock<Mutex<MapImage>>` + `Arc` 快照（换图不打断正在画的读者）。
* **无保留、无检测、无重试**。环不替读者占住槽，也不检测「你正在读的槽被覆盖了」；
  `skipped` / `overwritten` 只是统计。混合读被容许（用户裁定：读者落后 8 秒「几乎不可能
  发生」），因此不引入 seqlock 重试。
* **多生产端不设防**（用户裁定）：两个线程同时 `next_slot()` 会互相踩。
* `unsafe impl Send/Sync` 的依据写在实现处：槽访问全靠上述原子量同步，`T: Send` 保证值能
  跨线程移动、`T: Send + Sync` 保证共享出的 `&T` 能跨线程读。

## 测试

`src/lib.rs` 内 6 个单元测试，覆盖：槽数可配与钳位、未写入时 `latest()` 为默认值、
`poll()` 取最新并如实累计 `skipped`、`cursor()` 不补发历史、`reset()` 不让 `skipped`
下溢、绕圈后 `latest()` 与 `writes` 仍是最后一版/单调。
