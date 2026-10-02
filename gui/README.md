# gui —— 控制台（`wp8f-gui`）

一站式桌面工具：**wp8f 配置器 / FM 解析 / Replay 回放**。
**单个 Rust 进程**（系统托盘 + 本地 HTTP API + wry/WebView2 窗口），前端是零构建链的
原生 HTML/CSS/JS + ECharts。运行时不需要 Python。

## 职责

给用户一个不用碰命令行的入口：改配置、看 FM 曲线、回放飞行记录、更新 FM 数据库；
并按需拉起 / 回收 HUD（`binary/wp8f.exe`）与模拟服务端（`binary/test-server.exe`）。
它由**窗口 + 子进程监管 + 本地 HTTP API** 三件事组成，本身不含任何飞行计算。

## 结构图

```
wp8f-gui.exe              控制台（单进程）：系统托盘 + 本地 HTTP API + WebView2 窗口
  ├─ binary\wp8f.exe        业务进程（控制台拉起，比窗口活得久）
  └─ binary\test-server.exe 拖拽预览用模拟服务端（与 wp8f 互相兜底回收）
```

## 实现原理

- **产物布局（S1）**：仓库根只放用户双击的入口 `wp8f-gui.exe`，HUD / FM / 模拟服务端
  都在 **`binary/`** 下（`gui/src/env.rs` 与 `gui/src/children.rs` 按 `binary/` 定位）；
  `test-server/scenarios` 与 `resource/**` 保持原位 —— 前者按 **cwd** 解析（控制台起
  test-server 时把 cwd 设成 `test-server/`），后者按仓库根定位。
- **只管子进程里的"自己的"**：控制台按启动时记下的 pid 管理 `wp8f.exe` / `test-server.exe`；
  用户自己开的 `wp8f.exe` 既不影响控制台启动，也不会被控制台结束。

- **关窗 = 销毁 WebView**（不是关进程）：实测同进程内 drop(WebView) 后 6 个
  `msedgewebview2` 子进程全部退出、内存回落到宿主基线，且能在同一窗口里重建 ——
  于是「点开始 → 释放资源、只留托盘」和"单进程"可以同时满足。
  代价：宿主侧 WebView2 运行库常驻，关窗后进程约 **25MB**（曾经的两进程方案是 11MB）。
- **为什么 wp8f 归控制台管**：窗口随时会被销毁，wp8f 不能跟着一起死；它是控制台的子进程，
  并且挂在 **kill-on-close 作业对象**上 —— 控制台无论怎么死（含 `taskkill /F`），
  wp8f / test-server 都会一起结束。
- **单实例靠端口独占**：控制台启动时绑固定端口（不是锁文件），第二个实例直接失败退出；
  探针要另起实例时用 `--port` + `--no-window`。
- **界面就是本地 HTTP 服务**：`gui/src/http.rs` 是一个手写的静态文件 + JSON 路由服务
  （`gui/static/` 原样发出，无打包步骤），`gui/src/api.rs` 是全部 `/api/*` 路由；
  另有一条**只绑 127.0.0.1 + 令牌**的控制通道（`gui/src/control.rs`），
  供外部脚本/探针做 `wp8f/launch|stop`、`window/show|close`、`exit`。
- **拖拽预览是双向兜底回收**：预览模式下控制台同时管 wp8f 与 test-server，
  常驻 reaper 每 3 s 轮询，**任一退出就把配套的另一个一起收**（否则会留下僵尸 test-server）；
  非预览启动不带 test-server。
- **一个控制台只允许一个自己启动的 wp8f（预览时再加一个 test-server）**：
  已经在跑就直接**启动失败**（500 + 人话原因），不再"先杀掉上一批再起新的" ——
  那会把预览里刚拖好的位置连同进程一起丢掉。要换配置/换模式先停止它
  （预览按 ESC / 托盘「显示窗口」会先结束自己启动的 wp8f）。
- **预览结束后重读配置**：HUD 在松手/ESC/关窗时把拖拽结果写回配置文件，所以控制台在预览
  启动后盯着自己那个 wp8f，**等它真的退出**才重新 `loadConfig`（早读会读到写回之前的旧内容）；
  窗口重新获得焦点时也会比对磁盘内容、被外部改过就重载（本地有未保存改动时不抢）。
- **配置读写走 API**：保存前校验 JSON（`POST /api/config/save`），失败返回错误而不是写坏文件。
- **回放只认 `.wpr`**：`gui/src/record.rs` 只解容器头 + meta（列表页要快），
  底图按需从记录里流式取出；旧 `.acmi`/`.csv` 的读路径已删除。
- **FM 页签调二进制**：`/api/fm/*` 起 `binary/flightmodel.exe` 取数据（`gui/src/env.rs`），
  数据根用 `wp8f_flightmodel::resolve_data_root_in(仓库根)` 解析，与 HUD 同一份实现。
- **图标只有一个来源**：`scripts/make_icon.py` 从同一组几何常量生成
  `gui/static/logo.svg`（矢量）+ `gui/logo.ico`（16…256 七档）；
  `build.rs` 用 windres 把 `icon.rc`（ID 1）编进 exe —— 资源管理器/任务栏/窗口标题栏
  （tao 注册窗口类时 hIcon 为空 ⇒ 系统用 exe 的应用程序图标）与托盘（`tray.rs` 优先
  `LoadIconW(GetModuleHandleW(null), 1)`）共用同一份，不再往 `%TEMP%` 落临时 ico。

## 构建

```bash
cargo build --release -p wp8f-gui --target x86_64-pc-windows-gnu
cp target/x86_64-pc-windows-gnu/release/wp8f-gui.exe .      # 控制台留在仓库根（唯一入口）

# 或者一键整套（推荐，见根 README）：控制台 → 仓库根，HUD / FM / 模拟服务端 → binary/
bash scripts/build.sh    # 加 --webui-only 只构建控制台
```

## 启动

```powershell
.\wp8f-gui.exe               # 双击即可（纯 Rust，不需要 Python）
.\wp8f-gui.exe --no-window   # 只常驻托盘，需要时点托盘图标开窗
.\wp8f-gui.exe --browser     # 不开窗口，用系统浏览器打开页面（调试）
```

| 参数 | 说明 |
|------|------|
| `wp8f-gui.exe --no-window` | 只常驻托盘 + HTTP 服务（不开窗口；DOM 自检 / CI 用） |
| `wp8f-gui.exe --browser` | 不开窗口，用系统浏览器打开页面 |
| `wp8f-gui.exe --no-tray-promote` | 不把托盘图标提升为任务栏直接显示（见下） |
| `wp8f-gui.exe --port <N>` | HTTP 端口（默认 8765）。**端口独占 = 单实例判据**，被占用不再顺延（见下） |

## 生命周期与系统托盘

```
启动 ──► 绑 127.0.0.1:<port>（绑上 = 唯一实例；绑不上 = 已有实例/端口被占）──►
         建作业对象 + HTTP 服务 + 托盘 ──► 创建 WebView 打开窗口
点「开 始」 ──► 启动 wp8f（记 pid，单实例）──► 销毁 WebView（6 个 WebView2 退出）+ 隐藏窗口
点窗口 X  ──► 同上（释放 WebView2），进程与 wp8f 继续跑
托盘左键单击 / 「显示主窗口」──► 先 kill 自己启动的 wp8f，再重建 WebView 并显示窗口
托盘「退出」/ Ctrl+C / 进程被结束 ──► 结束全部子进程（作业对象兜底）
```

- **GUI 只回收自己启动的 wp8f**；托盘「显示主窗口」同样只结束本进程启动的实例，
  不会碰用户自己开的 HUD：机器上已有别人的 `wp8f.exe` 时，控制台照常拉起自己的实例。
- **内存**（实测）：开窗 ~25MB（含 WebView2 宿主）；关窗后 WebView2 的 6 个子进程退出，
  进程回落到同一水平（宿主运行库常驻）—— 想要更低的常驻内存就把窗口拆回独立进程
  （历史方案，git 记录里可查）。
- **点「开始」/「预 览」**：按当前配置启动新的 —— 但如果**本控制台已经有一个自己的 wp8f 在跑**，
  直接返回失败（500 + 原因），不替你杀进程（预览里刚拖好的位置不该被悄悄丢掉）。
  想换配置：先停掉它（预览按 ESC / 托盘「显示窗口」会先结束自己启动的 wp8f）。
  别人的 `wp8f.exe` / `test-server.exe` 一律不动（也不会阻止我们启动自己的）。
- **底部的「命令行调试」勾选框**：勾上后「开始 / 预览」用 `CREATE_NEW_CONSOLE` 启动 wp8f ——
  它有自己的命令行窗口（输出打在窗口里、不写 `logs/wp8f.log`）。其余行为与普通启动一致：
  「开始」照常关窗只留托盘（要看输出就看 wp8f 自己的那个窗口）、「预览」照常不关窗。
  勾选状态存 localStorage（下次打开还是这个状态）。
- **wp8f ↔ test-server 互相兜底回收**（常驻进程里的 reaper，3s 轮询）：拖拽预览时两个 pid
  都有记录，**任一退出就回收另一个**（只回收自己记录的那两个，别人的进程不参与判断）。
- **常驻进程结束 → 子进程全退**：Win32 kill-on-close 作业对象，`taskkill /F` 也绕不过
  （实测 4s 内窗口/wp8f/test-server 全退）。窗口就在本进程里，不存在"另一个进程失联"的问题。
- **托盘不可用就不关窗**：托盘注册失败时 `/api/system` 返回 `tray_available=false`，
  关窗请求被拒（409）并右下角告警 —— 免得窗口关了又没图标唤回。
- **重复启动（控制台自己的单实例）＝端口占用判定**。2026-10 起不再用 `Global\wp8f_gui` 命名互斥体：
  那是 Windows 专有机制，而"端口独占"本身就是互斥、且跨平台（Linux 上一样成立）。启动第一步绑
  `127.0.0.1:<port>`：
  - **绑上** → 本进程就是唯一实例，照常启动（写运行态、起托盘、开窗口）；
  - **绑不上，且 `logs/console-host.json` 里记的端口正是它** → 已有控制台：`POST /control/window/show`
    把它的窗口叫出来，stdout 打一行**英文**`wp8f console already running: …`（固定串，探针按它 +
    退出码判定）后自己退出（**退出码 0**；双击第二个就是这么用的）；
  - **绑不上，但运行态读不到 / 端口对不上** → 占住端口的是**别的程序**：stdout 打
    `port <N> is occupied by another program …`（**退出码 2**），消息框提示换 `--port`
    或结束占用该端口的进程；
  - **绑不上，运行态在、指向本端口，却连不上/不应答**（读写超时 3s）→ **已有控制台但无法唤醒**：
    stdout 打 `… does not respond: could not be woken up`（**退出码 3**），消息框提示结束它再启动，
    或换 `--port`；
  - `--no-window`（自检/CI）下失败只打 stdout、不弹模态框 —— 无人值守时模态框没人点，会把脚本挂住；
  - 其它绑定错误（权限等）明确报错退出（**退出码 1**），不再静默顺延到下一个端口；
  - **stdout 的这几条诊断一律英文、不随界面语言变**（`gui/src/main.rs` 的 `port_busy_exit`）：
    `scripts/tests/*.py` 靠它们 + 退出码区分"端口被谁占着"，本地化会让判据随语言漂移；
    给人看的**消息框**才走 i18n（`gui.dlg.*` 键）；
  - 判据是**按端口**的：显式换 `--port` 就能并行跑第二个控制台。注意运行态文件
    `logs/console-host.json` 只有一份、由最后启动的那个覆写，所以**同一 root 下并行两个实例时，
    只有一个能被双击唤出**（探针因此仍旧要求先退出用户那个控制台 —— 见「自检」一节）。
- **Win11 图标可见性**：Win11 默认把新托盘图标折叠进 `^` 溢出区，常驻进程会把本 exe 的
  通知区域条目置为"直接显示"（`HKCU\Control Panel\NotifyIconSettings` 的 `IsPromoted=1`），
  `--no-tray-promote` 可关闭；随时可在「任务栏设置 → 其他系统托盘图标」改回。
- 日志：`logs/console-host.log`（子进程输出在 `logs/wp8f.log` / `logs/test-server.log`）；
  运行态信息（pid/控制通道/令牌）`logs/console-host.json`（启动时覆写，仅本机工具用）。

## 代码目录

```
gui/                    # 控制台（单进程 wp8f-gui.exe）
├── src/main.rs         #   入口：托盘 + HTTP 线程 + 窗口/WebView 生命周期（事件循环）
├── src/api.rs          #   API 端点（与旧 FastAPI 版逐字段对齐）
├── src/http.rs         #   std::net 手写 HTTP + 静态资源
├── src/children.rs     #   wp8f/test-server 监管（自己启动的 pid + 作业对象 + 双向 reaper）
├── src/control.rs      #   本地控制通道（工具/自检用）
├── src/tray.rs         #   Shell_NotifyIcon 托盘（消息由 tao 事件循环分发）
├── src/win.rs          #   Win32 底层封装（进程/作业对象/子进程）
├── src/env.rs          #   路径探测（数据根解析复用 wp8f_flightmodel::fm_paths，显式传 --data-dir）+ flightmodel 调用
├── static/             #   前端（零构建：多个 classic script + vendor/），由 Rust 直接读盘
│   ├── index.html      #     页面结构 + **按顺序**加载的 <script>（顺序即依赖，见下）
│   ├── app.js          #     主逻辑 + 共享工具：启动、页签路由、状态提示、图表容器/主题、共享常量
│   ├── hud-colors.js   #     HUD 颜色口径 #RRGGBBAA 的解析/格式化（纯字符串，不碰 DOM）
│   ├── config-form.js  #     配置器：配置列表 / 表单生成 / 联动夹紧 / 自动保存 / 启动 wp8f
│   ├── fm-charts.js    #     飞行模型页签：机型选择、燃油与重量、6 张曲线图 + EM 图、机型信息与对比
│   ├── replay.js       #     回放页签：录制列表、3D 轨迹、亮黄飞机标记（位置点 + 姿态线框）、xoy 底面地图（原图纹理）、遥测、转换导出
│   ├── missile.js      #     导弹模拟页签：离线 iframe 的加载/卸载/缺子模块提示
│   ├── logo.svg        #     favicon（由 scripts/make_icon.py 生成，与 exe 图标同源）
│   └── vendor/         #     echarts / echarts-gl / wt-missile-simulator（子模块）
├── icon.rc             # 图标资源清单（ID 1 = exe 应用图标，见 src/tray.rs）
├── build.rs            # 构建脚本：windres 把 icon.rc 编进 exe（没有 windres 只警告）
├── logo.ico            # 多尺寸图标本体（16…256，scripts/make_icon.py 生成）
```

> **前端没有打包器**：`index.html` 里按顺序放多个 `<script>`（classic script，不是 ES module），
> 它们共享同一个全局作用域 —— 拆分只是为了降低单文件复杂度，不是模块化改造。
> 因此**加载顺序有约束**：`app.js` 必须第一个（`$` / `api` / `esc` / `report` / `notify` /
> 图表工具都在那里）；启动动作（拉配置列表、恢复上次页签）推迟到 `DOMContentLoaded`，
> 那时 6 个脚本都已执行。静态资源不带 `?v=`：`/static/*` 与首页都回 `no-store, must-revalidate`，
> 拆分后不会读到旧缓存副本。
> 为什么不用 ES module（`type="module"`）：module 有独立作用域 + 严格模式 + 必须显式
> `import`/`export`，等于把当前依赖全局作用域的几百处调用全改一遍；而 classic script 的
> 拆分是**逐行搬运**（`node --check` + 探针即可验证行为不变），收益与风险比更划算。

前端的自检探针在 `scripts/tests/`（见下面的「自检」一节）。

## API

| 接口 | 说明 |
|------|------|
| `GET /api/health` | 健康检查 |
| `GET /api/system` | 环境自检：`tray_available`/`tray_error`、`resident`/`resident_status`、`window_exists`/`window_visible`/`window_pid`、`wp8f_running`（只算本进程启动的 wp8f） |
| `GET /api/resident/status` | 常驻进程状态（含控制通道连通性） |
| `POST /api/gui/window` | `{action: show\|close}`：`show` = 先 kill **本进程启动的** wp8f 再重建窗口；`close` = 销毁 WebView 并隐藏窗口（wp8f 继续跑）；托盘不可用时 409 |
| `GET /api/config/list`、`GET /api/config/{name}`、`POST /api/config/save` | 配置读写（保存前校验 JSON） |
| `GET /api/fm/aircraft`、`GET /api/fm/{name}`、`GET /api/fm/{name}/curves` | 机型列表 / 详情 / 曲线（调用 **`binary/flightmodel.exe`**，透传 `fuel_pct`/`extra_weight`）。`/curves` 走 `--curves-json`：CL 极曲线、推力/功率-速度族、以及 **`em` 段（只喷气机有）** —— 控制台的 EM 能量机动图按 `flightmodel/scripts/plot_flight_model.py` 的 `plot_em` 画法绘制（Ps 色带 + 瞬时/持续包线 + 峰值标注 + 等过载线 + VNE 线，一档高度一张，用下拉切换）。数据根用 **`wp8f_flightmodel::resolve_data_root_in(仓库根)`** 解析（与 HUD 同一份实现）：`<仓库根>/resource/data` 优先，不存在时用更新器 A/B 分区的暂存根 `<仓库根>/resource/data_new`；以**绝对路径**显式传给 `--data-dir`（不依赖子进程 cwd 与 CLI 默认值）；目录不存在时返回 500 并在消息里指路 |
| `GET /api/fm/version` | FM 数据库版本（读数据根里的 `version`，纯文本一行，如 `2.58.0.35`；A/B 分区时是 `resource/data_new/version`）。`{ok, version, source}`；页签标题旁显示同一份值，缺文件时 `ok=false`（不编造版本号） |
| `POST /api/wp8f/launch`、`POST /api/wp8f/drag-preview` | 启动 wp8f（**已有一个自己的 wp8f 在跑则失败**，500 + 原因；body 里 `console:true` = 给它一个自己的控制台窗口）/ 拖拽预览（test-server + `--drag`，同样要求 test-server 未在跑） |
| `GET /api/replay/list`、`GET /api/replay/load?file=` | 飞行记录列表 / 解析回放数据（**只认 `.wpr`**；旧 `.acmi`/`.csv` 的读路径已删除，加载它们返回 400）。`map.has_image = false`（记录没带底图）→ 回放**不铺底面地图**（只留网格线）：JSON 里没有"借来的底图"这种字段，服务端也不扫 logs/ 找替代品 |
| `GET /api/replay/map?file=` | `.wpr` 内嵌的地图底图原始字节（Content-Type 取记录里的 `img_mime`）。**只解容器头 + meta**，不解析 CSV。记录没带底图（`img_len = 0`）→ 404（不伪造、不外借） |
| `POST /api/replay/upload?name=` | 上传本地飞行记录（**只接受 `.wpr`**，≤16MB，重名自动加序号，不覆盖；其它扩展名 400） |
| `GET /api/update/status` | FM 数据库更新器状态：`phase`（idle/checking/downloading/finalizing/ready/switching/done/failed）、本地/远端版本、进度（文件数 / 字节数 / 重试 / 失败数）、`staging_ready`（= 暂存目录已写好 `version`，可以切换）、`error{kind,message}`、`[UPDATE]` 日志、`dropped_top_level`、`hud_running` |
| `POST /api/update/check` | 检查更新（同步，拉上游 `version` 与本地数值分段比较，D10） |
| `POST /api/update/start` | 开始下载到 `resource/data_new`（后台线程；可选 body `{"tag":"2.59.0.43"}`）。逐文件、并发有上限、逐文件重试 + 多源回退（**github.com 原站 → api.github.com**，P9 起第三方 CDN 已删），校验 = 大小 + JSON 可解析；**全部校验通过才写 `data_new/version`** |
| `POST /api/update/cancel` | 取消（已下好的文件保留，下次接着下） |
| `POST /api/update/apply` | **用户第二次确认后**才切换：`data → data_old`（固定单槽，只留上一版）、`data_new → data`，失败自动回滚；HUD 正在运行时会因目录被占用而失败并提示先停止 |
| `POST /api/update/stop-hud` | 结束**本控制台启动的** wp8f（用户自己开的不动），供切换前一键腾出数据目录 |
| 控制通道（仅 127.0.0.1 + 令牌） | `GET /control/status`、`GET /control/ping`、`POST /control/wp8f/launch\|stop`、`POST /control/window/show\|close`、`POST /control/exit` |

> **更新器（P3）**：`/api/update/*` 一律返回 HTTP 200 + `{ok, error?, status}` ——
> 更新失败是**业务结果**（网络 / 校验 / 磁盘 / 权限分类在 `error.kind` 上），不是 HTTP 错误；
> 前端按 `error.kind` 选 i18n 文案（`update.err.*`），技术细节原样放在 `error.message`。
> 下载数据只落本地（`resource/data_new`、`resource/data_old` 都在 `.gitignore` 里），
> **不随包分发**；上游仓库（War-Thunder-Datamine）没有许可证，面板里有对应的合规说明。

> 回放页签的路由在 2026-10 从 `/api/acmi/*` 改名为 **`/api/replay/*`**（本页签是"回放"，
> 老名字是早就删掉的 ACMI 解析器留下的）。注意别和**导出目标格式** `format=acmi`
> （TacView 的 `text/acmi/tacview`，产物是 `.acmi` 文本）混为一谈 —— 那个格式名不变。
>
> 底面（xoy 底面地图）：**只有「原图纹理」一种画法**——把 `.wpr` 里的原图当贴图交给 GPU
> （4 个顶点、恒定开销，拉远拉近都不变慢、显示的都是原图像素；逐像素网格那几档已删除）。
> **记录没带底图 ⇒ 不铺底面地图**（也不借别的记录的图），只留 z=0 网格线。
>
> 回放的**飞机标记**（当前位置点 + 姿态线框）**只有亮黄 `#ffe600`**（P9 用户口径：深色描边已删，
> 可见性靠更粗的线框 + 更大的点；页签上有同色的图例行）。轨迹线 / 文件标签圆点 / 遥测左边框
> 仍按对象配色，多对象时靠它区分；轴上的数字按"好看步长"取整显示（范围与内部精度不变）。

> 换实现时用 `scripts/tests/api_parity.py`：先对旧实现 `capture` 黄金样本，再对新实现 `capture`，
> 然后 `compare` 逐字段比对（忽略 pid/时间等易变量）。`capture` 会先清空输出目录并返回
> 采集失败条数，`compare` 里"一侧样本缺失"算失败而不是跳过。

## 关键函数 Specification

### 子进程监管（`children.rs`）
`Children::{new(root: PathBuf) -> Option<Children>, launch_wp8f(config: &str, drag: bool,
console: bool) -> Result<(u32, Option<u32>), String>, verify_wp8f_alive(pid) -> Result<(), (u16, String)>,
stop_wp8f() -> Vec<u32>, kill_all() -> Vec<u32>, wp8f_running() -> bool,
all_wp8f_pids() -> Vec<u32>, start_reaper(self: Arc<Self>), status_json(webview_alive, pid) -> String}`
* 输入：`config` = 布局配置路径（`--layoutconfig`）；`drag` = 是否连带 test-server；
  `console` = 是否给 HUD 一个自己的控制台窗口（`CREATE_NEW_CONSOLE`，不重定向 stdio）。
* 输出：`(wp8f_pid, Option<test_server_pid>)`；`verify_wp8f_alive` 失败时给
  `(HTTP 码, 人话原因)`（启动失败一律 500 + 日志尾部）。
* 前置：`binary/{wp8f,test-server}.exe` 已构建（缺件时报错并提示先构建）；
  **本控制台还没有自己启动的 wp8f**（预览还要 test-server 也没在跑）。
* 后置：起一个 wp8f（预览时另起/复用一个可用的 test-server）；新进程挂进作业对象、
  记下 pid 供"只回收自己启动的"使用。
* 错误：`Err(String)` —— 缺件 / 已有自己的实例在跑 / 启动失败 / 端口被占；
  **别人的 wp8f 一律不动**。
* 副作用：起进程、写 `logs/wp8f.log` / `logs/test-server.log`、改内部 pid 记录。

### HTTP 层（`http.rs`）
`Request::read(&mut TcpStream) -> io::Result<Request>` + `query_get`/`query_f64`/`json`；
`respond_json` / `respond_bytes` / `respond_error` / `content_type(path)` /
`static_root(root)` / `safe_join(base, rel) -> Option<PathBuf>` / `serve(listener, handler)` /
`bind_port(port) -> io::Result<TcpListener>`；`MAX_BODY = 16 MiB`。
* `safe_join` 负责**路径穿越防护**（`..` 与绝对路径一律拒绝）；
  `percent_decode_path` 处理 URL 解码（安全回归用例覆盖）。
* 错误：解析/读取失败返回 `io::Error`；体积超 `MAX_BODY` 直接拒绝。
* 副作用：网络 I/O。

### 路由（`api.rs`）
`App`（进程级状态）+ `handle(app: &Arc<App>, stream, req)`：全部 `/api/*` 路由的入口
（清单见下面「API」一节）。约定：
* 业务失败（更新器 / 启动失败）→ **HTTP 200 + `{ok:false, error?}`**；
  参数或资源不存在 → 4xx；意外内部错误 → 500。
* 副作用：读写 `config/*.json`、起子进程、写 `logs/console-host.log`。

### 控制通道（`control.rs`）
`Control::bind() -> io::Result<Control>` / `url()` / `serve(app, ui) -> io::Result<(String, String)>`
* 只绑 `127.0.0.1`，每次启动生成随机令牌（写进 `logs/console-host.json` 供探针读取）。
* 端点：`/control/{status,ping,exit}`、`/control/wp8f/{launch,stop}`、`/control/window/{show,close}`。
* 错误：端口占用 → `io::Error`（控制通道失败不影响主界面）。

### 窗口与托盘（`main.rs` / `tray.rs` / `win.rs`）
* `main.rs`：`Ui`（窗口与 WebView 的生命周期）、`UserEvent`（跨线程事件）、
  `log(msg)`（统一写 `logs/console-host.log`）、`promote_tray_icon()`。
* `tray.rs`：`install(actions: Actions, tooltip: &str, promote: bool) -> bool` /
  `uninstall()`；`Actions` 是托盘菜单回调集合。托盘不可用时返回 `false`，api 层据此回 409。
* `win.rs`：Win32 封装 —— `pids_of(exe_name)` / `is_running` / `pid_alive` / `kill_pid` /
  `Job::create`+`assign`（作业对象）/ `spawn(cmd, cwd, log_path, new_console)`。

### 记录与回放（`record.rs`）
`parse_wpr_file(path) -> Result<Value, String>` / `read_map_image(path) -> Result<(String, Vec<u8>), String>` /
`convert_wpr_file(...)`；`FRAME_COLUMNS`（前端画图用的列名）。
* 前置：文件是 `.wpr` 版本 2（其他一律 `Err`）。
* 后置：列表/加载只解容器头 + meta（不解析 CSV）；底图按需单独取。
* 副作用：读磁盘。

### 环境与 i18n（`env.rs` / `i18n.rs`）
* `env::{win_exe, fm_exe, fm_data_dir, fm_dir, fm_version_file, fm_db_version, run_flightmodel,
  extract_json}`：`binary/` 下的可执行文件定位与 `flightmodel.exe` 调用；`extract_json`
  从子进程输出里挑出 JSON（容忍前后杂音）。
* `i18n::{SUPPORTED, LEGACY_ALIASES, AUTO, FALLBACK, DEFAULT_LANG, Table::{get, len},
  take_log}`：界面语言表（`auto` 跟随系统，缺键回落 `en`）。

## 约束与取舍

* **只管子进程里的「自己的」**：按启动时记下的 pid 管理；`status_json` 里的
  `test_server_pid` 只报自己启动的那一个（`-1` = 没有），这是 `api_parity` 的契约字段。
* **关窗 ≠ 退出**：销毁 WebView、只留托盘（宿主侧 WebView2 运行库常驻，约 25 MB）。
* **运行期依赖 `WebView2Loader.dll`（非 MSVC 目标）**：`webview2-com-sys` 对 MSVC 目标静态链接
  loader，其它目标生成的是对 `WebView2Loader.dll` 的普通导入 ⇒ Windows GNU 版的这个 DLL 必须与
  `wp8f-gui.exe` 同目录（`build.sh` 从依赖 OUT_DIR 取 x64 那份放到仓库根，`zip.sh` 校验它在包里、
  且是 x64 PE）。缺了它进程根本起不来（issue #1）。
* **控制通道与 API 都只绑 127.0.0.1**，控制通道另有令牌；界面没有鉴权需求但也不对外暴露。
* 回放底图**不伪造、不外借**：记录没带底图就不铺底面地图（只留网格线）。
* 更新器只在**用户二次确认**后才切换目录（`data → data_old`、`data_new → data`），
  HUD 在跑时会因目录占用失败并提示先停止。

## 测试

Rust 单测在 WSL 里跑（严格模式，编译失败不会被吞成"通过"）：

```bash
bash scripts/test.sh          # 完整说明见 project.md 的「测试」
```

> `wp8f-gui` 自己的单测（`gui/src/children.rs` 里的归属判定 / 预览双向 reaper 纯逻辑）
> 不在 `scripts/test.sh` 的包列表里：它只能编到 Windows 目标，WSL 里无法执行。
> WSL 负责构建、Windows 负责运行：
>
> ```bash
> cargo test --offline --no-run -p wp8f-gui --target x86_64-pc-windows-gnu   # WSL：打印测试 exe 路径
> ```
> ```powershell
> Start-Process -Wait -NoNewWindow <打印出来的 target\...\deps\wp8f_gui-*.exe>   # Windows：跑到退出码
> ```
>
> 它是 GUI 子系统程序，直接 `& <exe>` 不会等它结束，必须 `Start-Process -Wait` 才拿得到退出码。

控制台相关的探针都要在 **Windows** 终端里跑（各自独立、输出实时可见）；跑之前先退出
正在运行的控制台：探针用 8799/8861/… 这类专用端口，单实例判据（端口独占）不会拦它，
但它和用户控制台**共用同一份 `logs/console-host.json` 与日志**，所以每个探针都会先查
"本机有没有别的 `wp8f-gui.exe`"，有就报"已有控制台在跑"并返回 2：

```bash
python scripts\tests\tray_e2e.py              # 托盘/窗口/子进程全链路（39 项断言，约 2 分钟）
python scripts\tests\tray_click_probe.py      # 托盘点击链路（消息级注入）
python scripts\tests\tui_checks.py            # 控制台 UI：导航/FM/回放/配置器/导弹页签
python scripts\tests\layout_check.py          # 配置器布局 + FM 图真渲染断言
python scripts\tests\tab_check.py             # 页签切换 + FM 图（6 张通用 + 喷气 EM）+ JS 异常断言
python scripts\tests\security_check.py        # 安全回归（Origin/Content-Type/体积/路径解码）
python scripts\tests\window_flash_check.py    # 调 FM 时不闪控制台黑框
python scripts\tests\ooda.py                  # 自动起 --no-window 实例，Edge 无头 dump DOM/JS 异常
python scripts\tests\api_parity.py selftest   # 与 scripts/tests/golden_python 比对
python scripts\tests\window_size.py           # 窗口 16:9（客户区，偏差 ≤5%）
python scripts\tests\diag_console.py          # 对已在运行的控制台做只读体检
```

探针的截图/临时日志落在 `logs/_probe/<脚本名>/`，全部通过时自动删除，有失败才保留。
在 WSL 里跑 `bash scripts/test.sh --gui` 也会打印这份清单。
