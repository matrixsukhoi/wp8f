# Flight Model 参数详解

本文档详细解释战争雷霆(War Thunder)飞行模型(Flight Model)中的各项参数，并列出可计算、分析和挖掘的信息。

## 数据文件位置

- 主BLKX文件: `resource/data/gamedata/flightmodels/{aircraft}.blkx`
- FM文件: `resource/data/gamedata/flightmodels/fm/{aircraft}.blkx`

## 参数分类

### 1. 基础参数 (Basic Parameters)

| 参数名 | 类型 | 说明 | 示例(yak-3) | 示例(mig-21) | 示例(su-30SM2) |
|--------|------|------|-------------|--------------|-----------------|
| model | string | 飞机模型名称 | yak_3 | mig-21_bis | su_30sm2 |
| fmFile | string | FM文件路径 | fm/yak-3.blk | fm/mig-21_bis.blk | fm/su_30sm2.blk |
| EmptyMass | real | 空重(kg) | 2263 | 4600 | 18800 |
| MaxFuelMass0 | real | 主油箱容量(kg) | 270 | 2200 | 9400 |
| Length | real | 机身长度(m) | 8.5 | 14.1 | 21.93 |
| Crew | integer | 乘员数 | 1 | 1 | 2 |

### 2. 速度限制 (Speed Limits)

| 参数名 | 类型 | 说明 | yak-3 | mig-21 | su-30SM2 |
|--------|------|------|-------|--------|----------|
| Vne | real | 最大允许表速(km/h) | 685 | 1365 | 1400 |
| VneControl | real | 操控最大速度 | 685 | 1500 | 1400 |
| VneMach | real | 最大允许马赫数 | 0.73 | 2.21 | 1.8 |
| CriticalSpeed | real | 临界速度 | 50 | 50 | 50 |
| MaxSpeedNearGround | real | 低空最大速度 | 570 | 1093 | 900 |
| MaxSpeedAtAltitude | real | 高空最大速度 | 575 | 1060 | 930 |

### 3. 气动参数 (Aerodynamics)

#### 3.1 阻力系数 (Drag Coefficients)

| 参数名 | 类型 | 说明 | yak-3 | mig-21 | su-30SM2 |
|--------|------|------|-------|--------|----------|
| GearCd | real | 起落架阻力系数 | 0.025 | 0.01 | 0.035 |
| GearCentralCd | real | 中央起落架阻力 | 0 | 0 | 0 |
| RadiatorCd | real | 散热器阻力系数 | 0.0045 | 0 | 0 |
| OilRadiatorCd | real | 油冷器阻力系数 | 0.004 | 0 | 0.002 |
| AirbrakeCd | real | 减速板阻力系数 | 0 | 0.065 | 0.07 |
| FuseCd | real | 机身附加阻力 | 0 | 0 | 0 |
| BombBayCd | real | 炸弹舱阻力 | 0 | 0 | 0 |
| CockpitDoorCd | real | 座舱盖阻力 | 0.05 | 0.07321 | 0.05321 |

#### 3.2 升力参数 (Lift Parameters)

| 参数名 | 类型 | 说明 |
|--------|------|------|
| OswaldsEfficiencyNumber | real | 奥斯瓦尔德效率因子(翼展效率) |
| lineClCoeff | real | 升力曲线线性段斜率 |
| Cl0 | real | 零攻角升力系数 |
| alphaCritHigh | real | 正临界攻角(度) |
| alphaCritLow | real | 负临界攻角(度) |
| ClCritHigh | real | 正临界攻角对应升力系数 |
| ClCritLow | real | 负临界攻角对应升力系数 |
| CdMin | real | 最小阻力系数(零升阻力) |
| ClAfterCritHigh | real | 失速后升力系数(正) |
| ClAfterCritLow | real | 失速后升力系数(负) |

#### 3.3 马赫效应 (Mach Effects)

| 参数名 | 类型 | 说明 |
|--------|------|------|
| MachFactor | integer | 马赫效应因子数量 |
| MachCrit1-7 | real | 各临界马赫数 |
| MachMax1-7 | real | 各最大马赫数 |
| MultMachMax1-7 | real | 马赫数乘数 |
| MultLineCoeff1-7 | real | 线性系数乘数 |
| MultLimit1-7 | real | 限制乘数 |

### 4. 机翼参数 (Wing Parameters)

| 参数名 | 类型 | 说明 | yak-3 | mig-21 | su-30SM2 |
|--------|------|------|-------|--------|----------|
| Wingspan | real | 翼展(m) | 9.2 | 7.15 | 14.7 |
| SweptWingAngle | real | 后掠角(度) | 2 | 50 | 40 |
| WingTaperRatio | real | 梯形比 | 2.5 | 5.2 | 3.2 |
| WingAngle | real | 安装角(度) | 0 | 0 | 1 |
| VAngle | real | 上反角(度) | 3 | 0 | 0 |
| AspectRatio | real | 展弦比(计算值) | 5.7 | 2.2 | ~3.6 |

#### 机翼面积 (Wing Areas)

| 参数名 | 说明 | yak-3 | mig-21 | su-30SM2 |
|--------|------|-------|--------|----------|
| Areas.LeftIn | 左内侧机翼面积 | 3.46 | 3.83 | 10.33 |
| Areas.LeftMid | 左中段机翼面积 | 2.543 | 3.83 | 10.33 |
| Areas.LeftOut | 左外段机翼面积 | 1.411 | 3.83 | 12.33 |
| Areas.RightIn | 右内侧机翼面积 | 3.46 | 3.83 | 10.33 |
| Areas.RightMid | 右中段机翼面积 | 2.543 | 3.83 | 10.33 |
| Areas.RightOut | 右外段机翼面积 | 1.411 | 3.83 | 12.33 |
| Areas.Aileron | 副翼面积 | 0.33 | 0.6 | 1.0 |
| Areas.Fuselage | 机身面积 | 14.828 | 22.98 | - |

### 5. 襟翼参数 (Flaps Parameters)

#### 襟翼档位定义

| 参数名 | 说明 |
|--------|------|
| FlapsAxis.Retracted | 收起档位 |
| FlapsAxis.Combat | 战斗档位(20%) |
| FlapsAxis.Takeoff | 起飞档位(33%) |
| FlapsAxis.Landing | 着陆档位(100%) |

#### 襟翼气动数据

| 参数名 | 说明 | NoFlaps(0%) | FullFlaps(100%) |
|--------|------|-------------|-----------------|
| Flaps | 襟翼位置 | 0 | 1 |
| lineClCoeff | 升力曲线斜率 | 0.077 | 0.077 |
| alphaCritHigh | 正临界攻角 | 18.1 | 16.7 |
| alphaCritLow | 负临界攻角 | -13 | -14.5 |
| ClCritHigh | 正临界升力系数 | 1.32 | 1.52 |
| ClCritLow | 负临界升力系数 | -0.7 | -0.35 |
| CdMin | 零升阻力系数 | 0.0091 | 0.0698 |

### 6. 发动机参数 (Engine Parameters)

#### 6.1 发动机类型识别

```rust
// 通过EngineType0.Main.Type判断
EngineType0.Main.Type = "Jet"      // 喷气机
EngineType0.Main.Type = "Piston"   // 活塞机
EngineType0.Main.Type = "Turboprop" // 涡桨机
```

#### 6.2 喷气机参数 (Jet Engine)

| 参数名 | 类型 | 说明 | mig-21示例 |
|--------|------|------|------------|
| Main.Type | string | 发动机类型 | Jet |
| Main.Thrust | real | 额定推力 | 2600 |
| Main.AfterburnerBoost | real | 加力推力倍数 | 1.22 |
| Main.RPMMin | real | 最小转速 | 3000 |
| Main.RPMMax | real | 最大转速 | 11500 |
| Main.RPMAfterburner | real | 加力转速 | 11500 |
| Main.RPMMaxAllowed | real | 允许最大转速 | 14000 |

##### 推力高度速度曲线 (ThrustMax)

| 参数名 | 类型 | 说明 |
|--------|------|------|
| ThrustMax.ThrustMax0 | real | 静推力(kgf) |
| ThrustMax.Altitude_* | real | 高度节点(m) |
| ThrustMax.Velocity_* | real | 速度节点(km/h) |
| ThrustMax.ThrustMaxCoeff_*_* | real | 推力系数(2D矩阵) |

推力计算公式:
```
Thrust(altitude, velocity) = ThrustMax0 * ThrustMaxCoeff[alt_idx][vel_idx]
```

示例(mig-21):
- 高度节点: [0, 3000, 5000, 8000, 10000, 12000, 15000, 18000, 24000] m
- 速度节点: [0, 200, 400, 600, 800, 1000, 1200, 1400, 1600, 1900, 2200, 2400, 2600] km/h
- 静推力: 4040 kgf

##### 加力推力 (Afterburner)

| 参数名 | 说明 |
|--------|------|
| ThrustMax.ThrAftMaxCoeff_*_* | 加力推力系数矩阵 |

#### 6.3 活塞机参数 (Piston Engine)

| 参数名 | 类型 | 说明 | yak-3示例 |
|--------|------|------|------------|
| Main.Type | string | 发动机类型 | Piston |
| Main.Power | real | 额定功率(hp) | 1290 |
| Main.RPMMax | real | 最大转速 | 2700 |
| Main.RPMAfterburner | real | 加力转速 | 2700 |
| Main.RPMMaxAllowed | real | 允许最大转速 | 2800 |
| Main.ThrottleBoost | real | 油门增加倍数 | 1.0 |

##### 增压器参数 (Compressor)

| 参数名 | 类型 | 说明 | yak-3示例 |
|--------|------|------|------------|
| Compressor.NumSteps | integer | 增压器档位数 | 1 |
| Compressor.Altitude* | real | 档位切换高度(m) | 18300 |
| Compressor.Power* | real | 档位额定功率(hp) | 1310/1240 |
| Compressor.PowerAtCeiling* | real | 升限功率(hp) | 670/510 |
| Compressor.PowerConstRPMCurvature* | real | RPM功率曲线 | 1 |

功率随高度变化:
- 在Altitude以下: 使用Power值
- 在Altitude和Ceiling之间: 插值
- 在Ceiling以上: 使用PowerAtCeiling值

### 7. 舵面控制参数 (Control Parameters)

| 参数名 | 类型 | 说明 | yak-3 | mig-21 | su-30SM2 |
|--------|------|------|-------|--------|----------|
| AileronEffectiveSpeed | real | 副翼有效速度 | 380 | 650 | 680 |
| AileronPowerLoss | real | 副翼功率损失 | 3 | 1.4 | 1.1 |
| RudderEffectiveSpeed | real | 方向舵有效速度 | 420 | 600 | 750 |
| RudderPowerLoss | real | 方向舵功率损失 | 2.8 | 1.8 | 1.1 |
| ElevatorsEffectiveSpeed | real | 升降舵有效速度 | 490 | 950 | 700 |
| ElevatorPowerLoss | real | 升降舵功率损失 | 3.4 | 2.4 | 1.1 |

### 8. 惯性参数 (Inertia Parameters)

| 参数名 | 类型 | 说明 | yak-3 | mig-21 | su-30SM2 |
|--------|------|------|-------|--------|----------|
| MomentOfInertia | real[3] | 三轴转动惯量 [Ix, Iy, Iz] | [4100, 11850, 8100] | [6050, 52000, 49500] | [55000, 250000, 210000] |

单位: kg*m^2

### 9. 过载限制 (G-Load Limits)

| 参数名 | 类型 | 说明 | yak-3 | mig-21 |
|--------|------|------|-------|--------|
| WingPlane.Strength.CritOverload | real[2] | 临界过载 [负, 正] | [-120000, 160000] | - |
| Strength.CritOverload | real[2] | 临界过载 | - | - |

### 10. 襟翼限制速度

| 参数名 | 类型 | 说明 |
|--------|------|------|
| FlapsDestructionIndSpeed | real | 襟翼损毁指示速度 |
| FlapsDestructionIndSpeedP0-P4 | real[2] | 各档位襟翼限制速度 |

## 计算公式

### 展弦比 (Aspect Ratio)
```
AR = Wingspan^2 / WingArea
```

### 诱导阻力系数 (Induced Drag Coefficient)
```
Cd_i = Cl^2 / (π * AR * OswaldsEfficiencyNumber)
```

### 升力 (Lift)
```
L = 0.5 * ρ * V^2 * S * Cl
```
其中:
- ρ = 空气密度(随高度变化)
- V = 速度(m/s)
- S = 机翼面积(m²)
- Cl = 升力系数(随攻角变化)

### 推力 (喷气机)
```
T(altitude, velocity) = T0 * Coefficient[alt_idx][vel_idx]
```
其中:
- T0 = ThrustMax0 (静推力)
- alt_idx = 高度对应索引
- vel_idx = 速度对应索引

### 功率 (活塞机)
```
P(altitude, throttle) = P0 * throttle * DensityRatio(altitude)
```

## 数据示例

### yak-3 (活塞机)
```
EmptyMass: 2263 kg
MaxFuelMass0: 270 kg
Vne: 685 km/h
Wingspan: 9.2 m
WingArea: 14.828 m²
AspectRatio: 5.7
Engine: VK-105PF (Piston)
Power: 1290 hp
Compressor: 1 stage
```

### mig-21_bis (喷气机/二代机)
```
EmptyMass: 4600 kg
MaxFuelMass0: 2200 kg
Vne: 1365 km/h
VneMach: 2.21
Wingspan: 7.15 m
WingArea: 22.98 m²
Engine: Tumansky R-13 (Jet)
ThrustMax0: 4040 kgf
AfterburnerBoost: 1.22x
Altitude nodes: 9 (0-24000m)
Velocity nodes: 13 (0-2600km/h)
```

### su-30SM2 (喷气机/三代++重型战斗机)
```
EmptyMass: 18800 kg
MaxFuelMass0: 9400 kg
Vne: 1400 km/h
VneMach: 1.8+
Wingspan: 14.7 m
WingArea: ~60 m²
SweptWingAngle: 40°
Engine: 2x AL-31F (Jet)
ThrustMax0: 8800 kgf (each, total 17600 kgf)
AfterburnerBoost: 1.15x
Altitude nodes: 7 (0-25000m)
Velocity nodes: 12 (0-2400km/h)
OswaldsEfficiencyNumber: 0.53
lineClCoeff: 0.065
AlphaCritHigh: 32°
ClCritHigh: 1.5
CdMin: 0.0071
MomentOfInertia: [55000, 250000, 210000]
AileronEffectiveSpeed: 680 km/h
RudderEffectiveSpeed: 750 km/h
ElevatorsEffectiveSpeed: 700 km/h
HasThrustVectoringMode: true
HasManeuverabilityMode: true
```

### 现代喷气机特殊参数 (su-30SM2)

#### 可变后掠翼相关
| 参数名 | 类型 | 说明 | su-30SM2 |
|--------|------|------|-----------|
| SweepWingActuatorSpeed | real | 机翼后掠角变化速度 | 0.2 |
| WingWaveMassRel | real | 机翼质量相对位置 | 0.25 |
| SweepAxisByMachAuto | real[3] | 马赫数自动后掠 | 0, 0, 1 |

#### 飞行控制系统
| 参数名 | 类型 | 说明 | su-30SM2 |
|--------|------|------|-----------|
| FlyByWire | block | 电传飞控配置 | 有 |
| HasManeuverabilityMode | bool | 机动模式 | true |
| HasThrustVectoringMode | bool | 推力矢量模式 | true |
| ConvertAoa | bool | AoA转换 | true |

#### 发动机工作模式
| 参数名 | 类型 | 说明 | su-30SM2 | f-104c |
|--------|------|------|----------|--------|
| TurbineTimeConstant | real | 涡轮时间常数 | 1.2 | 2.6 |
| FuelConsumptionOnIdle | real | 怠速油耗(kg/s) | 0.108 | 0.108 |
| FuelConsumptionOnHalfThr | real | 半油门油耗(kg/s) | 0.43 | 0.476 |
| FuelConsumptionOnFullThr | real | 全油门油耗(kg/s) | 0.88 | 1.15 |
| AfterburnerBoost | real | 加力燃烧倍数 | 1.15 | - |
| ConsumptionOmegaMax | real | 最大耗油系数 | 0.77 | 1.05 |
| Mode{N}.Throttle | real | 油门位置 | - | 0-1.1 |
| Mode{N}.RPM | real | 发动机转速 | - | 0.66-1.0 |
| Mode{N}.ThrustMult | real | 推力倍数 | - | 0.05-1.4 |
| Mode{N}.ConsumptionMult | real | 各模式耗油倍数 | 1.0-2.5 | 1.0-2.3 |

##### 发动机模式 (EngineMode) 详解

喷气式发动机通过EngineMode定义不同油门下的推力和油耗特性：

```
Mode0 {
    Throttle:r = 0        # 油门位置 (0 = 怠速)
    RPM:r = 0.66          # 发动机转速百分比
    ThrustMult:r = 0.05   # 推力倍数
    ConsumptionMult:r = 1.8 # 油耗倍数
}
Mode4 {
    Throttle:r = 1         # 100%油门 (军推最大)
    RPM:r = 0.993
    ThrustMult:r = 1       # 推力倍数为1 (最大军推)
    ConsumptionMult:r = 1
}
Mode5 {
    Throttle:r = 1.1       # WEP/加力油门 (>1.0)
    RPM:r = 1
    ThrustMult:r = 1.4     # WEP推力倍数为1.4
    ConsumptionMult:r = 2.3
}
```

耗油率计算公式为：
\[
\text{燃油流量 (kg/h)} = \text{推力 (kgf)} \times FuelConsumptionOnFullThr or FuelConsumptionOnWEPThr \times \text{ConsumptionMult}
\]
**WEP推力计算公式**:
```
WEP推力 = ThrustMax0 × AfterburnerBoost × 最后Mode的ThrustMult
```

如果存在`ThrAftMaxCoeff`矩阵(加力推力系数矩阵)，则WEP推力直接从矩阵获取。

##### 推力矢量 (Thrust Vectoring)
| 参数名 | 类型 | 说明 |
|--------|------|------|
| ThrustVectoringMode | block | 推力矢量控制配置 |
| VtolMode | block | 垂直起降模式 |

### 11. 起飞重量计算 (Takeoff Weight)

#### 11.1 质量参数

| 参数名 | 类型 | 说明 | su-30sm2 | yak-3 | f-4e |
|--------|------|------|-----------|-------|------|
| EmptyMass | real | FM定义空重(kg) | 18800 | 2263 | 13990 |
| MaxFuelMass0 | real | 主油箱容量(kg) | 9400 | 270 | 7714 |
| MaxFuelMassExternal0 | real | 副油箱容量(kg) | 0 | 0 | 1850 |
| OilMass | real | 润滑油质量(kg) | 30 | 30 | 35 |

#### 11.2 起飞重量公式

```
起飞重量 = EmptyMass + 弹药 + 飞行员 + 其他 + 燃油重量
```

其中:
- `EmptyMass`: FM文件中定义的基础空重
- `弹药`: 机炮弹药、炸弹、导弹等
- `飞行员`: 乘员重量
- `其他`: 对抗措施(热诱弹/铝箔)、附加设备等
- `燃油重量`: MaxFuelMass0 × 燃油百分比

**简化公式** (不考虑额外载荷):
```
takeoff_weight(fuel_percent) = EmptyMass + MaxFuelMass0 × (fuel_percent / 100)
```

**带副油箱**:
```
takeoff_weight_with_ext_tanks(fuel_percent) = EmptyMass + (MaxFuelMass0 + MaxFuelMassExternal0) × (fuel_percent / 100)
```

#### 11.3 示例计算

**su-30sm2** (100%内油):
- EmptyMass: 18800 kg
- MaxFuelMass0: 9400 kg
- 计算: 18800 + 9400 = 28200 kg
- 实际游戏数据: 28559 kg
- 差异: ~360 kg (弹药 + 2名飞行员 + 其他设备)

**yak-3** (100%内油):
- EmptyMass: 2263 kg
- MaxFuelMass0: 270 kg
- 计算: 2263 + 270 = 2533 kg
- 实际游戏数据: 2680 kg
- 差异: ~148 kg (弹药 + 飞行员)

**f-4e** (100%内油):
- EmptyMass: 13990 kg
- MaxFuelMass0: 7714 kg
- 计算: 13990 + 7714 = 21704 kg
- 实际游戏数据: 20168 kg
- 差异: ~-1536 kg (可能包含副油箱或数据差异)

## 可计算/分析/挖掘的信息

基于上述FM参数，可以进行以下计算、分析和挖掘：

### 12. 气动性能分析

#### 12.1 失速特性 (Stall Characteristics)

| 可计算指标 | 计算方法 | 数据来源 |
|-----------|----------|----------|
| 失速速度(Stall Speed) | V_stall = sqrt(2*W/(ρ*S*Cl_max)) | Cl_max, WingArea, EmptyMass |
| 失速攻角(Stall AoA) | 直接读取 | alphaCritHigh, alphaCritLow |
| 失速后升力系数 | 直接读取 | ClAfterCritHigh, ClAfterCritLow |
| 失速后阻力增量 | CxAfterCoeff | 失速后阻力增加 |

#### 12.2 升阻比分析 (Lift-to-Drag Ratio)

| 可计算指标 | 计算方法 | 数据来源 |
|-----------|----------|----------|
| 最大升阻比(L/D max) | (L/D)_max = 0.5 * sqrt(π*AR*e/Cd_min) | AR, e, Cd_min |
| 最佳巡航速度 | V_opt = sqrt(2*W/(ρ*S) * sqrt(π*AR*e/(3*Cd_min))) | 同上 |
| 零升阻力系数 | Cd_min | 直接读取 |
| 诱导阻力因子 | k = 1/(π*AR*e) | AR, Oswalds |

#### 12.3 极曲线分析 (Polar Analysis)

- 根据Cl-Cd极曲线数据，可以绘制完整极曲线
- 分析不同攻角下的升阻特性
- 确定作战机动攻角范围

### 13. 飞行性能计算

#### 13.1 爬升性能 (Climb Performance)

| 可计算指标 | 计算方法 | 数据来源 |
|-----------|----------|----------|
| 爬升率(Rate of Climb) | RC = (T-D)*V/W | T, D, W, V |
| 爬升角(Climb Angle) | γ = arcsin((T-D)/W) | T, D, W |
| 最大爬升率 | 在最佳速度下求解 | T(alt, V), D(V, alt) |
| 爬升时间 | ∫dh/RC(altitude) | 爬升率曲线积分 |
| 理论升限 | RC=0的高度 | T(alt) = D(alt) |

#### 13.2 盘旋性能 (Turn Performance)

| 可计算指标 | 计算方法 | 数据来源 |
|-----------|----------|----------|
| 瞬时盘旋角速度 | ω = g*sqrt(n²-1)/V | 过载n, 速度V |
| 最小盘旋半径 | R_min = V²/(g*sqrt(n_max²-1)) | V, n_max |
| 稳定盘旋速度 | 升力=重力分量+向心力 | Cl, W, V |
| 最大盘旋过载 | 直接读取 | WingPlane.Strength.CritOverload |
| 持续盘旋能力 | T >= D + W*sin(γ) | 推力曲线, 阻力曲线 |

#### 13.3 俯冲性能 (Dive Performance)

| 可计算指标 | 计算方法 | 数据来源 |
|-----------|----------|----------|
| 俯冲加速 | a = g*(sin(γ) - (D/W)*cos(γ)) | 阻力曲线 |
| 俯冲速度增长 | dV/dt = a + V²/R | 重力分量 |
| 最大俯冲速度 | Vne, VneMach | 速度限制 |

#### 13.4 续航性能 (Endurance & Range)

| 可计算指标 | 计算方法 | 数据来源 |
|-----------|----------|----------|
| 最大航程(Range) | R = (L/D)_max * (FuelMass/Weight) * g | Cd_min, AR, e, FuelMass |
| 最大续航时间(Endurance) | E = η*FuelMass/(ρ*V*Cd) | 燃油流量, 阻力系数 |
| 最佳巡航高度 | 升阻比最大的高度 | T(alt)=D(alt) |
| 燃油消耗率 | FuelFlow = f(throttle, RPM, altitude) | Engine modes |

#### 13.5 加速性能 (Acceleration)

| 可计算指标 | 计算方法 | 数据来源 |
|-----------|----------|----------|
| 剩余加速度 | a = (T-D)/m | 推力, 阻力, 质量 |
| 速度增量时间 | Δt = m*ΔV/(T-D) | 同上 |
| 能量时间常数 | τ = m*V/(T-D) | 能量机动性指标 |

### 14. 操纵性能分析

#### 14.1 舵面效率

| 可计算指标 | 计算方法 | 数据来源 |
|-----------|----------|----------|
| 副翼效力 | 随速度增加而下降 | AileronEffectiveSpeed, AileronPowerLoss |
| 方向舵效力 | 随速度增加而下降 | RudderEffectiveSpeed, RudderPowerLoss |
| 升降舵效力 | 随速度增加而下降 | ElevatorsEffectiveSpeed, ElevatorPowerLoss |

**速度对舵效的影响**:
```
PowerFactor = (1 - (V/V_effective)^PowerLoss)
当V > V_effective时，PowerFactor < 0，舵面反向效率
```

#### 14.2 滚转/俯仰/偏航响应

| 可计算指标 | 计算方法 | 数据来源 |
|-----------|----------|----------|
| 滚转角速度 | p = δ_a * Q * S * b * Cl_p / Ix | 转动惯量Ix, 副翼面积 |
| 俯仰角速度 | q = δ_e * Q * S * c * Cm_q / Iy | 转动惯量Iy, 尾翼面积 |
| 偏航角速度 | r = δ_r * Q * S * b * Cn_r / Iz | 转动惯量Iz, 垂尾面积 |
| 操纵延迟 | 时间常数 = 1/(ω_n * ζ) | 固有频率, 阻尼比 |

#### 14.3 配平特性

- 配平攻角随速度变化
- 配平配平速度
- 重心位置影响

### 15. 结构与强度分析

#### 15.1 过载限制

| 可计算指标 | 数据来源 |
|-----------|----------|
| 正极限过载 | WingPlane.Strength.CritOverload[1] |
| 负极限过载 | WingPlane.Strength.CritOverload[0] |
| 最大使用过载 | 结构强度限制 |

#### 15.2 速度限制

| 可计算指标 | 数据来源 |
|-----------|----------|
| Vne (最大允许速度) | 速度限制 |
| VneMach (马赫限制) | 马赫数限制 |
| 襟翼限制速度 | FlapsDestructionIndSpeed |

#### 15.3 应力分析

- 翼载(Wing Loading): W/S = Mass*g/WingArea
- 推重比(Thrust-to-Weight): T/W
- 功重比(Power-to-Weight): P/W (活塞机)

### 16. 发动机性能分析

#### 16.1 推重比与功重比

| 可计算指标 | 计算方法 | 数据来源 |
|-----------|----------|----------|
| 推重比(T/W) | ThrustMax / (EmptyMass * g) | 推力, 空重 |
| 功重比(P/W) | Power / (EmptyMass * g) | 功率, 空重 |
| 起飞推重比 | (ThrustMax * n_engines) / (TakeoffMass * g) | 起飞重量 |

#### 16.2 发动机高度特性

| 可计算指标 | 计算方法 | 数据来源 |
|-----------|----------|----------|
| 静推力 | ThrustMax0 | 推力曲线 |
| 高度推力衰减 | ThrustMax(alt) | 推力矩阵 |
| 功率高度衰减 | Power(alt) | 增压器曲线 |

#### 16.3 燃油消耗分析

| 可计算指标 | 计算方法 | 数据来源 |
|-----------|----------|----------|
| 耗油率(SFC) | FuelFlow/Thrust | kg/(kgf*h) |
| 单位燃油消耗 | FuelFlow/Power | kg/(hp*h) |
| 加力油耗倍数 | AfterburnerBoost | 加力燃烧 |
| 怠速油耗 | FuelConsumptionOnIdle | 怠速油耗 |

### 17. 综合性能指标

#### 17.1 能量特性 (Energy Metrics)

| 指标 | 计算公式 | 说明 |
|------|----------|------|
| 动能 | E_k = 0.5*m*V² | 速度能量 |
| 势能 | E_p = m*g*h | 高度能量 |
| 总能量 | E = E_k + E_p | 飞机能量 |
| 能量率 | Ė = T*V - D*V | 能量变化率 |

#### 17.2 能量机动性 (Energy-Maneuverability)

| 指标 | 计算公式 | 说明 |
|------|----------|------|
| 特定能量残留 | P_s = (T-D)*V/m - g*sin(γ) | 能量机动性指标 |
| 能量交换率 | 爬升/俯冲时的能量变化 | 战术机动关键 |
| 盘旋能量维持 | 稳定盘旋所需的推力 | 能量守恒 |

#### 17.3 作战性能指数

| 指标 | 计算方法 | 用途 |
|------|----------|------|
| 综合性能评分 | 加权多维指标 | 飞机对比 |
| 能量效率 | E/MFuel | 续航能力 |
| 机动性指数 | (T/W) / (W/S) | 加速+盘旋 |
| 速度潜力 | Vmax - Vs | 高速性能 |

### 18. 时序与动态特性

#### 18.1 发动机响应

| 参数 | 说明 |
|------|------|
| TurbineTimeConstant | 涡轮加速时间常数 |
| AfterburnerDelay | 加力点燃延迟 |
| RPM响应 | 转速随油门变化 |

#### 18.2 飞控系统

| 参数 | 说明 |
|------|------|
| FlyByWire | 电传飞控模式 |
| HasManeuverabilityMode | 机动模式限制 |
| HasThrustVectoringMode | 推力矢量控制 |

## 参考资料

- voidmei项目: https://github.com/matrixsukhoi/voidmei
- 战争雷霆FM数据格式
