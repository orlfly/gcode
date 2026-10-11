## Purpose

把制造能力接进 agent：解析 Gerber、生成作业、导出 G-code 与预览、查询参数，并在提示词层面禁止声称已实际打样。

## ADDED Requirements

### Requirement: 制造工具注册
系统 SHALL 向 agent 注册制造相关工具，至少包含：解析 Gerber/钻孔并报告、生成激光作业与统计、导出 G-code 与 SVG 预览、查询/覆盖激光参数；每个工具 MUST 用 JSON schema 描述输入。

#### Scenario: 工具可用
- **WHEN** agent 运行时启动
- **THEN** 制造工具已注册且可被调用

#### Scenario: 参数校验
- **WHEN** 以缺失必填参数调用制造工具
- **THEN** 返回指明缺失参数的错误而非崩溃

### Requirement: 不得声称已打样
系统 SHALL 在系统提示词与工具输出中要求：只能声明「已生成可上机的 G-code 并做了几何校验」，MUST 禁止声称已实际雕刻/打样成功（除非用户提供实测证据）。

#### Scenario: 提示词约束存在
- **WHEN** 启动 agent 会话
- **THEN** 系统提示词包含「不得声称已实际打样」与制造工具的用法

### Requirement: 未支持构造必须报告
系统 SHALL 把解析器的未支持构造报告透传到工具输出，使 agent 不能默默忽略图纸内容。

#### Scenario: 报告透传
- **WHEN** Gerber 含未支持构造
- **THEN** 工具输出包含该构造的计数与提示

### Requirement: 产物路径可追溯
系统 SHALL 在工具输出中包含 G-code 与 SVG 的文件路径与关键作业统计，便于用户直接上机与复查。

#### Scenario: 输出含路径与统计
- **WHEN** 生成制造产物
- **THEN** 输出包含 G-code 路径、SVG 路径与作业统计

### Requirement: 无硬件时降级
系统 SHALL 在没有任何激光/GRBL 设备时仍能完整生成产物与校验，且 MUST 不因缺少设备而报错。

#### Scenario: 无设备仍可生成
- **WHEN** 本机没有激光设备
- **THEN** 仍可生成 G-code 与预览并给出几何校验结果
