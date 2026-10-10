## Purpose

把仿真波形变成工程结论：提取可比较的指标，与预期/容差比对给出 PASS/FAIL，并用中文解释偏差与下一步排查方向。

## ADDED Requirements

### Requirement: 工程指标提取
系统 SHALL 从波形中提取至少以下指标（按分析类型适用）：直流节点电压、低频/通带增益、-3dB 截止频率、相位裕度、峰值与峰峰值、上升时间、纹波。

#### Scenario: 低通截止频率
- **WHEN** 对 RC 低通（R=1k, C=100n）运行 AC 分析
- **THEN** 提取的 -3dB 截止频率在 1.59 kHz 的 5% 内

#### Scenario: 放大器增益
- **WHEN** 对同相放大器（增益 10.1）运行低频 AC 或直流分析
- **THEN** 提取的增益在预期值的 10% 内

### Requirement: 与预期比对并判定
系统 SHALL 接受预期值与其容差，对每个指标给出 PASS/FAIL 与实测值/预期值/偏差，并汇总整体判定。

#### Scenario: 在容差内
- **WHEN** 实测值在容差内
- **THEN** 该项判为 PASS

#### Scenario: 超出容差
- **WHEN** 实测值超出容差
- **THEN** 该项判为 FAIL，输出实测值、预期值、偏差百分比与绝对偏差

### Requirement: 中文解读与偏差归因
系统 SHALL 为每项结果给出中文解释，并在 FAIL 时给出至少一条可能原因与排查建议（例如元件容差、模型精度、拓朴接错）。

#### Scenario: FAIL 给归因
- **WHEN** 某项 FAIL
- **THEN** 输出包含该指标的物理含义、可能原因与建议动作

### Requirement: 不自己下结论
系统 SHALL 的 PASS/FAIL 必须来自仿真数据与预期值的比较，MUST 不由语言模型主观判定。

#### Scenario: 无仿真数据不判定
- **WHEN** 仿真未成功或未返回数据
- **THEN** 不给出 PASS/FAIL，而是报告仿真失败
