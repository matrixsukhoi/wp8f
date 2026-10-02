# datalink —— Link-8 数据链（`datalink`）

共享飞机飞行状态的轻量数据链：**独立服务端 + wp8f 客户端**，UDP 请求/响应，
纯无状态设计（抗丢包、抗断连、免重连）。

## 职责

让同一对局的 wp8f 用户互相看见：把本机状态按 `send_hz` 上报给数据链服务端，收下服务端回的
**全量友军快照**，交给 HUD 画到地图与标签面板上。本 crate 只管「协议 + 网络 + 追踪 + 线程」，
**不依赖 `wp8f-disp`**（依赖方向是 `disp` → `datalink`，协议类型由 HUD 侧反向使用），
与 HUD 的全部耦合就是两个进程级无锁槽（`link_ring.rs`）。

## 实现原理

- **对局隔离**：`session_id`（u8，客户端配置）分池，只有同一对局的人互相共享
- **唯一标识**：`client_id` = 客户端启用时刻 Unix 毫秒 —— 同一 NAT 多客户端、重连换 IP 都能正确区分
- **超时释放**：同 `client_id` 连续 N 秒（默认 5s）没来包自动释放
- **字段零转换**：包字段与 `DisplayData`/`MapDisplay` 同名同型（f64），收到即写入友军槽
- **XOR 混淆**：逐 4 字节异或（异或值 = 密钥字节求和），解密后魔数校验不过静默丢弃
- **独立线程、主循环零 I/O**：上报/收包/刷新友军快照全在 datalink 线程里，HUD 主循环只
  `publish_own()` 一次（无锁、无分配），绘制线程 `latest_tracks()` 取最新消费

## 代码目录

```text
datalink/
├── Cargo.toml              # crate 名 datalink；依赖 wp8f-ring；可选特性 bins（clap）
├── README.md
├── src/
│   ├── lib.rs              # 协议层：StatePacket / Reply / XOR 混淆 / 常量（含单元测试）
│   ├── track.rs            # TrackPool：client_id → 追踪对象（覆盖更新、超时、快照）
│   ├── server.rs           # Server / ServerConfig：UDP 请求-应答、限频、按对局分池
│   ├── client.rs           # DataLinkClient：发送自身状态 / 接收最新全量应答
│   ├── link_ring.rs        # 两个进程级槽：本机状态（core→线程）、友军快照（线程→HUD）
│   ├── thread.rs           # 数据链线程：定期上报 + 每拍排空应答 + 发布友军快照
│   └── bin/
│       ├── datalink_server.rs       # 二进制 datalink-server（--features bins）
│       └── datalink_test_client.rs  # 二进制 datalink-test-client（--features bins）
└── tests/
    └── integration.rs      # 集成测试：随机端口 Server 线程 + 真实客户端本地回环（含线程端到端）
```

- `datalink-server`：可独立部署的服务端（无其他 crate 依赖）
- `datalink-test-client`：合成数据收发的联调客户端
- 测试：`cargo test --offline -p datalink`（单元测试在各模块 `#[cfg(test)]`，
  回环集成测试在 `tests/integration.rs`）

## 包格式

### StatePacket = 116 字节（小端、线上 4 字节对齐）

| 偏移 | 类型 | 字段 | 说明 |
|------|------|------|------|
| 0  | u64 | `client_id`     | 客户端唯一 ID（启用时刻 Unix ms） |
| 8  | u32 | `magic`         | `0x4C38_4B38`（"L8K8"），解密后校验 |
| 12 | u8  | `version`       | 协议版本（当前 1） |
| 13 | u8  | `session_id`    | 对局编号（同对局才共享） |
| 14 | u8  | `flags`         | bit0 = `FLAG_SENDER`（服务端回填的"发件人本人"标记） |
| 15 | u8  | `seq`           | 客户端包计数（u8，255→0 回绕，丢包观测） |
| 16 | [u8; 16] | `aircraft_type` | 机型，定长零填充（不足补 0、超长按字节截断） |
| 32 | f64 | `ias`           | 表速 km/h |
| 40 | f64 | `tas`           | 真速 km/h |
| 48 | f64 | `altitude`      | 高度 m |
| 56 | f64 | `mach`          | 马赫数 |
| 64 | f64 | `heading`       | 水平航向 deg |
| 72 | f64 | `vy`            | 垂直速度 m/s |
| 80 | f64 | `x`             | 归一化地图坐标 |
| 88 | f64 | `y`             | 归一化地图坐标 |
| 96 | f64 | `poi_x`         | 本机 POI 归一化地图坐标 |
| 104 | f64 | `poi_y`        | 本机 POI 归一化地图坐标 |
| 112 | u8 | `poi_enabled`   | 是否启用 POI 同步（0/1，0 时忽略 poi_x/poi_y） |
| 113–115 | — | 保留（3 字节） | 恒为 0 |

### Reply = 8 字节头 + N × StatePacket（N ≤ 12）

| 偏移 | 类型 | 字段 | 说明 |
|------|------|------|------|
| 0 | u32 | `magic`      | 同上，解密后校验 |
| 4 | u8  | `version`    | 同上 |
| 5 | u8  | `session_id` | 对局编号（客户端只接受与自己相同的应答） |
| 6 | u8  | `count`      | 条目数（≤ `MAX_TRACKS` = 12，超界截断） |
| 7 | u8  | `server_seq` | 服务端应答计数（u8 回绕） |

`8 + 116 × 12 = 1400` 字节，留在 UDP 安全负载内；逐条解析、坏条目跳过不影响其余。

### XOR 混淆

- `xor_key = sum(密钥字节) as u32`（u32 环绕）
- 包内每个 4 字节字 `^= xor_key`（加/解密同一函数），不足 4 字节的尾巴用密钥低位字节
- 解密后魔数/版本不符的包静默丢弃（服务端计入 `rx_bad`）
- 注意：这是**防串台的混淆**，不是密码学加密（已知明文可破）；**密钥为空时不做混淆（等于明文传输）**

## 服务端（独立运行）

```bash
# 构建（静态链接，任何 x86_64 Linux 可跑）
cargo build --release --offline -p datalink --features bins --target x86_64-unknown-linux-musl --bin datalink-server

# 运行
./datalink-server --key <密钥> --port 2887 --hz 4 --timeout 5
```

| 参数 | 默认 | 说明 |
|------|------|------|
| `--port` | 2887 | 监听端口（UDP） |
| `--key` | 空 | 加密密钥（异或值 = 密钥字节求和，须与客户端一致） |
| `--hz` | 4 | 数据周期（单客户端应答限频，`<=0` 不限） |
| `--timeout` | 5 | 追踪对象超时释放（秒） |
| `--max-tracks` | 12 | 单应答最大追踪对象数（≤ `MAX_TRACKS` = 12，留在 UDP 安全负载内） |

远程部署示例（当前测试服务器）：

```bash
scp target/x86_64-unknown-linux-musl/release/datalink-server root@206.237.20.130:/root/
ssh root@206.237.20.130 'nohup /root/datalink-server --key netkey --port 2887 --timeout 5 --hz 4 > /root/datalink_server.log 2>&1 &'
```

服务端每 10s 打印一次统计（`tracks/session 数、收包、坏包、应答、限频、新建、过期`）。

## 测试客户端

```bash
# 构建（二进制需 --features bins）
cargo build --offline -p datalink --features bins

# 基本用法：发 10 个包后退出（--count 0 = 持续运行）
./target/debug/datalink-test-client --server 127.0.0.1:2887 --key netkey --session 1 --ac-type f_16c --count 10

# 启用 POI 同步：开启 POI 并指定归一化坐标（--poi-x / --poi-y）
./target/debug/datalink-test-client --server 127.0.0.1:2887 --key netkey --session 1 --poi --poi-x 0.6 --poi-y 0.4 --count 10

# 固定序列回放：8 个航路点绕图心循环（表速/高度/马赫固定），便于游戏内核对友军显示
./target/debug/datalink-test-client --server 127.0.0.1:2887 --key netkey --session 1 --fixed --count 20
```

| 参数 | 默认 | 说明 |
|------|------|------|
| `--server` | 127.0.0.1:2887 | 服务端地址（ip:port） |
| `--key` | 空 | 加密密钥（须与服务端一致） |
| `--session` | 1 | 对局编号（同对局才互相共享） |
| `--hz` | 4 | 发送频率 Hz |
| `--ac-type` | test_plane | 机型（写入 `aircraft_type`，超 16 字节截断） |
| `--x` / `--y` | 0.5 / 0.5 | 初始归一化地图坐标 |
| `--count` | 0 | 发送多少个包后退出（0 = 一直运行） |
| `--fixed` | 关 | 固定序列回放模式 |
| `--poi` | 关 | 启用 POI 同步（配合 `--poi-x`/`--poi-y`） |
| `--poi-x` / `--poi-y` | 0.6 / 0.4 | POI 归一化地图坐标 |

每帧按 `--hz` 发一帧合成状态并打印收到的全量追踪对象（`*` = 自己，
服务端回填 `FLAG_SENDER`）。

## wp8f 客户端配置（config 的 `datalink` 段）

`config/default.json`：

```json
"datalink": {
  "enabled": true,
  "server": "206.237.20.130",
  "port": 2887,
  "key": "netkey",
  "session_id": 1,
  "send_hz": 4,
  "peer_timeout_secs": 5
},
"datalink_x": 48,
"datalink_y": 300
```

| 键 | 默认 | 说明 |
|------|------|------|
| `enabled` | false | 是否启用 Link-8 上报/接收 |
| `server` | 127.0.0.1 | 服务端 IP/主机名 |
| `port` | 2887 | 服务端端口 |
| `key` | 空 | 加密密钥（须与服务端一致） |
| `session_id` | 1 | 对局编号（同对局才互相共享） |
| `send_hz` | 4 | 上报频率（Hz，整数，钳到 1..30） |
| `peer_timeout_secs` | 5 | 友军快照超时（秒）：这么久没收到服务端应答就清空友军；`0` = 关闭清空（一直保留最后一份） |
| `datalink_x` / `datalink_y` | 48 / 300 | 友军数据标签 Panel 位置 |

> **旧配置键兼容**：迁移前的配置段名是 `"link8"`（以及 `link8_x`/`link8_y`），
> 已通过 serde `alias` 保持兼容——旧配置**无需修改**即可照常读取，
> 新配置请使用 `"datalink"`（及 `datalink_x`/`datalink_y`）。

启用后：

- **datalink 线程**（`thread::spawn_link()`，core 只调用一次）按 `send_hz`（**整数** Hz，钳到 1..30）
  从本机状态槽取**最新**一版上报：机型、表速、真速、高度、马赫数、航向、垂直速度、归一化地图坐标、
  **POI 坐标与启用标志**；收包另有 50 ms 节拍（不必等服务端一个上报周期），只保留最新一份全量应答。
  主循环里**没有任何 socket 调用**，只每帧 `link_ring::publish_own(...)` 一次。
- 线程每 5 s 打一行 `[DATALINK] send_hz=4 sent=N replies=M peers=K skipped=S reset=R expired=E`
  （只在该窗口有活动时打），退出时再打一行 `[DATALINK] stopped: …`；
  上报被跳过的本机状态版本数在 `skipped` 里（主循环帧率高于 `send_hz` 的正常现象）。
- **超时清空**：超过 `peer_timeout_secs`（默认 5 s，与服务端追踪超时同口径）没收到**任何**应答，
  线程就发布一份空快照（地图标记 + 数据标签面板一起消失），并打一行
  `[DATALINK] peers expired: no reply for 5s, cleared N peers`；同一轮超时只清一次，
  下一份应答到达即恢复。配 `0` 可关闭这个行为（一直保留最后一份快照）。
- **地图 Panel** 显示同对局友军：小号绿色三角箭头（**旋转方向 = 航向**，0°=北，顺时针）+ 机型标签（仅前 4 字符）；
  **友军 POI 显示为绿色十字**（`poi_enabled` 时），与飞机标记解耦（友机在面板外也可显示其 POI）
- **友军数据标签 Panel**（`datalink_x`/`datalink_y` 配置位置，**drag 模式可拖曳**）：
  `机型 | 表速 | 高度 | 航向 | 距离 | 区域`（区域由坐标经本地地图网格推导，如 D3），
  全面板统一字号（与飞行面板标签同号），最多 8 行，跳过自己（服务端回填 FLAG_SENDER）
- **重连换局**：core 在每次连上 8111 后调 `link_ring::reset_tracks()`，友军列表立刻清空
  （generation 前进，HUD 不会把上一局的友军留在屏幕上），线程继续填新一局的数据

## 结构图

三个角色，互不等待：

```text
core 主循环 ──每帧 publish_own(OwnState)──▶ [本机状态槽] ──send_hz 取最新──▶ datalink 线程
                                                                              │ UDP send/recv
HUD 绘制线程 ──每帧 latest_tracks()──◀ [友军快照槽] ◀──每拍发布最新全量快照────┘
```

两个槽都在 `link_ring.rs`，底层是 `wp8f-ring` 的**无锁环形缓冲区**（与 HUD/logger 的帧缓冲区同一个原语）：
无锁、无 `Arc`、无逐帧分配，生产端只写下一个槽、消费端直读已发布的槽。
载荷必须是 POD，所以本机状态用定长机型字段、友军快照用定长 `Snap`（截到 `MAX_TRACKS` 条）。
数据链应答本身就是全量快照，历史版本没有意义，所以消费端只取最新一份：
线程用 `poll_own()` 取最新并拿到 `skipped`/`reset` 统计，HUD 用 `latest_tracks()` 直接取
（永不阻塞，没数据就是空切片）。

## 关键函数 Specification

### 协议层（`lib.rs`，无 I/O、无状态）
| 接口 | 输入 → 输出 | 说明 |
|---|---|---|
| `StatePacket::new(client_id: u64) -> Self` | 客户端 id → 空包 | 其余字段为 0，机型为空串 |
| `StatePacket::{set_type(&str), type_str() -> &str}` | 机型名（≤ `TYPE_LEN = 16` 字节，超出截断）↔ 定长字段 | 线上定长，避免变长解析 |
| `StatePacket::validate(&self) -> bool` | — → 是否可发 | 检查魔数/版本/`client_id != 0` |
| `StatePacket::{to_bytes() -> [u8; 116], from_bytes(&[u8]) -> Option<Self>}` | 结构体 ↔ 小端字节 | 解码失败（长度不符/魔数版本不对）返回 `None`，调用方静默丢弃 |
| `Reply::{to_bytes() -> Vec<u8>, from_bytes(&[u8]) -> Option<Self>}` | 8 字节头 + N × `StatePacket`（N ≤ `MAX_TRACKS = 12`） | 应答即全量快照 |
| `xor_key(key: &str) -> u32` / `xor_crypt(&mut [u8], key: u32)` | 密钥字符串 → 异或值（各字节求和） | 逐 **4 字节**异或；加解密同一函数 |
| `write_type(&mut [u8; 16], &str)` / `generate_client_id() -> u64` | — | 机型写入定长字段；`client_id` = 启用时刻 Unix 毫秒 |

错误：以上全部**不返回 `Result`**（`from_bytes` 用 `Option`）—— UDP 上坏包没有重试价值，
丢弃即可。副作用：无（纯函数）。

### 追踪（`track.rs`）
`TrackPool::{new(timeout: Duration), upsert(&mut self, StatePacket, now: Instant) -> bool,
prune(&mut self, now: Instant) -> usize, snapshot(&self, requester: u64, max: usize) -> Vec<StatePacket>,
len(), is_empty()}`
* 输入：`client_id` 为键的追踪对象；`now` 由调用方注入（便于测试，不读系统时钟）。
* 输出：`upsert` 返回是否**新建**（而非覆盖）；`prune` 返回释放条数；`snapshot` 返回除
  `requester` 外的最新 N 条（服务端回填 `FLAG_SENDER`，客户端据此跳过自己）。
* 前置：`timeout` 与服务端 `PEER_TIMEOUT_SECS`（5 s）同口径。
* 后置：`upsert` 覆盖同 `client_id` 的旧条目并刷新时间戳；`prune` 只删超时项。
* 错误：无。副作用：改自身表；`snapshot` 分配一个 `Vec`。

### 进程级槽（`link_ring.rs`，底层 `wp8f-ring`）
* `init_slots()`：建两个进程级环（幂等，core 启动时调一次）。
* `publish_own(OwnState)` / `poll_own(&mut Cursor) -> Poll<'static, OwnState>`：
  本机状态槽（core 每帧写 → datalink 线程按 `send_hz` 取最新）。`poll_own` 顺带给出
  `skipped`（主循环帧率高于上报频率的正常现象）与 `reset`。
* `publish_tracks(&[StatePacket])` / `latest_tracks() -> &'static [StatePacket]` /
  `tracks_generation() -> u64`：友军快照槽（线程每拍写 → HUD 绘制线程直读，**永不阻塞**，
  无数据即空切片）。
* `reset_tracks()`：换局时清空（generation 前进，HUD 据此不把上一局友军留在屏幕上）。
* `is_active()`：datalink 线程是否已起来（HUD 用来决定要不要画友军图层）。
* 错误：无。副作用：写槽（`publish_own`/`publish_tracks` 无分配，载荷是 POD）。

### 线程（`thread.rs`）
`spawn_link(cfg: LinkConfig) -> Option<LinkHandle>`
* 输入：`LinkConfig { enabled, server, port, key, session_id, send_hz(钳 1..30),
  peer_timeout_secs }`（core 从配置的 `datalink` 段搬来）。
* 输出：`Some(LinkHandle)`；`enabled == false` 时 `None`（本次飞行不启用数据链）。
* 前置：`link_ring::init_slots()` 已调用。
* 后置：线程按 `send_hz` 上报本机最新状态，另按 50 ms 节拍排空应答；每 5 s（有活动时）
  打一行 `[DATALINK] send_hz=… sent=… replies=… peers=… skipped=… reset=… expired=…`。
* 错误：UDP socket 创建/绑定失败 → 记入 `send_errors` 并打日志，**不让飞行失败**。
* 副作用：起线程、占一个 UDP socket、写 `logs/`（经统一日志）。

`LinkHandle::{stats() -> LinkStats, send_hz() -> u32, stop(self)}`
* `stats` 读共享原子计数（`sent` / `replies` / `send_errors` / `skipped` / `resets` /
  `expired` / `peers`）；`stop` 置停止位并 `join`（`Drop` 同样会停，忘调不会漏线程）。

### 服务端（`server.rs`，二进制 `datalink-server`）
`Server::{bind(cfg: ServerConfig) -> io::Result<Self>, stats() -> ServerStats,
local_addr() -> io::Result<SocketAddr>, track_count() -> usize, session_count() -> usize}`
* `ServerConfig { port(默认 2887), key, hz(默认 4.0), timeout(5 s), max_tracks(12) }`。
* `ServerStats { rx_ok, rx_bad, tx_replies, tx_throttled, spawned, expired }`。
* 前置：端口可用；`key` 与客户端一致（不一致 → 解密后魔数校验不过，计入 `rx_bad`）。
* 错误：绑定失败返回 `io::Error`（二进制里报错退出）。
* 副作用：占 UDP 端口；按 `0.75/hz` 限频应答（避免同频发包时周期性丢应答）。

### 客户端（`client.rs`，二进制 `datalink-test-client`）
`DataLinkClient::{new(server, key, session_id) -> io::Result<Self>,
with_id(server, key, session_id, client_id), client_id(), session_id(),
send_state(StatePacket) -> io::Result<usize>, recv_latest() -> Option<Vec<StatePacket>>}`
* `recv_latest` 排空 socket 里所有应答，**只保留最新一份全量快照**（历史无意义）。
* 错误：`io::Result`；`recv_latest` 用 `Option`（超时/无包 → `None`）。
* 副作用：网络 I/O。

## 约束与取舍

- **无握手/无会话**：每包自包含（魔数+版本+数据），丢包只是下个周期再发，客户端离场只是超时消失
- **NAT 友好**：客户端主动发包打通回程，服务端只应答来源地址，无需推送/端口映射
- **应答即全量**：客户端取最新一份应答即可刷新全部友军，无需增量合并
- **限频余量**：服务端按 `0.75/hz` 限频，避免客户端同频发包时被周期性丢应答
- **主循环零 I/O**（2026-10 解耦，参考 logger）：socket 收发、上报限频计时、应答排空、
  友军快照刷新全部搬进 `thread::spawn_link()` 起的线程；core 主循环只剩
  `link_ring::publish_own()`（无锁、无分配），HUD 侧只剩
  `link_ring::latest_tracks()`（只取最新、不等待）。线程之间不共享锁：三个角色各拿各的槽。
- **依赖方向不闭环**：`datalink` 不依赖 `wp8f-disp`（disp 反过来依赖 datalink 取协议类型），
  所以本机状态用 `link_ring::OwnState` 快照传递，而不是让线程去读 `DisplayData`。
  两边需要的无锁环形缓冲区因此抽成了零依赖小 crate `wp8f-ring`（`ring/`），
  `disp` 的帧缓冲区与这里的两个槽是**同一份实现**。
- **旧配置键仍兼容**：`link8` / `link8_x` / `link8_y` 由 serde `alias` 兜住，老配置无需改动；
  新配置一律写 `datalink` / `datalink_x` / `datalink_y`。协议线上格式自迁移以来未变。

## 测试

单元测试在各模块的 `#[cfg(test)]`（协议编解码、XOR、`TrackPool` 超时与快照），
回环集成测试在 `tests/integration.rs`（随机端口起 Server 线程 + 真实客户端走本地回环，
含线程端到端）。`scripts/test.sh` 会带上 `-p datalink` 一起跑。

```bash
cargo test --offline -p datalink          # 单元 + 集成（WSL 内执行，一律 --offline）

# 二进制（服务端/测试客户端需 --features bins，否则只编译库）
cargo build --offline -p datalink --features bins
cargo build --release --offline -p datalink --features bins

# musl 静态交叉编译（产出静态链接二进制，任何 x86_64 Linux 可跑）
cargo build --release --offline -p datalink --features bins \
    --target x86_64-unknown-linux-musl --bin datalink-server
```
