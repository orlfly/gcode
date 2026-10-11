## Purpose

把 KiCad 导出的 Gerber（RS-274X）与 Excellon 钻孔读成结构化图形数据，并对读不慬的部分明确报告，而不是静默丢图形。

## ADDED Requirements

### Requirement: Gerber 格式声明与单位
系统 SHALL 解析 `FS`（坐标格式与零省略策略）与 `MO`（单位），并把后续坐标正确换算为毫米。

#### Scenario: 解析整型坐标
- **WHEN** 文件声明 `FSLAX46Y46` 且坐标写作 `X141605000Y-99695000`
- **THEN** 解析为 (141.605, -99.695) 毫米

#### Scenario: 缺少单位声明
- **WHEN** 文件没有 `MO` 声明
- **THEN** 报错并说明缺少单位声明，而不是假设一个单位

### Requirement: 孔径与绘制操作
系统 SHALL 解析 `AD` 孔径定义（至少 C 圆、R 矩形、O 椭圆）与 `D01/D02/D03` 操作，产出 flash（单点图形）与 draw（带孔径的线段）两类图元。

#### Scenario: flash 与 draw 区分
- **WHEN** 文件含 `D03` 闪光与 `D02`→`D01` 绘制
- **THEN** 产出对应的 flash 与 draw 图元，且 draw 带起点、终点与所用孔径

#### Scenario: 未知孔径
- **WHEN** 图元引用了未定义的孔径号
- **THEN** 报错并指出该孔径号

### Requirement: 区域与极性
系统 SHALL 支持 `G36/G37` 区域填充（转多边形状）与 `LP` 极性声明，并至少正确处理正极性（`LPD`）。

#### Scenario: 区域转多边形
- **WHEN** 文件含 `G36` … `G37` 路径
- **THEN** 该段产出多边形状图元

#### Scenario: 负极性明示
- **WHEN** 文件使用 `LPC`（负极性）
- **THEN** 系统明确报告该层含负极性图形及其可能影响

### Requirement: 不支持构造要明示
系统 SHALL 统计并报告不支持的构造（如宏孔径 `AM`、圆弧 `G02/G03`、步进重复 `SR`），MUST 不静默丢弃。

#### Scenario: 圆弧被计入报告
- **WHEN** 文件含 `G02`/`G03`
- **THEN** 解析结果的报告里包含圆弧计数与提示

#### Scenario: 报告可查阅
- **WHEN** 解析完成
- **THEN** 返回结构化的解析报告（图元计数、层类型、未支持构造列表）

### Requirement: 识别层类型
系统 SHALL 从 `TF.FileFunction` 等属性识别层类型（铜层/板框/其它），供后续选择隔离铜层与板框。

#### Scenario: 识别铜层与板框
- **WHEN** 解析 KiCad 导出的 `*-top_cu.gtl` 与 `*-Edge_Cuts.gm1`
- **THEN** 分别识别为铜层（Copper）与板框（Profile）

### Requirement: Excellon 钻孔解析
系统 SHALL 解析 Excellon 钻孔文件（头部的刀具表 `T#C<直径>`、单位、`T#` 选刀与坐标），产出孔位列表（含直径与镀铜标记）。

#### Scenario: 解析孔位
- **WHEN** 解析一个真实钻孔文件
- **THEN** 返回孔位列表，每个孔含坐标与对应刀具直径

#### Scenario: 工具未选择
- **WHEN** 遇到坐标但没有有效的刀具选择
- **THEN** 报错并指出该行位置
