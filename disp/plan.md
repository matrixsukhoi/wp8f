# Disp 模块实施计划

## 模块职责

Disp模块负责从共享内存读取DisplayData，并使用iced框架在透明窗口中显示飞行数据。参考voidmei的最简HUD设计。

## 当前状态

- [x] 基础HUD文本显示功能
- [ ] 透明窗口配置（iced 0.14升级）
- [ ] 矢量HUD组件绘制
- [ ] 窗口交互配置

## 技术选型

- **框架**: iced 0.14.x (已支持透明窗口)
- **窗口**: 透明、无边框、置顶、点击穿透
- **渲染**: 矢量绘制，可缩放、可配置

## 透明窗口方案

### 方案：升级到iced 0.14

 iced 0.14已完整支持透明窗口功能：

```rust
// iced 0.14 透明窗口配置
use iced::window::Settings;

let settings = Settings {
    transparent: true,
    decorations: false,  // 无边框
    always_on_top: true, // 置顶
    ..Default::default()
};
```

**支持的功能**：
- `transparent: bool` - 窗口透明
- `blur: bool` - 模糊背景（macOS/Linux）
- `decorations: bool` - 无边框窗口
- `level: Level` - 窗口层级（置顶）

### iced 0.14 API变更

主要变更：
1. `Application` trait 方法变化
2. `window::Settings` 替代直接配置
3. `Task` 替代 `Command`

## API设计

```rust
pub struct DispConfig {
    pub shared_memory_name: String,
    pub refresh_rate: u64,
    pub window_width: u32,
    pub window_height: u32,
    pub opacity: f32,
    pub always_on_top: bool,
    pub click_through: bool,
}

impl Default for DispConfig {
    fn default() -> Self {
        Self {
            shared_memory_name: "wp8f_display".to_string(),
            refresh_rate: 60,
            window_width: 800,
            window_height: 600,
            opacity: 0.8,
            always_on_top: true,
            click_through: true,
        }
    }
}
```

## HUD组件设计

### 基础组件

| 组件 | 描述 | 颜色 |
|------|------|------|
| SpeedIndicator | 速度表 (IAS/TAS/Mach) | 白色 |
| AltitudeIndicator | 高度表 | 白色 |
| HeadingIndicator | 航向指示器 | 白色 |
| AttitudeIndicator | 人工地平仪 | 青色 |
| ThrottleBar | 油门条 | 绿色 |
| EnginePanel | 发动机面板 | 绿色 |
| FuelGauge | 燃油量表 | 黄色/红色 |
| StallWarning | 失速警告 | 红色 |
| OverspeedWarning | 超速警告 | 红色 |

### 组件结构

```rust
pub trait HUDComponent {
    fn update(&mut self, data: &DisplayData);
    fn draw(&self) -> Element<Message>;
    fn position(&self) -> Position;
    fn set_position(&mut self, Position);
    fn scale(&self) -> f32;
    fn set_scale(&mut self, f32);
    fn color(&self) -> Color;
    fn set_color(&mut self, Color);
}
```

### 人工地平仪

```rust
pub struct AttitudeIndicator {
    pub width: f32,
    pub height: f32,
    pub pitch_scale: f32,  // 每度像素
    pub roll_scale: f32,  // 每度像素
}

impl AttitudeIndicator {
    pub fn new() -> Self;
    pub fn draw(&self, roll: f32, pitch: f32) -> Canvas;
}
```

## 窗口管理

```rust
pub struct DisplayWindow {
    config: DispConfig,
    // iced应用句柄
}

impl DisplayWindow {
    pub fn new(config: DispConfig) -> Result<Self, DisplayError>;
    pub fn run(&mut self) -> !;
    pub fn set_opacity(&mut self, opacity: f32);
    pub fn set_always_on_top(&mut self, on_top: bool);
    pub fn set_click_through(&mut self, through: bool);
}
```

## 实现步骤

### Step 1: 升级iced版本 (Day 1)

1. [ ] 更新Cargo.toml中iced版本从0.13到0.14
2. [ ] 适配API变更（application模块变化）
3. [ ] 添加透明窗口配置
4. [ ] 移除窗口边框，设置置顶
5. [ ] 测试透明效果

### Step 2: 数据读取 (Day 2)

1. [ ] 连接共享内存
2. [ ] 读取DisplayData
3. [ ] 处理数据有效性
4. [ ] 错误处理和重连

### Step 3: HUD组件 (Day 3-4)

1. [ ] 实现人工地平仪（矢量绘制）
2. [ ] 实现速度/高度刻度带
3. [ ] 实现发动机状态条
4. [ ] 实现燃油表
5. [ ] 实现告警指示
6. [ ] 实现航向指示器

### Step 4: 交互配置 (Day 5)

1. [ ] 实现组件拖拽
2. [ ] 实现颜色配置
3. [ ] 实现缩放配置
4. [ ] 配置文件保存/加载

## 平台支持

| 平台 | 透明 | 模糊 | 备注 |
|------|------|------|------|
| Linux | ✅ | ✅ | 需要 compositor 支持 |
| macOS | ✅ | ✅ | 需要开启模糊效果 |
| Windows | ✅ | ⚠️ | 仅部分后端支持 |

## 依赖

```toml
[dependencies]
iced = "0.14"
shared_memory = "0.12"
```

## 性能目标

- 渲染帧率: 60fps
- 内存占用: <20MB
- CPU占用: <5%
