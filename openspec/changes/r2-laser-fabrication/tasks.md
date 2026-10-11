# R2 制造 — 实施任务

> 状态标记：`[x]` 已完成并有验证；`[ ]` 未完成/阻塞（原因见行内说明）。
> 验证方式列在每项后面：`测试:` 指 `cargo test`，`真实:` 指对 kicad-cli 导出的真实文件验证。

## 1. crate 骨架

- [x] 1.1 新建 `crates/gcode-laser`，加入 workspace members 与 workspace.dependencies
- [x] 1.2 依赖：`serde`、`serde_json`、`thiserror`（**不含** gcode-kicad：本 crate 不需要 IR，
      电路标识/文档链接由 `gcode-tools` 层负责，保持依赖最小）
- [x] 1.3 `lib.rs` 导出模块（gerber / excellon / geometry / toolpath / gcode / svg / profile）

## 2. Gerber 与钻孔解析（spec: gerber-import）

- [x] 2.1 `FS`/`MO` 解析与坐标换算（LA 前后零省略；MM/IN）— 测试: `parses_coordinates_*`、`inches_are_converted`
- [x] 2.2 孔径 `AD`（C/R/O）与 `D01/D02/D03` → flash / draw 图元
- [x] 2.3 `G36/G37` 区域 → 多边形状图元
- [x] 2.4 `LP` 极性：识别 `D`，`C` 计入未支持报告
- [x] 2.5 未支持构造计数（`SR`、`LM/LR/LS`、无法构造的宏图元、被跳过图元）并输出结构化解析报告
- [x] 2.6 `TF.FileFunction` 层类型识别（Copper / Profile / 其它），并保留原始 function 串用于选层
- [x] 2.7 Excellon 解析：刀具表、单位、选刀、坐标 → 孔位（含直径与镀铜标记）
- [x] 2.8 对真实 KiCad 导出的 Gerber/钻孔解析测试（含未支持构造报告断言）— 真实: ecc83-pp / stickhub
- [x] 2.9 （实测新增）宏孔径解释器：表达式求值 + 图元 1/4/20/21/22 + 旋转，
      KiCad `RoundRect` 焊盘可真实画出 — 测试: `kicad_roundrect_macro_*`、面积核对
- [x] 2.10 （实测新增）`G02/G03` 按模态插补模式线性化（弦高 ≤ 0.01mm），段数计入报告
- [x] 2.11 （实测新增）宏定义块整体跳过；`TO./TA./TF./TD` 属性记录不被当成选刀/数据块

## 3. 铜几何（spec: copper-geometry）

- [x] 3.1 占用栅格：圆/矩/椭圆 flash、不同孔径的 draw（含端点圆帽）、区域多边形、宏图元
- [x] 3.2 确定性：同输入同分辨率两次栅格化结果一致（`Raster` 可比较）
- [x] 3.3 按半径膨胀（安全侧加半个像素 + 简化容差）
- [x] 3.4 边界跟踪提取闭合轮廓（格子边界图遍历，非 Moore 点跟踪；对环可分出内轮廓）
- [x] 3.5 Douglas-Peucker 简化，点数下降且闭合性保持
- [x] 3.6 最小距离校验：路径到铜的最近距离 ≥ 请求安全间距（**实测**，非推导）— 真实: 0.2761 ≥ 0.2500mm
- [x] 3.7 最近邻串联路径，结果确定

## 4. 激光作业（spec: laser-job）

- [x] 4.1 作业结构：三类走刀（隔离/钻孔/板框）+ 每层参数 + 参数来源
- [x] 4.2 坐标归一：平移到非负，记录平移量与 Y 约定
- [x] 4.3 隔离多遍（遍数单一真值来源：参数档）；板框多遍
- [x] 4.4 钻孔 Mark / Circle 两种模式
- [x] 4.5 缺输入（无钻孔/无板框）时仍可生成并报告缺项影响
- [x] 4.6 作业统计：各类段数、总长、空程、预估时间、包围盒；可复现

## 5. G-code 与预览（spec: laser-output）

- [x] 5.1 GRBL 子集输出：前言/`G0`/`M3 S`/`G1 F`/`G4`/`M5`/`M2`，指令白名单
- [x] 5.2 注释中标注板材、功率、速度、遍数与来源 + 试刻提示 + 免责声明
- [x] 5.3 空程用 `G0` 且激光关闭（校验器会把「激光开启时 G0」判为违规）
- [x] 5.4 确定性输出（同作业两次生成字节级一致）
- [x] 5.5 SVG 分层预览（三类不同颜色 + 图例 + 板框虚线），确定性，XML 转义
- [x] 5.6 G-code 结构校验：指令白名单、坐标有限且落在板内、功率不超上限、结尾 `M2`

## 6. 参数知识库（spec: laser-profile）

- [x] 6.1 `LaserProfile` 结构与 6 条常见板材条目（含 source 与适用范围）
- [x] 6.2 查询：命中/近似命中/未命中（未命中给 nearest 参考并明确未收录）
- [x] 6.3 强制免责：起点、需试刻、与机器相关（测试断言每条都承认未验证）
- [x] 6.4 用户覆盖：功率/速度/遍数/最大 S 可覆盖，来源在作业与 G-code 中标注

## 7. 工具与提示词（spec: laser-workflow）

- [x] 7.1 工具 `gcode_import_fab`：解析 Gerber/钻孔并报告（含问题与提示）
- [x] 7.2 工具 `gcode_laser_job`：生成作业（统计 + 几何校验）并导出 G-code 与 SVG
- [x] 7.3 工具 `gcode_laser_params`：查询/说明参数与覆盖入口
- [x] 7.4 注册进 `gcode_tools::all_tools()`（15 个工具），schema 完整
- [x] 7.5 提示词：制造工作流 + **不得声称已实际打样** + 参数需试刻 + 图形不完整要警告
- [x] 7.6 工具参数校验测试（缺参、未知路径、非法几何参数、仅铜层、标记模式警告）
- [x] 7.7 未支持构造报告透传到工具输出（测试：含 `SR` 的文件在输出里出现计数）

## 8. 收尾

- [x] 8.1 全量 `cargo test --workspace` 通过（351 项；R0/R1/R1.5 无回归）
- [x] 8.2 验收 example：真实 demo 板 Gerber 跑完整链路，落盘 G-code / SVG / 报告
      （`crates/gcode-laser/tests/acceptance.rs`，两块板）
- [x] 8.3 `docs/PRD-gcode.md` 附五：R2 状态、已验证范围与「无硬件所以未打样」
- [x] 8.4 `openspec validate r2-laser-fabrication --strict` 通过

## 未做（明确不做）

- 双面翻面对位、阻焊/丝印激光工艺、机械钻孔/铣削、非 GRBL 控制器、标签桥（tab）。
- `SR` 步进重复、`LM/LR/LS`、负极性图形只做「计数 + 报告」。
- 实际打样：本机没有激光机与 GRBL 控制器。
