## Purpose

把仿真结果变成用户能直接看的文件：机器可读的波形数据与无需外部依赖的矢量图，供人在 KiCad 之外查看与归档。

## ADDED Requirements

### Requirement: 波形数据落盘
系统 SHALL 把取回的波形写成 CSV（首行为表头，列为自变量与各探针），且同一仿真结果 MUST 产生字节级一致的 CSV。

#### Scenario: CSV 可读
- **WHEN** 写入波形 CSV
- **THEN** 首行是列名、后续行为数值，可被常见工具直接读取

### Requirement: 图形产物无外部依赖
系统 SHALL 生成 SVG 图（波特图：幅频与相频；瞬态/直流：时域或扫描曲线），且 MUST 不依赖外部绘图库或网络。

#### Scenario: 波特图
- **WHEN** 对 AC 结果生成图
- **THEN** 产出 SVG 文件，含坐标轴、网格与两条曲线（幅值/相位）

#### Scenario: 确定性
- **WHEN** 用相同数据生成两次
- **THEN** 两次 SVG 内容完全相同

### Requirement: 文件名可预测
系统 SHALL 以可预测的命名规则输出产物（含电路标识与分析类型），并在覆盖前不静默丢失已有产物。

#### Scenario: 命名可预测
- **WHEN** 对电路 `rc-lowpass` 的 AC 分析生成产物
- **THEN** 产物文件名包含该电路标识与 `ac`
