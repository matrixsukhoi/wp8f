# test-server —— 模拟 8111 游戏服务端（`test-server`）

## 职责

在本地伪造 War Thunder 的 8111 HTTP 端口，让 HUD/控制台在**没有游戏**的情况下也能跑通整条链路
（拖拽预览）：按 TOML 场景返回 `/state`、`/indicators`、`/map_info.json`、`/map_obj.json`、
`/map.img`。它是 HUD 的对手方，不是 HUD 的一部分 —— 主程序不依赖它，缺了只是拖拽预览起不来。

## 实现原理

* **单文件 TCP 服务**。标准库 `TcpListener` 绑 `127.0.0.1`，每个连接一个线程；只实现 HUD 需要的
  那一小块 HTTP：读请求首行（遇到 `\n` 或攒够 4096 字节就当请求读完）→ 按 URL 选响应 →
  写完关连接。没有 keep-alive、没有并发上限、没有路由框架。
* **手写 JSON，而不是 `serde_json`**。真实 8111 的报文有自己的脾气：键**按字母序排列**、
  冒号两侧留空格（`/state` 用 `"k": v`、`/map_info.json` 用 `"k" : v`）、数组写成
  `[ {…},{…} ]`。wp8f 的 `flat_json` 解析器就是照这份报文写的，所以这里必须原样复刻格式 ——
  `format_wt_object` / `format_wt_array` 负责排序与空白，`format_wt_value` 负责 TOML → JSON
  的值转换（NaN/±inf → `0.0`，整数值补 `.0`，字符串转义）。
* **场景即配置**。一份 TOML 描述一个飞行状态：`name`/`description` 是说明，`state`/`indicators`
  是 8111 的两张键值表，`map_info`（表）与 `map_objects`（表数组）是地图信息与地图对象。
  顶层键名与 8111 的字段名一一对应，改场景不用改代码。
* **底图是现生成的**：`/map.img` 每次请求都按同心圆渐变画一张 512×512 的 JPEG（`image` crate 的
  jpeg 编码器），只为让 HUD 的地图面板有张图可贴 —— 不做缓存，也不读磁盘。
* **端口按序试**：`--port` 列表从左到右试绑，**第一个成功的就是唯一在听的**（成功即进入
  accept 循环）；全失败才退出码 1。默认 `8111,9222` —— 8111 被真游戏占用时自动落到 9222
  （HUD 侧的 `--drag`/`--port` 要跟着指）。

## 结构图

```
  GUI「拖拽预览」                                   手工调试
   ┌──────────────────────────────┐          ┌───────────────────────────┐
   │ binary/test-server.exe       │          │ cargo run -p test-server  │
   │ cwd = test-server/           │          │ （cwd 决定场景相对路径）  │
   └───────────────┬──────────────┘          └─────────────┬─────────────┘
                   └──────────────┬────────────────────────┘
                                  ▼
                    ┌──────────────────────────────┐
                    │ main(): 解析 --scenario/--port│
                    │ load_scenario() → Arc<Scenario>│
                    │ 依次 bind 127.0.0.1:port      │
                    └───────────────┬──────────────┘
                                    ▼  每连接一线程
        ┌───────────────────────────────────────────────────────────────┐
        │ handle_client: 读首行 → parse_http_request(GET → url)         │
        │   └─ handle_request(url, scenario)                            │
        │        /state          → format_wt_object(state)     (JSON)   │
        │        /indicators     → format_wt_object(indicators)(JSON)   │
        │        /map_info.json  → format_wt_object(map_info)  (JSON)   │
        │        /map_obj.json   → format_wt_array(map_objects)(JSON)   │
        │        /map.img        → 512² JPEG（现生成）      (octet-stream)│
        │        /editor/fm_commands?cmd=setAlt|setVelocity → {"result":"ok"}│
        │        其他            → 不写响应，直接关连接                  │
        └───────────────────────────────────────────────────────────────┘
                                    ▲
                     HUD（wp8f.exe --drag）／flat_json 解析器
```

## 代码目录

| 路径 | 内容 |
|---|---|
| `src/main.rs` | 全部逻辑：TOML→JSON 格式化、路由、HTTP 响应拼装、底图生成、监听与主循环 |
| `src/scenario.rs` | `Scenario`（serde 反序列化 + `Default` = 空场景） |
| `scenarios/*.toml` | 场景：`default` / `cruise` / `high_g` / `low_alt`（按 **cwd** 解析，故留在本目录） |

产物与部署：`scripts/build.sh` 输出 `binary/test-server.exe`；控制台把 cwd 设成 `test-server/`
再拉起它，所以 `scenarios/` **不随二进制移动**（`scripts/zip.sh` 也按原位打包）。

## 关键函数 Specification

### `fn main()`
* 输入：CLI `-s/--scenario <路径>`（默认 `scenarios/default.toml`，相对路径按 **cwd** 解析）、
  `-p/--port <端口表>`（逗号分隔，默认 `8111,9222`）。日志级别由 `RUST_LOG` 控制（默认 INFO）。
* 输出：无（阻塞在 accept 循环里）。
* 前置：场景文件存在且是合法 TOML。
* 后置：只监听 `127.0.0.1`；启动时把场景名、描述与全部端点打进日志。
* 错误：读/解析场景失败 → `error!` + 退出码 1；所有端口都绑不上 → 逐个 `error!` + 退出码 1。
* 副作用：占一个 TCP 端口、起每连接一个线程（线程数无上限）。

### `fn load_scenario(path: &PathBuf) -> Result<Scenario, String>`
* 输入：TOML 路径（相对路径先拼 `current_dir()`）。
* 输出：`Scenario`；错误时返回人话字符串（含实际读的绝对路径）。
* 前置：无（文件缺失/语法错误都走 `Err`）。
* 后置：无 —— 场景在启动时**读一次**，运行期不再重载（改场景要重启进程）。
* 错误：`Err(String)` —— 取 cwd 失败 / 读文件失败 / TOML 解析失败。
* 副作用：读磁盘。

### `fn handle_request(url: &str, scenario: &Scenario) -> Option<HttpResponse>`
* 输入：请求 URL（含 query）；场景。
* 输出：`Some(Text|Binary)`；未知路径返回 `None`。
* 前置：URL 已从请求首行切出。
* 后置：无 —— 纯函数，不改场景（`setAlt`/`setVelocity` 只打日志并回
  `{"result":"ok"}`，**不会**真的改高度/速度）。
* 错误：无 `Result`；`/editor/fm_commands` 上的未知命令回 `{"error":"unknown command"}`。
* 副作用：`info!/warn!` 日志（把每个请求打出来，方便对着 HUD 排查）。
* 注意：`cmd=getFmProperties` 目前是空分支，会落到「unknown command」。

### `fn parse_http_request(request: &str) -> Option<String>`
* 输入：连接上收到的原始字节（lossy 转成的字符串）。
* 输出：`GET` 请求的 URL（首行第二个字段）；不是 GET 或首行不完整 → `None`。
* 前置：无。
* 后置：无 —— 只取首行，忽略请求头与 body。
* 错误：无（`None` 即「不响应」）。
* 副作用：无。

### `fn build_http_response(body: &str, ct: &str) -> String` / `build_binary_response(body: &[u8], ct: &str) -> Vec<u8>`
* 固定 `HTTP/1.1 200 OK` + `Server: test-server` + `Content-Length` +
  `Access-Control-Allow-Origin: *`（HUD 侧无跨域问题，但带上方便浏览器直接看端点）。
* 没有 404/500 分支 —— 「找不到」表现为 **不写响应直接关连接**。
* 错误：无；副作用：无。

### `fn generate_placeholder_map_img(width: u32, height: u32) -> Vec<u8>`
* 输出：同心圆渐变的 RGB JPEG 字节（当前固定 512×512）。
* 前置：`width/height > 0`（`ImageBuffer` 分配失败即 panic/OOM）。
* 后置：无缓存 —— 每次 `/map.img` 请求重新画、重新编码。
* 错误：编码失败被 `.ok()` 吞掉 → 返回半截或空数据（只在内存不足时可能发生）。
* 副作用：一次图像缓冲与一次 JPEG 编码的分配。

### `format_wt_value` / `format_wt_object` / `format_wt_array`（内部函数）
* 值转换：字符串（转义 `\` 与 `"`）、整数、浮点（NaN/±inf → `0.0`，无小数点则补 `.0`）、
  布尔、数组（`[ a, b ]` 带空格）、其他 → `null`。
* 对象：键**排序后**输出 `{"k":v,...}`；`separator` 参数决定冒号间距
  （`/state`、`/indicators` 用 `": "`，`/map_info.json` 用 `" : "`，与真实 8111 一致）。
* 错误：无；副作用：无。

## 场景 TOML 格式

```toml
name = "default"
description = "默认巡航场景"

[state]                 # → /state（键值表，键名照抄 8111 字段名）
"valid" = true
"IAS, km/h" = 560.0

[indicators]            # → /indicators
"rpm" = 0.85

[map_info]              # → /map_info.json（可省，默认空）
"map_generation" = 1

[[map_objects]]         # → /map_obj.json（表数组，可省，默认空）
"type" = "aircraft"
"x" = 0.5
```

`Scenario::default()` 是「空场景」：三张表皆空、`map_objects` 为空数组。

## 约束与取舍

* **本机专用**：只绑 `127.0.0.1`，无鉴权、无 TLS —— 它是调试工具，不要暴露到网络上。
* **HTTP 只做到够用**：无 keep-alive、无 chunked、无请求头解析；请求超过 4096 字节即当读完。
* **没有单元测试**：行为靠端到端预览验证（HUD 对着它跑）。`core/tests/map_image_verbatim.rs`
  复用了同款 JPEG 编码方式，间接覆盖「底图字节原样进记录」这条链路。
* **场景不热重载**：改 TOML 要重启 test-server。控制台在「预览」时会起它（若本控制台
  已经起过一个还在跑，预览会直接失败，先停掉再试）。
* 与 HUD 的端口约定：默认 8111；被真游戏占用时落 9222，HUD 侧要显式指过去。

## 测试

无自动化测试。手工验证：

```bash
cargo run --release -p test-server                 # 默认场景 + 8111/9222
curl http://127.0.0.1:8111/state                   # 看手写 JSON 的排序与间距
curl -o /tmp/map.jpg http://127.0.0.1:8111/map.img # 512×512 底图
cargo run --release -p wp8f-core -- --drag --port 8111   # HUD 连过来跑拖拽预览
```
