## Purpose

把实测值与仿真/用户基准放在一起算出偏差，并按 PRD 的 KR2 阈值给出可解释判定，使“仿真可信吗”这个问题有可复算的答案。

## ADDED Requirements

### Requirement: 偏差计算
系统 SHALL 对每个可比较项计算实测值、基准值、绝对偏差与百分比偏差，并在基准值为 0 时以绝对偏差表达。

#### Scenario: 百分比偏差
- **WHEN** 基准 3.30V、实测 3.31V
- **THEN** 输出绝对偏差 0.01V 与百分比偏差约 +0.30%

#### Scenario: 基准为零
- **WHEN** 基准值为 0
- **THEN** 输出绝对偏差且不输出荒谬的百分比

### Requirement: KR2 阈值判定
系统 SHALL 按 PRD 的 KR2 阈值判定：直流工作点相对偏差 ≤ 10%，AC 峰值频率相对偏差 ≤ 20%；超阈判为 FAIL 并给出偏差。

#### Scenario: 直流在阈内
- **WHEN** 直流工作点偏差 6%
- **THEN** 判为 PASS

#### Scenario: 直流超阈
- **WHEN** 直流工作点偏差 15%
- **THEN** 判为 FAIL 并输出偏差百分比

#### Scenario: AC 频率超阈
- **WHEN** AC 峰值频率偏差 25%
- **THEN** 判为 FAIL（阈值为 20%）

### Requirement: 阈值可显式覆盖
系统 SHALL 允许调用方传入自定义容差，且 MUST 在输出中同时注明使用的是默认 KR2 阈值还是覆盖值。

#### Scenario: 自定义容差
- **WHEN** 调用方指定直流容差 5%
- **THEN** 按 5% 判定并在输出中标明为覆盖值

### Requirement: FAIL 归因
系统 SHALL 在 FAIL 时给出至少一条可能原因与排查建议（器件容差、焊接/接触、拓扑与设计不符、仿真模型偏差、测量条件不符）。

#### Scenario: 给出归因
- **WHEN** 某项 FAIL
- **THEN** 输出包含该差异的可能原因与建议动作

### Requirement: 不可比较项不伪造结论
系统 SHALL 对缺失基准或缺失实测的项标记为不可比，且 MUST 不计入 PASS。

#### Scenario: 缺基准
- **WHEN** 某步没有基准值
- **THEN** 标记为不可比且不计为 PASS
