# 能量机动(EM)曲线计算说明

本文档说明如何使用FM数据计算能量机动(Energy-Maneuverability)曲线。

## 1. 理论基础

### 1.1 能量机动理论

能量机动理论由John Boyd提出，核心概念：

| 指标 | 公式 | 说明 |
|------|------|------|
| 比能量 | E = h + V²/(2g) | 单位重量势能 + 动能 |
| 剩余功率 | P_s = (T-D)×V/W | 能量变化率 |
| 推重比 | T/W | 加速能力 |
| 翼载 | W/S | 盘旋性能 |

### 1.2 关键公式

**升力**:
```
L = 0.5 × ρ × V² × S × C_L
```

**阻力**:
```
D = 0.5 × ρ × V² × S × C_D
```

**阻力极曲线**:
```
C_D = C_D_min + C_L² / (π × e × AR)
```
其中:
- C_D_min: 最小阻力系数
- e: 奥斯瓦尔德效率因子
- AR: 展弦比

**剩余功率**:
```
P_s = (T - D) × V / W - d(h)/dt
```

## 2. FM数据需求

### 2.1 已实现的FM数据

| 数据 | 来源 | 状态 |
|------|------|------|
| 翼面积 S | Aerodynamics.WingPlane.Areas.Main | ✅ 已实现 |
| 展弦比 AR | (Span² / Area) | ✅ 已实现 |
| C_D_min | NoFlaps.CdMin | ✅ 已实现 |
| C_L_max | NoFlaps.ClCritHigh | ✅ 已实现 |
| 奥斯瓦尔德效率 | NoFlaps.OswaldsEfficiencyNumber | ✅ 已实现 |
| 推力曲线 | ThrustMax系数矩阵 | ✅ 已实现 |
| 功率曲线 | Compressor stages | ✅ 已实现 |
| 重量 | EmptyMass + 燃油 | ✅ 已实现 |

### 2.2 空气密度

标准大气模型:
```
ρ = ρ₀ × exp(-h/H)
```
其中:
- ρ₀ = 1.225 kg/m³ (海平面)
- H = 8500 m (标高)

## 3. 计算流程

### 3.1 阻力计算

```rust
// 获取气动参数
let cd_min = data.cd_min();           // 最小阻力系数
let cl_max = data.cl_max();           // 最大升力系数
let oswald = data.oswalds_efficiency(); // 奥斯瓦尔德效率
let wing_area = data.wing_area();     // 翼面积
let aspect_ratio = data.aspect_ratio(); // 展弦比

// 计算给定升力系数的阻力
fn drag_coefficient(cl: f64, cd_min: f64, oswald: f64, ar: f64) -> f64 {
    let k = 1.0 / (std::f64::consts::PI * oswald * ar);
    cd_min + k * cl * cl
}
```

### 3.2 喷气机推力计算

```rust
// 使用已有的推力曲线
if let Some(tc) = data.thrust_curve() {
    let thrust = tc.get_thrust(altitude, velocity);
}
```

### 3.3 剩余功率计算

```rust
fn specific_excess_power(
    thrust: f64,      // 推力 (N)
    velocity: f64,     // 速度 (m/s)
    weight: f64,      // 重量 (N)
    drag: f64,        // 阻力 (N)
) -> f64 {
    (thrust - drag) * velocity / weight  // m/s
}
```

### 3.4 转弯性能

**稳定盘旋转弯率**:
```
ω = g × sqrt(LoadFactor² - 1) / V
```

**最小稳定转弯半径**:
```
R_min = V / ω
```

## 4. 输出曲线

### 4.1 剩余功率-速度曲线

- X轴: 速度 (km/h)
- Y轴: 剩余功率 P_s (m/s)
- 多条曲线: 不同高度 (0m, 3000m, 6000m, 9000m)

### 4.2 转弯性能曲线

- X轴: 速度 (km/h)
- Y轴: 转弯率 ω (deg/s) 或 转弯半径 R (m)
- 多条曲线: 不同高度

### 4.3 能量机动图

- X轴: 速度
- Y轴: 高度
- 等值线: 剩余功率 (P_s = const)

## 5. 示例计算

### su-30sm2 在 6000m 高度

输入参数:
- 高度: 6000m
- 速度: 600 km/h (166.7 m/s)
- 重量: 25000 kg
- 翼面积: 65.98 m²
- 推力: ~3000 kgf (~29430 N)

计算:
- 空气密度 ρ ≈ 0.66 kg/m³
- 动压 q = 0.5 × 0.66 × 166.7² ≈ 9176 Pa
- 需要升力 L = W = 25000 × 9.81 = 245250 N
- 需要 C_L = L / (q × S) = 245250 / (9176 × 65.98) ≈ 0.41

查推力曲线得: T ≈ 29430 N
查阻力得: D ≈ 8000 N

剩余功率: P_s = (29430 - 8000) × 166.7 / 245250 ≈ 14.6 m/s

## 6. 参考资料

- Boyd, J. R. (1975). Energy Maneuverability Theory
- Blizard, K. W. & Hallion, R. P. (1977). The Boyds: Biography of Colonel John Boyd
- USAF Test Pilot School. Aircraft Performance Manual
