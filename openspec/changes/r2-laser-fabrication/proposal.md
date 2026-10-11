## Why

R0/R1 交付了设计侧闭环（KR1 10/10 通过 ERC）、R1.5 交付了实测链路（机制已验证）。
PRD §8 的 **R2 · 制造** 是最后一段缺口：把设计变成可以上机的东西。
但现状是流程断裂 —— PRD §7 描述的老路径是
`KiCad → 导出 Gerber/Drill → FlatCAM（陈旧难用）→ LaserGRBL → 反复试错烧板`，
中间两段没有任何自动化，参数靠经验，且**没有预览**，试错成本是一块板。

PRD §4 的 **KR3**：「从 KiCad 项目到激光雕刻 G-code 成功打样一块双层板，无需 FlatCAM」。

已确认的环境事实（实测）：

- `kicad-cli pcb export gerbers/drill` 可用，本机有 **38 个真实 demo PCB**。
  实测导出 `ecc83-pp`：真实 RS-274X Gerber（`%FSLAX46Y46*%`、`%MOMM*%`、
  C/R/O 孔径、33 个 flash、1922 段 draw）与真实 Excellon 钻孔（33 孔）。
- **没有激光雕刻机/GRBL 控制器**（无串口设备），也没有 FlatCAM/LaserGRBL。

因此 R2 的诚实形态与 R1.5 一致：**把 Gerber → GRBL G-code 的整条链路建起来，
并用真实 Gerber/钻孔验证到「可直接上机的产物」**；物理打样（KR3 的最后一公里）
因缺硬件无法完成，必须明确标注。

## What Changes

- 新增 **`gcode-laser` crate**：
  - **Gerber 解析（RS-274X 子集）**：格式声明（`FS`/`MO`）、孔径定义（`AD` 的 C/R/O）、
    极性（`LP`）、绘制模式（`G01/G02/G03`）、操作（`D01/D02/D03`）、区域（`G36/G37`）、
    文件属性（`TF.FileFunction` 用于识别铜层/板框）。不支持的构造必须**显式报告**而不是静默跳过。
  - **Excellon 钻孔解析**：`M48` 头、`METRIC`、`T#C<直径>` 刀具表、`T#` 选刀、坐标（十进制）。
  - **铜几何 → 走刀路径**：铜图元栅格化为确定性占用网格 → 按
    `工具半径 + 安全间距` 膨胀 → 轮廓提取（边界跟踪）→ Douglas-Peucker 简化 →
    最近的顺序串联。多遍隔离雕刻通过逐步增大膨胀半径实现。
  - **激光作业（laser-job）**：把隔离雕刻、钻孔、板框切割组装成一个有序作业，
    输出作业统计（路径总长、预估时间、各层段数）。
  - **GRBL 输出**：`G21/G90/G94`、激光模式（`M3/M4` + `S` 功率）、`G0` 空程、`G1` 加工、
    收尾关机；输出**确定性**；同时产出 **SVG 走刀预览**（分层着色）。
  - **激光参数知识库**：板材（覆铜板类型/厚度）→ 功率/速度/遍数的推荐起点，
    含来源与「必须先试刻」的诚实提示。
- Agent 侧新增工具与提示词约束（**不得声称已实际打样**、参数是起点而非结论）。
- 验收：用真实 demo 板导出的 Gerber/钻孔跑完整链路，做**几何正确性校验**
  （隔离路径到铜的最近距离 ≥ 请求的安全间距；钻孔路径与 Excellon 孔位匹配；
  板框路径与 Profile 匹配），并落盘报告、G-code 与 SVG。

**Non-goals（R2 明确不做）**：非 GRBL 控制器（Smoothieware/Marlin 变体）、
双面板自动对位/翻面（只做单面一次装夹的闭环）、阻焊/丝印的激光工艺、
机械钻孔/铣削（spindle）路线、真实打样（无硬件）、FlatCAM 兼容性。

## Capabilities

### New Capabilities

- `gerber-import`: RS-274X 子集与 Excellon 钻孔的解析，产出结构化图元与孔表，
  并对不支持的构造给出可解释的报告（而不是静默丢失图形）。
- `copper-geometry`: 把铜图元栅格化为确定性占用网格，按半径膨胀，提取并简化轮廓，
  得到可用于走刀的有序路径。
- `laser-job`: 组装隔离雕刻 / 钻孔 / 板框切割三类走刀为一个有序作业，给出可复算的作业统计。
- `laser-output`: 输出可直接交付 GRBL 控制器的 G-code（含前言/收尾、激光模式与功率），
  以及分层的 SVG 走刀预览；两者都确定性。
- `laser-profile`: 板材 → 功率/速度/遍数推荐起点，带来源与适用范围说明，
  并明确「推荐值是起点，须先试刻校准」。
- `laser-workflow`: agent 侧工具（解析 Gerber、生成激光作业、导出 G-code 与预览、
  查询参数）与提示词约束（不得声称已实际打样、参数需试刻验证）。

### Modified Capabilities

<!-- 无：openspec/specs/ 为空（R0/R1/R1.5 尚未归档）。待归档后，agent 工具注册与提示词可改为
     对 agent-runtime / instrument-workflow 的 MODIFIED 增量。 -->

## Impact

**新增代码**
- `crates/gcode-laser`：`gerber`、`excellon`、`geometry`、`toolpath`、`gcode`、`svg`、`profile`、`job`。
- `crates/gcode-tools`：新增 4 个制造工具（共 16 个）。
- `crates/gcode-agent`：提示词增加制造工作流与「不得声称已打样」约束。
- `Cargo.toml`：workspace 增加 `gcode-laser`。

**依赖与环境**
- 输入用真实 Gerber：由 `kicad-cli pcb export` 从 `/usr/share/kicad/demos` 的 demo 板导出
  （本机 38 个 PCB 可选）。不引入 FlatCAM/gerbv 等外部依赖，解析自研。
- **无激光雕刻机**：G-code 只能做结构/几何校验，不能上机验证。
- 不引入几何库（无 CGAL/clipper）；栅格 + 轮廓跟踪 + Douglas-Peucker 自研，分辨率可配置。

**风险**
- **无硬件 → KR3 的最后一步（真实打样）无法完成**。→ 严格区分「产物已生成并做几何校验」与
  「已打样」；报告与文档都不得含糊；给出可上机产物与试刻建议。
- **栅格化引入分辨率误差**（刀具半径与间距的量化误差）。→ 分辨率可配置并计入安全间距
  （膨胀半径额外加半个像素）；用真实 Gerber 做「最小距离 ≥ 请求间距」的几何校验。
- **Gerber 子集覆盖不全**（圆弧 G02/G03、宏孔径 AM、负极性 LPC）。→ 明确列出未支持构造，
  解析时给出计数与提示；对 demo 板实测覆盖率并记录。
- **参数推荐责任边界**：激光参数与板材/机器强相关，推荐可能不准。→ 输出标注为**起点**，
  要求先试刻；不承诺结果。
- **G-code 正确性无法上机验证**：可能因后处理细节导致机器行为异常。→ 只依赖 GRBL 稳定子集
  （G21/G90/G94/M3/M4/G0/G1/S），并在 G-code 里注明来源与参数。
