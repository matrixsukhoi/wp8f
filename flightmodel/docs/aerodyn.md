# 气动计算设计方案 (fm_parts.md)

## 1. 概述

本方案实现机体在任意速度、高度、攻角下的升力和阻力计算功能。

## 2. 数据结构

### 2.1 极曲线参数 (Polar)

每个气动面(WingPlane, FuselagePlane, HorStabPlane, VerStabPlane)都包含极曲线数据：

```
Polar {
    OswaldsEfficiencyNumber:r = 0.88    // 奥斯瓦尔德效率因子
    lineClCoeff:r = 0.075                // 升力线斜率 (每度攻角的升力系数增量)
    Cl0:r = 0.09                         // 零升力角对应的升力系数
    
    alphaCritHigh:r = 18                 // 临界攻角(正)
    alphaCritLow:r = -12                 // 临界攻角(负)
    ClCritHigh:r = 1.37                  // 临界攻角对应的升力系数(正)
    ClCritLow:r = -0.72                  // 临界攻角对应的升力系数(负)
    
    CdMin:r = 0.0066                     // 最小阻力系数(零升阻力)
    
    // 失速后参数
    AfterCritParabAngle:r = 2            // 失速后抛物线角度
    AfterCritDeclineCoeff:r = 0.05       // 失速后衰减系数
    ClAfterCritHigh:r = 0.9              // 失速后最大升力系数(正)
    ClAfterCritLow:r = -0.9              // 失速后最大升力系数(负)
    CxAfterCoeff:r = 0.01                // 失速后阻力增量系数
    
    // 马赫数修正参数 (7组)
    MachFactor:i = 3                      // 启用的马赫修正组数
    MachCrit1:r = 0.66                    // 临界马赫数1
    MachMax1:r = 0.85                     // 最大马赫数1
    MultMachMax1:r = 10                   // 最大马赫数乘数1
    MultLineCoeff1:r = -5.2               // 升力线斜率乘数1
    MultLimit1:r = 2                      // 限制值1
    ... (MachCrit2-7, MachMax2-7, MultMachMax2-7, MultLineCoeff2-7, MultLimit2-7)
}
```

### 2.2 气动面结构

```
WingPlane {
    Span:r = 12.49                        // 展长
    SweptAngle:r = 0                      // 后掠角
    TaperRatio:r = 1.35                   // 根梢比
    Angle:r = 2                            // 安装角(度)
    Areas {                                // 各部分面积
        LeftIn:r = 3.7195
        LeftMid:r = 4.7144
        LeftOut:r = 6.1521
        RightIn:r = 3.7195
        RightMid:r = 4.7144
        RightOut:r = 6.1521
        Aileron:r = 0.42035
    }
    FlapsPolar0 { ... }                    // 襟翼收起状态
    FlapsPolar1 { ... }                    // 襟翼放下状态
}

FuselagePlane {
    Span:r = 12.49
    Angle:r = 0
    Areas {
        Main:r = 29.172
    }
    Polar { ... }
}

HorStabPlane {
    Span:r = 5.0292
    Angle:r = 1.25
    Areas { ... }
    Polar { ... }
}

VerStabPlane {
    Span:r = 3.0
    Angle:r = 0
    Areas { ... }
    Polar { ... }
}
```

## 3. 核心数据结构 (Rust)

### 3.1 Mach修正参数

```rust
#[derive(Debug, Clone, Default)]
pub struct MachCorrection {
    pub mach_crit: f64,
    pub mach_max: f64,
    pub mult_mach_max: f64,
    pub mult_line_coeff: f64,
    pub mult_limit: f64,
}
```

### 3.2 极曲线数据

```rust
#[derive(Debug, Clone, Default)]
pub struct PolarData {
    // 基本参数
    pub oswalds_efficiency: f64,
    pub line_cl_coeff: f64,    // 升力线斜率 (1/度)
    pub cl0: f64,              // 零升力攻角的升力系数
    
    // 临界攻角
    pub alpha_crit_high: f64,  // 正临界攻角 (度)
    pub alpha_crit_low: f64,   // 负临界攻角 (度)
    pub cl_crit_high: f64,     // 正临界攻角对应的升力系数
    pub cl_crit_low: f64,      // 负临界攻角对应的升力系数
    
    // 最小阻力
    pub cd_min: f64,
    
    // 失速后参数
    pub after_crit_parab_angle: f64,
    pub after_crit_decline_coeff: f64,
    pub cl_after_crit_high: f64,
    pub cl_after_crit_low: f64,
    pub cx_after_coeff: f64,
    
    // 马赫修正 (最多7组)
    pub mach_corrections: Vec<MachCorrection>,
}
```

### 3.3 气动面数据

```rust
#[derive(Debug, Clone, Default)]
pub struct AeroSurface {
    pub name: String,
    pub span: f64,              // 展长
    pub angle: f64,             // 安装角 (度)
    pub area: f64,              // 投影面积
    
    // 展弦比和诱导阻力计算用
    pub taper_ratio: f64,
    pub swept_angle: f64,
    
    // 极曲线数据
    pub polar: PolarData,
    
    // 襟翼极曲线 (机翼特有)
    pub flaps_polar_0: Option<PolarData>,  // 襟翼收起
    pub flaps_polar_1: Option<PolarData>,  // 襟翼放下
}
```

### 3.4 气动组件汇总

```rust
#[derive(Debug, Clone, Default)]
pub struct AeroData {
    pub wing: AeroSurface,           // 机翼
    pub fuselage: AeroSurface,       // 机身
    pub hor_stab: AeroSurface,       // 水平尾翼
    pub ver_stab: AeroSurface,       // 垂直尾翼
    
    // 参考面积
    pub reference_area: f64,         // 机翼总面积
    
    // 附加阻力
    pub gear_cd: f64,                // 起落架阻力
    pub airbrake_cd: f64,            // 减速板阻力
    pub cockpit_door_cd: f64,       // 座舱盖阻力
}
```

## 4. 核心算法

### 4.1 马赫数修正

根据当前马赫数 M，在各修正区间线性插值得到修正因子：

```rust
fn apply_mach_correction(polar: &PolarData, mach: f64) -> (f64, f64, f64, f64, f64) {
    // 初始化修正因子
    let mut mult_line = 1.0;
    let mut mult_cd_min = 1.0;
    let mut mult_alpha_crit = 1.0;
    let mut mult_cl_crit = 1.0;
    
    for mc in &polar.mach_corrections {
        if mc.mach_crit == 0.0 && mc.mach_max == 0.0 {
            continue; // 跳过未使用的组
        }
        
        let k = if mach <= mc.mach_crit {
            1.0
        } else if mach >= mc.mach_max {
            mc.mult_mach_max
        } else {
            // 线性插值
            let t = (mach - mc.mach_crit) / (mc.mach_max - mc.mach_crit);
            1.0 + t * (mc.mult_mach_max - 1.0)
        };
        
        // 应用限制
        let k = k.max(1.0 - mc.mult_limit).min(1.0 + mc.mult_limit);
        
        // 分配到不同参数 (简化处理)
        mult_line *= k.powf(0.5);
        mult_cd_min *= k.powf(0.3);
        mult_alpha_crit *= k.powf(-0.2);
        mult_cl_crit *= k.powf(0.1);
    }
    
    (mult_line, mult_cd_min, mult_alpha_crit, mult_cl_crit, 1.0)
}
```

### 4.2 升力系数计算

根据攻角 alpha 计算升力系数：

```rust
fn get_lift_coefficient(polar: &PolarData, alpha: f64) -> f64 {
    // 获取马赫修正因子
    let (mult_line, _, _, mult_cl_crit, _) = apply_mach_correction(polar, mach);
    
    let alpha_crit_high = polar.alpha_crit_high * mult_alpha_crit;
    let cl_crit_high = polar.cl_crit_high * mult_cl_crit;
    let line_cl_coeff = polar.line_cl_coeff * mult_line;
    
    if alpha <= alpha_crit_high && alpha >= polar.alpha_crit_low {
        // 线性段: Cl = Cl0 + lineClCoeff * alpha
        polar.cl0 + line_cl_coeff * alpha
    } else if alpha > alpha_crit_high {
        // 失速后(正攻角)
        let da = (alpha - alpha_crit_high).min(polar.after_crit_parab_angle);
        let cl = cl_crit_high - polar.after_crit_decline_coeff * da * da;
        // 渐变到 after_crit 极限
        cl.max(polar.cl_after_crit_high)
    } else {
        // 失速后(负攻角)
        let da = ((polar.alpha_crit_low - alpha)).min(polar.after_crit_parab_angle);
        let cl = polar.cl_crit_low + polar.after_crit_decline_coeff * da * da;
        // 渐变到 after_crit 极限
        cl.min(polar.cl_after_crit_low)
    }
}
```

### 4.3 阻力系数计算

```rust
fn get_drag_coefficient(polar: &PolarData, cl: f64, mach: f64, area: f64, span: f64) -> f64 {
    // 马赫修正
    let (_, mult_cd_min, _, _, _) = apply_mach_correction(polar, mach);
    
    let cd_min = polar.cd_min * mult_cd_min;
    
    // 展弦比
    let aspect_ratio = if area > 0.0 { span * span / area } else { 0.0 };
    
    // 诱导阻力: Cd_ind = Cl^2 / (pi * AR * e)
    let e = polar.oswalds_efficiency;
    let cd_ind = if aspect_ratio > 0.0 && e > 0.0 {
        cl * cl / (std::f64::consts::PI * aspect_ratio * e)
    } else {
        0.0
    };
    
    // 失速后阻力增量
    let cd_stall = calculate_stall_drag(polar, alpha);
    
    cd_min + cd_ind + cd_stall
}
```

### 4.4 襟翼插值

```rust
fn get_blended_polar(polar0: &PolarData, polar1: &PolarData, flaps: f64) -> PolarData {
    // flaps: 0.0 = 收起, 1.0 = 放下
    // 线性插值所有参数
    PolarData {
        oswalds_efficiency: lerp(polar0.oswalds_efficiency, polar1.oswalds_efficiency, flaps),
        line_cl_coeff: lerp(polar0.line_cl_coeff, polar1.line_cl_coeff, flaps),
        cl0: lerp(polar0.cl0, polar1.cl0, flaps),
        alpha_crit_high: lerp(polar0.alpha_crit_high, polar1.alpha_crit_high, flaps),
        alpha_crit_low: lerp(polar0.alpha_crit_low, polar1.alpha_crit_low, flaps),
        cl_crit_high: lerp(polar0.cl_crit_high, polar1.cl_crit_high, flaps),
        cl_crit_low: lerp(polar0.cl_crit_low, polar1.cl_crit_low, flaps),
        cd_min: lerp(polar0.cd_min, polar1.cd_min, flaps),
        // ... 其他参数
    }
}
```

### 4.5 总升力和总阻力计算

```rust
pub struct AeroForces {
    pub lift: f64,           // 升力 (N)
    pub drag: f64,          // 阻力 (N)
    pub cl: f64,            // 总升力系数
    pub cd:总阻力系数
}

pub fn calculate_aero_forces(
    aero_data: &AeroData,
    velocity: f64,           // 真速 (m/s)
    altitude: f64,          // 高度 (m)
    alpha: f64,             // 攻角 (度)
    beta: f64,              // 侧滑角 (度)
    flaps: f64,             // 襟翼位置 [0, 1]
    gear_deployed: bool,   // 起落架是否放下
    airbrake_deployed: bool // 减速板是否放下
) -> AeroForces {
    // 空气密度
    let rho = atmosphere::density(altitude);
    let q = 0.5 * rho * velocity * velocity; // 动压
    
    // 获取各面的极曲线数据
    let wing_polar = if let (Some(p0), Some(p1)) = (&aero_data.wing.flaps_polar_0, &aero_data.wing.flaps_polar_1) {
        get_blended_polar(p0, p1, flaps)
    } else {
        aero_data.wing.polar.clone()
    };
    
    // 计算各面的升力和阻力
    // 机翼
    let wing_alpha = alpha + aero_data.wing.angle;
    let cl_wing = get_lift_coefficient(&wing_polar, wing_alpha, mach);
    let cd_wing = get_drag_coefficient(&wing_polar, cl_wing, mach, aero_data.wing.area, aero_data.wing.span);
    let wing_lift = q * aero_data.wing.area * cl_wing;
    let wing_drag = q * aero_data.wing.area * cd_wing;
    
    // 机身
    let fuse_alpha = alpha + aero_data.fuselage.angle;
    let cl_fuse = get_lift_coefficient(&aero_data.fuselage.polar, fuse_alpha, mach);
    let cd_fuse = get_drag_coefficient(&aero_data.fuselage.polar, cl_fuse, mach, aero_data.fuselage.area, aero_data.fuselage.span);
    let fuse_lift = q * aero_data.fuselage.area * cl_fuse;
    let fuse_drag = q * aero_data.fuselage.area * cd_fuse;
    
    // 水平尾翼 (考虑下洗)
    let downwash = calculate_downwash(aero_data, alpha, beta);
    let stab_alpha = alpha - downwash + aero_data.hor_stab.angle;
    let cl_stab = get_lift_coefficient(&aero_data.hor_stab.polar, stab_alpha, mach);
    let cd_stab = get_drag_coefficient(&aero_data.hor_stab.polar, cl_stab, mach, aero_data.hor_stab.area, aero_data.hor_stab.span);
    let stab_lift = q * aero_data.hor_stab.area * cl_stab;
    let stab_drag = q * aero_data.hor_stab.area * cd_stab;
    
    // 附加阻力
    let mut extra_cd = 0.0;
    if gear_deployed {
        extra_cd += aero_data.gear_cd;
    }
    if airbrake_deployed {
        extra_cd += aero_data.airbrake_cd;
    }
    let extra_drag = q * aero_data.reference_area * extra_cd;
    
    // 总升力和阻力
    let total_lift = wing_lift + fuse_lift + stab_lift;
    let total_drag = wing_drag + fuse_drag + stab_drag + extra_drag;
    
    // 总升力/阻力系数
    let cl = total_lift / (q * aero_data.reference_area);
    let cd = total_drag / (q * aero_data.reference_area);
    
    AeroForces {
        lift: total_lift,
        drag: total_drag,
        cl,
        cd,
    }
}
```

## 5. 实现步骤

1. **Step 1**: 扩展现有的 `PolarData` 结构，添加所有缺失字段
2. **Step 2**: 实现马赫修正计算函数
3. **Step 3**: 实现升力系数计算函数 (`get_lift_coefficient`)
4. **Step 4**: 实现阻力系数计算函数 (`get_drag_coefficient`)
5. **Step 5**: 实现襟翼状态插值函数
6. **Step 6**: 完善 `AeroSurface` 解析，读取 WingPlane/FuselagePlane/HorStabPlane/VerStabPlane
7. **Step 7**: 实现 `calculate_aero_forces` 主函数
8. **Step 8**: 添加大气密度计算辅助函数
9. **Step 9**: 集成到 FlightModel 中

## 6. 参考资料

- 原始 FM 文件格式: `resource/data/gamedata/flightmodels/fm/f4u-1a.blkx`
- voidmei 参考实现（独立仓库，不随本仓库分发）: `src/parser/blkx.java`
- 气动原理说明: `docs/aerodyn.md`
