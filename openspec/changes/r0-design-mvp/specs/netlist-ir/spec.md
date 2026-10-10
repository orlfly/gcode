## Purpose

定义 LLM 与确定性代码之间唯一的电路契约（NetlistIr），使自然语言产出的电路意图可被机器校验，并在生成 KiCad 文件前拦截结构性错误。

## ADDED Requirements

### Requirement: 组件与网络的结构化表示
系统 SHALL 接受以 JSON 表达的网表 IR：`components`（每项含唯一 `reference`、`lib_id`、`value`，可选 `footprint`）与 `nets`（每项含 `name` 与 `connections`，连接为 `[reference, pin]` 对）。

#### Scenario: 接受合法网表
- **WHEN** 提供含 R1/R2 两个电阻与 VIN/VOUT/GND 三个网络的 IR
- **THEN** 系统接受该 IR 并通过校验

#### Scenario: 缺少必填字段
- **WHEN** 某组件缺少 `lib_id` 或某网络缺少 `name`
- **THEN** 反序列化失败并返回指明缺失字段的错误

### Requirement: 生成前强制结构校验
系统 SHALL 在任何文件写入前校验 IR；校验失败时 MUST 不产生或修改任何 `.kicad_sch` 文件，并返回可定位到具体组件或网络的错误。

#### Scenario: 重复位号被拒
- **WHEN** 两个组件的 `reference` 同为 "R1"
- **THEN** 校验失败并报告重复位号

#### Scenario: 网络引用未知组件被拒
- **WHEN** 某网络连接引用了不存在的组件 "Q9"
- **THEN** 校验失败并指出网络名与未知组件

#### Scenario: 空网络被拒
- **WHEN** 某网络的 `connections` 为空
- **THEN** 校验失败并报告该网络无连接

#### Scenario: 校验失败不落盘
- **WHEN** 对无效 IR 调用生成
- **THEN** 目标路径不存在任何新建或被修改的文件

### Requirement: 确定性输出
系统 SHALL 对等价 IR 产出字节级一致的序列化结果，且结果与输入顺序无关。

#### Scenario: 顺序无关
- **WHEN** 以不同的组件与连接顺序构造等价的 IR
- **THEN** 两次序列化结果完全相同

### Requirement: 内置拓扑模板
系统 SHALL 为常见拓扑提供可直接实例化的模板，且模板产出的 IR 必须通过结构校验。

#### Scenario: 分压模板
- **WHEN** 以 R1/R2 取值与 VIN/VOUT/GND 名称实例化分压器模板
- **THEN** 得到含两个组件、三个网络的合法 IR
