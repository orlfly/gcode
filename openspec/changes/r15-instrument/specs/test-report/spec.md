## Purpose

按 PRD §7.3.5 的 schema 输出可复算的测试报告，让 PASS/FAIL、偏差与归因有统一载体，供人阅读也供后续阶段消费。

## ADDED Requirements

### Requirement: TestReport IR
系统 SHALL 输出符合 PRD §7.3.5 结构的报告：`plan_id`、`items`（每项含 `step`、`target`、`expected{value,tol}`、`measured`、`unit`、`verdict`、可选 `analysis`）、`summary`。

#### Scenario: 结构完整
- **WHEN** 生成报告
- **THEN** 顶层含 plan_id、items、summary，且 items 每项含 step/target/expected/measured/unit/verdict

#### Scenario: PASS 项
- **WHEN** 某步实测在容差内
- **THEN** 该项 verdict 为 PASS，且不带 analysis 字段

#### Scenario: FAIL 项带归因
- **WHEN** 某步实测超容差
- **THEN** 该项 verdict 为 FAIL 且 analysis 给出归因文本

### Requirement: 汇总结论
系统 SHALL 在 summary 中给出通过数与总数，并能表达整体是否通过。

#### Scenario: 部分通过
- **WHEN** 3 项中 2 项 PASS
- **THEN** summary 体现出 2/3 与整体未通过

### Requirement: 数据来源标注
系统 SHALL 在报告中标注每组测量的来源（仪器后端/设备标识），并能区分虚拟、demo 与真实设备。

#### Scenario: 标注 demo 来源
- **WHEN** 数据来自 sigrok demo 设备
- **THEN** 报告中标注来源为 demo 设备

### Requirement: 人类可读渲染
系统 SHALL 能渲染报告为可读文本（中文），含逐项实测/预期/偏差/判定与归因。

#### Scenario: 渲染包含关键数据
- **WHEN** 渲染报告
- **THEN** 文本包含每个目标的实测值、预期值、偏差与判定

### Requirement: 可落盘且确定
系统 SHALL 支持将报告写入文件（JSON 与可读文本），且同一输入 MUST 产生字节级一致的输出。

#### Scenario: 确定性
- **WHEN** 用相同输入生成两次
- **THEN** 两次内容完全相同
