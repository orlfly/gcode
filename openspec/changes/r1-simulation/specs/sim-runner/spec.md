## Purpose

以 ngspice 为后端执行直流/AC/瞬态仿真，把结果以机器可读形式取回，并把失败（缺工具链、不收敛、语法错误）变成可操作的结构化信息。

## ADDED Requirements

### Requirement: 工具链探测与版本门控
系统 SHALL 探测 ngspice 可执行文件（`GCODE_NGSPICE` 覆盖 → `PATH`），解析其版本，并在不可用或版本低于要求时返回明确错误。

#### Scenario: 正常探测
- **WHEN** ngspice 可用且版本满足要求
- **THEN** 返回可执行路径与版本字符串

#### Scenario: 未安装
- **WHEN** ngspice 不在 PATH 且未设置 `GCODE_NGSPICE`
- **THEN** 返回可操作的错误（说明如何提供 ngspice）且不 panic

### Requirement: 执行仿真并取回波形
系统 SHALL 以批处理模式运行 ngspice，并把每个探针的波形取回为机器可读的数据（自变量 + 各因变量序列），而不是仅解析屏幕文本。

#### Scenario: AC 分析取回
- **WHEN** 对 RC 低通运行 AC 分析并指定探针
- **THEN** 返回频率与探针幅值/相位序列，长度等于扫描点数

#### Scenario: 直流工作点取回
- **WHEN** 运行直流工作点分析
- **THEN** 返回指定节点的直流电压值

#### Scenario: 瞬态取回
- **WHEN** 运行瞬态分析
- **THEN** 返回时间序列与探针电压序列

### Requirement: 失败可诊断
系统 SHALL 在 ngspice 报错或不收敛时保留原始输出并给出结构化错误，MUST 不把失败当作空结果静默返回。

#### Scenario: 不收敛
- **WHEN** 仿真不收敛
- **THEN** 返回失败状态并包含 ngspice 原始错误信息

#### Scenario: 网表语法错
- **WHEN** ngspice 报告网表语法错误
- **THEN** 返回失败状态并包含出错行或原文片段
