## Purpose

让所有电路修改以可审阅的 diff 呈现、经用户确认后写回，并尽可能以增量方式编辑既有原理图，避免整体重写。

## ADDED Requirements

### Requirement: 修改以 diff 呈现
系统 SHALL 对每次修改生成结构化 diff，标示新增/删除的组件、变更的取值，以及新增/删除/改接的网络连接。

#### Scenario: 修改元件取值
- **WHEN** 用户要求把 R2 从 10k 改为 4.7k
- **THEN** diff 显示 R2 取值由 10k 变为 4.7k 且其余组件不变

#### Scenario: 改接网络
- **WHEN** 用户要求把某引脚从 GND 改接到 VCC
- **THEN** diff 显示对应的连接删除与新增

### Requirement: 确认后写回
系统 SHALL 仅在用户明确确认后把变更写入文件；未确认时 MUST 不修改磁盘上的原理图。

#### Scenario: 未确认不落盘
- **WHEN** 生成 diff 后用户未确认
- **THEN** 原文件内容保持不变

#### Scenario: 确认后写入
- **WHEN** 用户确认 diff
- **THEN** 变更写入文件且文件内容与 diff 一致

### Requirement: 增量编辑
系统 SHALL 在修改既有原理图时保留未被改动元素的标识，使未修改部分不因重写而变化。

#### Scenario: 未改动元素稳定
- **WHEN** 仅修改单个组件的取值
- **THEN** 其余组件的标识与网络连接保持不变

### Requirement: 可回退
系统 SHALL 允许在写入前放弃变更，并在写入后可将文件恢复为变更前内容。

#### Scenario: 写入后回退
- **WHEN** 用户对已应用的变更执行回退
- **THEN** 文件恢复为变更前内容
