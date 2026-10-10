## Purpose

提供 `gcode` CLI 运行时，隔离全部持久化数据，注册 R0 电路工具，并约束 agent 必须经由网表 IR 工具而非直接书写 s-expression 来产出设计。

## ADDED Requirements

### Requirement: 隔离运行时
系统 SHALL 把配置、会话、日志与临时文件保存在 `~/.gcode`（可经 `GCODE_HOME` 覆盖），且 MUST 不读写 jcode 自身的用户数据目录。

#### Scenario: 默认隔离
- **WHEN** 以默认环境启动 gcode
- **THEN** 所有持久化写入发生在 `~/.gcode` 下

#### Scenario: 自定义 home
- **WHEN** 以 `GCODE_HOME=/x/y` 启动
- **THEN** 所有持久化写入发生在 `/x/y` 下

### Requirement: 工具注册
系统 SHALL 向 agent 运行时注册 R0 电路工具，至少包含原理图生成与 ERC 检查，且每个工具 MUST 通过 JSON schema 描述其输入。

#### Scenario: 工具可用
- **WHEN** agent 运行时启动
- **THEN** 原理图生成与 ERC 检查工具已注册且可被调用

#### Scenario: 参数校验
- **WHEN** 以缺失必填参数调用工具
- **THEN** 返回指明缺失参数的错误而非崩溃

### Requirement: 提示词契约
系统 SHALL 通过系统提示词要求 agent 按「理解→网表→验证→报告」顺序工作，并 MUST 禁止 agent 直接书写 KiCad s-expression 文件。

#### Scenario: 顺序与约束存在
- **WHEN** 启动 agent 会话
- **THEN** 系统提示词包含网表 IR 契约与「不得直接写 s-expression」的约束

### Requirement: 工具链探测与版本门控
系统 SHALL 探测 `kicad-cli` 并报告其版本，在版本不满足 KiCad 10 最低要求时给出明确提示且拒绝执行 ERC。

#### Scenario: 探测到可用工具链
- **WHEN** `kicad-cli` 可用且版本满足要求
- **THEN** 报告版本并允许执行 ERC

#### Scenario: 版本不满足
- **WHEN** `kicad-cli` 版本低于最低要求
- **THEN** 给出明确提示且不执行 ERC
