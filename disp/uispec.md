# DISP UI组件框架规格说明

## 概述

本文档定义了disp显示模块的UI组件框架规格，参考voidmei实现，提供可组合的飞行信息UI组件。

## 设计目标

- 使用标准字体大小作为基准，支持矢量缩放
- 提供可组合的组件框架（LabeledText、Compass、AttitudeIndicator等）
- 支持网格布局配置（M x N排列）
- 使用ab_glyph进行矢量字体渲染

## 模块结构

```
disp/
├── Cargo.toml
└── src/
    ├── lib.rs              # 主入口、窗口管理
    ├── components/         # UI组件框架
    │   ├── mod.rs         # 组件trait和公共定义
    │   ├── theme.rs       # 主题颜色定义
    │   ├── text_scale.rs  # 字体缩放管理
    │   ├── labeled_text.rs # LabeledText组件
    │   ├── compass.rs     # 罗盘组件
    │   └── attitude.rs    # 人工地平仪
    ├── layout/            # 布局管理
    │   └── mod.rs        # 网格布局器
    └── panels/            # 面板实现（使用组件）
        └── flight_info.rs # 飞行信息面板
```

## 组件规格

### 1. 字体缩放系统 (TextScale)

**设计原理**：以标准字体大小为基准，通过scale因子实现整体矢量缩放。

```rust
pub struct TextScale {
    base_size: f32,  // 标准字体大小，默认16.0
    scale: f32,      // 缩放因子，默认1.0
}

impl TextScale {
    pub fn new(base_size: f32) -> Self;
    pub fn with_scale(self, scale: f32) -> Self;
    
    // 尺寸计算
    pub fn data_size(&self) -> f32;    // 数据: 1x base_size
    pub fn label_size(&self) -> f32;   // 标签名: 2x base_size
    pub fn unit_size(&self) -> f32;    // 单位: 1x base_size
    
    // 布局尺寸
    pub fn row_height(&self) -> f32;  // 行高: label_size + unit_size
    pub fn col_width(&self) -> f32;   // 列宽: data区域宽度
}
```

**字体大小映射**：
| 元素 | 倍数 | 示例 (base=16, scale=1.0) |
|------|------|--------------------------|
| 数据值 | 2x | 32px |
| 标签名 | 1x | 16px |
| 单位 | 1x | 16px |

### 2. 主题系统 (Theme)

```rust
#[derive(Clone, Copy)]
pub struct Theme {
    pub data_color: RgbaColor,   // 数据颜色
    pub label_color: RgbaColor,  // 标签名颜色
    pub unit_color: RgbaColor,    // 单位颜色
    pub background: RgbaColor,   // 背景色（透明）
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            data_color: RgbaColor::new(0.0, 1.0, 0.0, 1.0),  // 绿色
            label_color: RgbaColor::new(1.0, 1.0, 1.0, 1.0), // 白色
            unit_color: RgbaColor::new(1.0, 1.0, 0.0, 1.0),  // 黄色
            background: RgbaColor::new(0.0, 0.0, 0.0, 0.0),  // 透明
        }
    }
}

impl Theme {
    pub fn warning(mut self) -> Self;   // 警告色（红色）
    pub fn critical(mut self) -> Self;  // 危险色（闪烁红）
}
```

### 3. LabeledText组件

**布局格式**：
```
+------------------------+
| [数据]          [标签] |
|                 [单位] |
+------------------------+
   ↑2x字体大小   ↑1x↑1x
```

**结构定义**：
```rust
pub struct LabeledText {
    pub data: String,      // 当前数据值
    pub label: String,    // 标签名
    pub unit: String,     // 单位
    pub color: RgbaColor, // 整体颜色（可覆盖theme）
    pub visible: bool,    // 是否显示
}

impl LabeledText {
    pub fn new(label: &str, unit: &str) -> Self;
    pub fn update(&mut self, value: f64, formatter: impl Fn(f64) -> String);
    pub fn set_visible(&mut self, visible: bool);
}

pub struct LabeledTextStyle {
    pub theme: Theme,
    pub text_scale: TextScale,
    pub label_width_ratio: f32,  // 标签区域宽度比例，默认3.0
}
```

**绘制方法**：
```rust
impl LabeledText {
    pub fn draw(&self, ctx: &mut DrawContext, x: f32, y: f32, style: &LabeledTextStyle);
}
```

**布局计算**：
- 总宽度 = 数据区域宽度 + 间距 + 标签区域宽度
- 数据区域宽度 = max(数据字符串宽度, 单位字符串宽度)
- 标签区域宽度 = 标签名字符串宽度
- 行高 = label_size + unit_size + 行间距

### 4. 网格布局器 (GridLayout)

**设计**：支持M x N排列的网格布局组件列表。

```rust
pub struct GridLayout {
    rows: usize,
    cols: usize,
    components: Vec<Box<dyn HUDComponent>>,
    spacing: f32,         // 组件间距
    padding: f32,         // 整体边距
    text_scale: TextScale,
}

impl GridLayout {
    pub fn new(rows: usize, cols: usize, text_scale: TextScale) -> Self;
    pub fn add_component(&mut self, component: Box<dyn HUDComponent>);
    pub fn set_spacing(&mut self, spacing: f32);
    pub fn set_padding(&mut self, padding: f32);
}

pub trait HUDComponent: Send {
    fn update(&mut self, data: &DisplayData);
    fn draw(&self, ctx: &mut DrawContext, x: f32, y: f32);
    fn width(&self, scale: &TextScale) -> f32;
    fn height(&self, scale: &TextScale) -> f32;
}
```

### 5. Compass组件（罗盘）

```rust
pub struct Compass {
    pub heading: f64,        // 当前航向
    pub tick_spacing: f32,   // 刻度间距
}

impl Compass {
    pub fn new(text_scale: TextScale) -> Self;
    pub fn draw(&self, ctx: &mut DrawContext, x: f32, y: f32, width: f32);
}
```

### 6. AttitudeIndicator（人工地平仪）

```rust
pub struct AttitudeIndicator {
    pub width: f32,
    pub height: f32,
    pub pitch_scale: f32,   // 每度像素
}

impl AttitudeIndicator {
    pub fn new(width: f32, height: f32) -> Self;
    pub fn draw(&self, ctx: &mut DrawContext, roll: f64, pitch: f64);
}
```

## 渲染上下文

```rust
pub struct DrawContext<'a> {
    buffer: &'a mut [u32],
    width: usize,
    height: usize,
    font: &'a FontRef,
}

impl DrawContext<'_> {
    pub fn draw_text(&mut self, x: f32, y: f32, text: &str, size: f32, color: RgbaColor);
    pub fn draw_line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, color: RgbaColor, width: f32);
}
```

## 使用示例

```rust
// 创建飞行信息面板
let text_scale = TextScale::new(16.0).with_scale(1.5);
let mut layout = GridLayout::new(4, 3, text_scale);

// 添加LabeledText组件
layout.add_component(Box::new(LabeledText::new("表速", "km/h")));
layout.add_component(Box::new(LabeledText::new("高度", "m")));
layout.add_component(Box::new(LabeledText::new("航向", "°")));

// 渲染
layout.update(&display_data);
layout.draw(&mut ctx, 50.0, 50.0);
```

## 颜色定义

| 用途 | 颜色 | RGBA |
|------|------|------|
| 正常数据 | 绿色 | (0, 255, 0, 255) |
| 标签名 | 白色 | (255, 255, 255, 255) |
| 单位 | 黄色 | (255, 255, 0, 255) |
| 警告 | 红色 | (255, 0, 0, 255) |
| 超速/失速警告 | 闪烁红 | (255, 0, 0, 255) |

## 坐标系

- 原点(0,0)在左上角
- Y轴向下增长
- 所有坐标使用浮点数，支持亚像素精度

## 性能目标

- 渲染帧率: 60fps
- 内存占用: <20MB
- 组件数量: 支持最多20个LabeledText组件
