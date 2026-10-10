## Purpose

把仿真能力接进 agent：对话式选择分析、跑仿真、拿解读，并在提示词层面约束「结论必须来自仿真结果」而非模型自述。

## ADDED Requirements

### Requirement: 仿真工具注册
系统 SHALL 向 agent 运行时注册仿真相关工具，至少包含：列出电路可用的仿真配方、执行仿真、取回解读结果；每个工具 MUST 用 JSON schema 描述输入。

#### Scenario: 工具可用
- **WHEN** agent 运行时启动
- **THEN** 仿真工具已注册且可被调用

#### Scenario: 参数校验
- **WHEN** 以缺失必填参数调用仿真工具
- **THEN** 返回指明缺失参数的错误而非崩溃

### Requirement: 仿真结论必须来自仿真
系统 SHALL 在系统提示词中要求：增益/截止频率/工作点等数值结论必须引用仿真工具的输出，MUST 禁止仅凭计算或经验给出「已验证」的结论。

#### Scenario: 提示词约束存在
- **WHEN** 启动 agent 会话
- **THEN** 系统提示词包含「数值结论必须来自仿真结果」与仿真工具的用法

### Requirement: 仿真结果可追溯
系统 SHALL 在工具输出中包含本次仿真所用的分析类型、探针与产物文件路径，使用户能复查原始数据。

#### Scenario: 输出含产物路径
- **WHEN** 仿真工具返回结果
- **THEN** 输出包含波形/图产物路径与分析配置

### Requirement: 工具链不可用时降级不报错
系统 SHALL 在 ngspice 不可用时给出可操作提示，且 MUST 不影响 R0 已有的生成与 ERC 功能。

#### Scenario: 无 ngspice
- **WHEN** ngspice 不可用且用户请求仿真
- **THEN** 返回安装/配置提示，生成与 ERC 工具仍可正常使用
