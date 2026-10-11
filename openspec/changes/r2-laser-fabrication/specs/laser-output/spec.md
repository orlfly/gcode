## Purpose

输出能直接交给 GRBL 控制器的 G-code 和让人一眼看懂走刀的 SVG 预览，两者都确定、可归档、可复查。

## ADDED Requirements

### Requirement: GRBL 兼容的 G-code
系统 SHALL 输出仅使用 GRBL 稳定子集（`G21`/`G90`/`G94`、`M3`/`M4` 与 `S`、`G0`、`G1`、`M5`），并包含可识别的前言与收尾。

#### Scenario: 前言与收尾
- **WHEN** 生成 G-code
- **THEN** 文件以单位/模式设置与激光开启指令开头，以关激光与回零/结束指令收尾

#### Scenario: 不引入非 GRBL 指令
- **WHEN** 检查产物
- **THEN** 不包含圆弧/刀具补偿/主轴等未声明使用的指令

### Requirement: 功率与速度可追溯
系统 SHALL 在 G-code 中体现每层的功率与进给，并以注释标明参数来源（板材与推荐来源）。

#### Scenario: 参数进注释
- **WHEN** 使用参数知识库的推荐值
- **THEN** G-code 注释里包含板材、功率、速度、遍数与「需试刻验证」提示

### Requirement: 空程与加工区分
系统 SHALL 用 `G0` 表示空程、`G1` 表示加工，且 MUST 在每条加工段前保证激光状态正确。

#### Scenario: 空程不切劊
- **WHEN** 走刀从一个轮廓跳到另一个
- **THEN** 使用 `G0` 且激光处于关闭状态

### Requirement: 确定性输出
系统 SHALL 对同一作业产出字节级一致的 G-code 与 SVG。

#### Scenario: 可复现
- **WHEN** 同一作业生成两次
- **THEN** 两次 G-code 与 SVG 内容完全相同

### Requirement: SVG 走刀预览
系统 SHALL 产出分层的 SVG 预览（隔离雕刻/钻孔/板框各自颜色），含板框范围与比例信息，不依赖外部库。

#### Scenario: 分层着色
- **WHEN** 生成预览
- **THEN** SVG 中三类走刀有不同颜色且有图例

#### Scenario: 坐标正确
- **WHEN** 作业有归一后的坐标
- **THEN** SVG 的几何与作业坐标一致（同比例、同方向）
