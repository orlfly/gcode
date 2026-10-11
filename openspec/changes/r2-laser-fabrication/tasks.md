# R2 制造 — 实施任务

> 状态标记：`[x]` 已完成并有验证；`[ ]` 未完成/阻塞（原因见行内说明）。

## 1. crate 骨架

- [ ] 1.1 新建 `crates/gcode-laser`，加入 workspace members 与 workspace.dependencies
- [ ] 1.2 依赖：`gcode-kicad`、`serde`、`serde_json`、`thiserror`；dev: 无重依赖
- [ ] 1.3 `lib.rs` 导出模块（gerber / excellon / geometry / toolpath / gcode / svg / profile / job）

## 2. Gerber 与钻孔解析（spec: gerber-import）

- [ ] 2.1 `FS`/`MO` 解析与坐标换算（LA 前后零省略；MM/IN）
- [ ] 2.2 孔径 `AD`（C/R/O）与 `D01/D02/D03` → flash / draw 图元
- [ ] 2.3 `G36/G37` 区域 → 多边形状图元
- [ ] 2.4 `LP` 极性：识别 `D`，`C` 计入未支持报告
- [ ] 2.5 未支持构造计数（`G02/G03`、`AM`、`SR`、`LM/LR/LS`）并输出结构化解析报告
- [ ] 2.6 `TF.FileFunction` 层类型识别（Copper / Profile / 其它）
- [ ] 2.7 Excellon 解析：刀具表、单位、选刀、坐标 → 孔位（含直径）
- [ ] 2.8 对真实 KiCad 导出的 Gerber/钻孔解析测试（含未支持构造报告断言）

## 3. 铜几何（spec: copper-geometry）

- [ ] 3.1 占用栅格：圆/矩/椭圆 flash、不同孔径的 draw（含端点圆帽）、区域多边形
- [ ] 3.2 确定性：同输入同分辨率两次栅格化结果一致
- [ ] 3.3 按半径膨胀（安全侧加半个像素）
- [ ] 3.4 边界跟踪提取闭合轮廓
- [ ] 3.5 Douglas-Peucker 简化（向安全侧取整），点数下降且闭合性保持
- [ ] 3.6 最小距离校验：路径到铜的最近距离 ≥ 请求安全间距
- [ ] 3.7 最近邻串联路径，结果确定

## 4. 激光作业（spec: laser-job）

- [ ] 4.1 作业结构：三类走刀（隔离/钻孔/板框）+ 每层参数 + 坐标归一变换
- [ ] 4.2 坐标归一：平移到非负，记录平移量与 Y 约定
- [ ] 4.3 隔离多遍 + 重叠；板框多遍
- [ ] 4.4 钻孔 Mark / Circle 两种模式
- [ ] 4.5 缺输入（无钻孔/无板框）时仍可生成并报告缺项影响
- [ ] 4.6 作业统计：各类段数、总长、预估时间、包围盒；可复现

## 5. G-code 与预览（spec: laser-output）

- [ ] 5.1 GRBL 子集输出：前言/`G0`/`M3 S`/`G1 F`/`M5`/收尾，指令白名单
- [ ] 5.2 注释中标注板材、功率、速度、遍数与来源（知识库/用户覆盖）+ 试刻提示
- [ ] 5.3 空程用 `G0` 且激光关闭
- [ ] 5.4 确定性输出（同作业两次生成字节级一致）
- [ ] 5.5 SVG 分层预览（三类不同颜色 + 图例 + 比例），确定性
- [ ] 5.6 G-code 结构校验：指令白名单、坐标有限且落在板内范围

## 6. 参数知识库（spec: laser-profile）

- [ ] 6.1 `LaserProfile` 结构与若干常见板材条目（含 source 与适用范围）
- [ ] 6.2 查询：命中/未命中（未命中给 nearest 参考并明确未收录）
- [ ] 6.3 强制免责：起点、需试刻、与机器相关
- [ ] 6.4 用户覆盖：功率/速度/遍数可覆盖，来源在作业与 G-code 中标注

## 7. 工具与提示词（spec: laser-workflow）

- [ ] 7.1 工具：解析 Gerber/钻孔并报告
- [ ] 7.2 工具：生成激光作业（含统计）并导出 G-code 与 SVG
- [ ] 7.3 工具：查询/覆盖激光参数
- [ ] 7.4 注册进 `gcode_tools::all_tools()`，schema 完整
- [ ] 7.5 提示词：制造工作流 + **不得声称已实际打样**（只能声明已生成产物并做几何校验）
- [ ] 7.6 工具参数校验测试（缺参、未知路径、无板框等）
- [ ] 7.7 未支持构造报告透传到工具输出

## 8. 收尾

- [ ] 8.1 全量 `cargo test --workspace` 通过（R0/R1/R1.5 不回归）
- [ ] 8.2 验收 example：用真实 demo 板 Gerber 跑完整链路，落盘 G-code / SVG / 报告
- [ ] 8.3 `docs/PRD-gcode.md` 附五：R2 状态、已验证范围与「无硬件所以未打样」
- [ ] 8.4 `openspec validate r2-laser-fabrication --strict` 通过
