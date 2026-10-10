## Purpose

用 `kicad-cli` 对生成的原理图执行电气规则检查，把机器报告翻译成可读结论与修复建议，并驱动「生成→检查→修正→重生成」的自查循环。

## ADDED Requirements

### Requirement: headless 运行 ERC
系统 SHALL 通过 `kicad-cli sch erc` 以 headless 方式对 `.kicad_sch` 运行 ERC，并支持通过环境变量 `GCODE_KICAD_CLI` 覆盖可执行文件路径。

#### Scenario: 正常执行
- **WHEN** 对合法原理图运行 ERC 且 kicad-cli 可用
- **THEN** 系统返回结构化 ERC 报告

#### Scenario: 缺少 kicad-cli
- **WHEN** 环境未安装 kicad-cli 且未设置 GCODE_KICAD_CLI
- **THEN** 系统返回可操作的错误（提示安装或设置变量）且不 panic

### Requirement: 解析并汇总 ERC 报告
系统 SHALL 解析 kicad-cli 的 JSON 报告，区分 error 与 warning，并统计违规数、未连接项与原理图一致性项。

#### Scenario: 汇总计数
- **WHEN** 报告含 1 个 error、1 个 warning 与 1 个未连接项
- **THEN** 汇总显示 error 数为 1 且判定为未通过

#### Scenario: 干净报告
- **WHEN** 报告无任何违规与未连接项
- **THEN** 判定为 PASS 并输出 "ERC: PASS"

### Requirement: 违规的解读与修复建议
系统 SHALL 为每类 ERC 违规给出中文说明与至少一条可执行的修复建议，并指出涉及的元件或网络。

#### Scenario: 给出修复建议
- **WHEN** ERC 报告包含一类已知违规
- **THEN** 输出包含该违规的中文解释与针对具体元件/网络的修复建议

#### Scenario: 未知违规类型兜底
- **WHEN** 违规类型不在已知映射表中
- **THEN** 输出仍包含原始描述与通用排查建议，不丢失信息

### Requirement: 自查循环
系统 SHALL 支持「生成→ERC→修正→重生成」循环，并在达到最大尝试次数后停止并报告未解决的违规。

#### Scenario: 一轮修正后通过
- **WHEN** 首次 ERC 失败且修正后重生成通过
- **THEN** 循环以 PASS 结束并报告所用轮数

#### Scenario: 达到尝试上限
- **WHEN** 连续多轮仍存在 error 违规
- **THEN** 循环在预设上限处停止并列出残留违规
