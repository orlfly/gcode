## Purpose

把已验证的网表 IR 确定性地翻译成可交给 ngspice 的 SPICE 网表，含节点归一化、器件卡映射、器件模型注入与分析卡，使仿真不依赖任何手工编辑。

## ADDED Requirements

### Requirement: 从网表 IR 确定性生成 SPICE 网表
系统 SHALL 从 `NetlistIr` 生成 SPICE 网表，且同一 IR + 同一分析配置 MUST 生成字节级一致的内容。

#### Scenario: 可复现
- **WHEN** 以相同 IR 与分析配置生成两次
- **THEN** 两次网表内容完全相同

#### Scenario: 无源件映射
- **WHEN** IR 含 `Device:R` / `Device:C` / `Device:L`
- **THEN** 分别生成 `R` / `C` / `L` 器件卡，值为 IR 中的 value

### Requirement: 接地与节点归一化
系统 SHALL 把接地网络映射为 SPICE 节点 `0`，并对其余网络名做 SPICE 安全化处理（去除非字母数字下划线字符），且 MUST 保证不同网络归一化后不碰撞。

#### Scenario: GND 映射为 0
- **WHEN** IR 含名为 `GND` 的网络
- **THEN** 网表中该网络出现为节点 `0`

#### Scenario: 节点名碰撞被拒绝
- **WHEN** 两个不同网络名归一化后得到相同节点名
- **THEN** 生成失败并报告冲突的网络名

### Requirement: 器件模型注册与注入
系统 SHALL 为已知有源器件注入 SPICE 模型定义（行为级 macromodel），并在输出中标注模型来源为行为级；遇到无模型的器件时 MUST 报错而非静默输出无效卡。

#### Scenario: 有源件注入模型
- **WHEN** IR 含 `Amplifier_Operational:LM321`
- **THEN** 网表包含对应的子电路定义且实例引用该子电路

#### Scenario: 未知器件被拒绝
- **WHEN** IR 含一个注册表里没有模型的器件
- **THEN** 生成失败并指出该 `lib_id`

### Requirement: 分析卡生成
系统 SHALL 按分析配置生成对应的分析卡：直流工作点、AC 扫频（含扫描类型与点数）、瞬态（含步长与时长）。

#### Scenario: AC 扫频
- **WHEN** 配置为 AC 分析、十倍频程、起始/终止频率
- **THEN** 网表包含对应的 `.ac` 卡

#### Scenario: 瞬态
- **WHEN** 配置为瞬态分析、步长与时长
- **THEN** 网表包含对应的 `.tran` 卡

### Requirement: 与 KiCad 导出交叉校验
系统 SHALL 提供与 `kicad-cli sch export netlist --format spice` 输出在**无源件拓扑**上的一致性校验（节点集合与器件连接），以证明原理图确实表达了同一电路。

#### Scenario: 拓扑一致
- **WHEN** 对仅含无源件的电路同时用本系统与 kicad-cli 导出
- **THEN** 两者的器件连接关系一致（允许节点命名与卡序差异）
