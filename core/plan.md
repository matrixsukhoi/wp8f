# Core 模块实施计划

## 模块职责

Core模块是系统核心，负责协调各模块工作，实现主事件循环，进行飞行数据计算。

## 架构设计

**默认模式（同进程）**:
```
┌─────────────────────────────────────────────────────┐
│                    Core Event Loop                  │
├─────────────────────────────────────────────────────┤
│                                                     │
│  ┌──────────┐    ┌──────────┐    ┌──────────────┐│
│  │ Channel  │ -> │  Parser  │ -> │ Calculator    ││
│  └──────────┘    └──────────┘    └──────────────┘ │
│                                            │       │
│                                            v       │
│                                    ┌──────────────┐│
│                                    │ Arc<Mutex<>> ││
│                                    └──────────────┘│
│                                                     │
└─────────────────────────────────────────────────────┘
```

**shm模式（可选扩展）**:
启用`--features shm`时，额外创建共享内存供外部程序访问。

## API设计

### 配置

```rust
pub struct CoreConfig {
    pub tick_rate: u64,        // tick频率 Hz (默认60)
    pub sma_window: usize,      // 移动平均窗口大小
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            tick_rate: 60,
            sma_window: 5,
        }
    }
}
```

### 共享内存（可选）

```rust
// 仅在shm特性启用时可用
pub const SHM_NAME: &str = "wp8f_display";
```
            sma_window: 5,
            shared_memory_name: "wp8f_display".to_string(),
        }
    }
}
```

### 共享内存数据结构

```rust
// 显示数据结构（固定大小4096字节）
#[repr(C)]
pub struct DisplayData {
    pub magic: u32,           // 魔数 0xWP8F
    pub version: u32,         // 版本号
    pub timestamp: u64,       // 时间戳(毫秒)
    pub valid: u8,            // 数据有效标志
    
    // 飞行状态
    pub altitude: f64,        // 高度 m
    pub tas: f64,            // 真速 km/h
    pub ias: f64,            // 表速 km/h
    pub mach: f64,           // 马赫数
    pub aoa: f64,            // 攻角 deg
    pub aos: f64,            // 侧滑角 deg
    pub ny: f64,             // 法向过载 G
    pub vy: f64,             // 垂直速度 m/s
    pub heading: f64,        // 航向 deg
    pub roll: f64,           // 滚转角 deg
    pub pitch: f64,          // 俯仰角 deg
    
    // 操纵面
    pub aileron: f64,         // 副翼 %
    pub elevator: f64,        // 升降舵 %
    pub rudder: f64,         // 方向舵 %
    pub flaps: f64,          // 襟翼 %
    pub gear: f64,           // 起落架 %
    pub airbrake: f64,        // 减速板 %
    
    // 燃油
    pub fuel_kg: f64,        // 当前燃油 kg
    pub fuel_percent: f64,   // 燃油百分比 %
    
    // 发动机 (8个)
    pub engine_count: usize,
    pub engine_throttle: [f64; 8],
    pub engine_power: [f64; 8],
    pub engine_rpm: [f64; 8],
    pub engine_thrust: [f64; 8],
    pub engine_temp_water: [f64; 8],
    pub engine_temp_oil: [f64; 8],
    
    // 计算结果
    pub sep: f64,             // 剩余功率 m/s
    pub stall_warning: u8,    // 失速警告
    pub overspeed_warning: u8,// 超速警告
    pub g_load: f64,          // 当前过载
    
    // 飞机信息
    pub aircraft_type: [u8; 64], // 机型名称
    
    // 预留填充
    pub _reserved: [u8; 4000],
}
```

### 计算器

```rust
pub trait Calculator {
    fn calculate(&mut self, state: &FlightState) -> DisplayData;
}

pub struct FlightCalculator {
    sma_altitude: SimpleMovingAverage,
    sma_speed: SimpleMovingAverage,
    sma_aoa: SimpleMovingAverage,
    // ... 更多SMA滤波器
}

impl FlightCalculator {
    pub fn new(window_size: usize) -> Self;
    pub fn calculate(&mut self, state: &FlightState) -> DisplayData;
}
```

## 计算公式

### 剩余功率 (SEP)
```
SEP = (Thrust - Drag) * TAS / Weight - Vy
    = (Thrust - 0.5 * rho * V^2 * Cd * S) * V / Weight - Vy
```

### 失速警告
```
当 AoA > 临界攻角 - 安全余度 时触发
```

### 超速警告
```
当 TAS > VNE 时触发
```

## 实现步骤

### Step 1: 基础框架 (Day 1)

1. 创建Cargo.toml和项目结构
2. 实现SimpleMovingAverage滤波器
3. 实现事件循环框架

### Step 2: 数据流 (Day 2)

1. 集成Channel模块读取数据
2. 集成Parser模块解析数据
3. 实现计算逻辑
4. 写入共享内存

### Step 3: 状态机 (Day 3)

1. 实现冷启动状态
2. 实现热启动状态
3. 实现断线重连
4. 实现错误恢复

### Step 4: 配置和优化 (Day 4)

1. 实现配置文件读取
2. 性能优化
3. 内存优化

## 性能目标

- 事件周期: 16.67ms (60Hz)
- 计算延迟: <2ms
- 共享内存写入: <0.1ms
- 内存占用: <10MB
