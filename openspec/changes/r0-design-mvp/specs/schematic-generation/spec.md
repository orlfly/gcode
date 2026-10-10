## Purpose

把通过校验的 NetlistIr 确定性地转换为可被 KiCad 打开、可被本地解析器回读的 `.kicad_sch` 原理图文件。

## ADDED Requirements

### Requirement: 确定性生成 KiCad 原理图
系统 SHALL 从 NetlistIr 生成 `.kicad_sch` 文件，文件以 `kicad_sch` 为根节点且包含 KiCad 10 可识别的版本与生成器信息；同一 IR 与布局参数 MUST 生成字节级一致的内容。

#### Scenario: 生成合法文件
- **WHEN** 对合法分压器 IR 调用生成
- **THEN** 写出的文件以 `(kicad_sch` 开头且符合 KiCad 10 语法

#### Scenario: 输出可复现
- **WHEN** 以相同 IR 与布局参数生成两次
- **THEN** 两次文件内容完全一致

### Requirement: 组件、引脚与网络完整映射
系统 SHALL 为每个组件写出符号实例（含 `lib_id`、`value`、可选 `footprint`）与其引脚，并为每个网络写出电气连接（导线与/或标签），使 ERC 能识别全部连接。

#### Scenario: 网络连接可识别
- **WHEN** 生成含 VOUT 网络的原理图
- **THEN** VOUT 所连各引脚在文件中属于同一网络

#### Scenario: 未连接引脚暴露
- **WHEN** 某组件引脚未出现在任何网络中
- **THEN** 该引脚在原理图中保持未连接，供 ERC 报告

### Requirement: 布局确定性且不重叠
系统 SHALL 依据布局参数（原点、间距）确定性地放置组件，使同一 IR 的布局可复现且组件符号互不重叠。

#### Scenario: 布局可复现
- **WHEN** 以相同布局参数生成两次
- **THEN** 两次组件坐标完全一致

### Requirement: 回读保真
系统 SHALL 保证生成的原理图可被本项目的 s-expression 解析器回读为等价的组件与网络集合。

#### Scenario: 回读一致
- **WHEN** 生成后再解析该文件
- **THEN** 解析得到的组件位号集合与网络名集合与输入 IR 一致
