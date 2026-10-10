## Context

见 `proposal.md - Why`。R0 已交付并验证：`NetlistIr` 契约、确定性 `.kicad_sch` 生成（KiCad 10 格式、
嵌入真实符号定义）、引脚级连接（KiCad 网表逐条一致 10/10）、ERC 10/10 PASS。

R1 只需新增一层：**数值仿真**。已确认的环境事实（全部实测）：

- `ngspice` 42 可用，且**无需 root**：`apt-get download ngspice` + `dpkg -x` 解包后
  安装到 `~/.local/bin/ngspice`（已在 PATH）。`libngspice.so.0` 随 KiCad 已存在（R1 不用）。
- `kicad-cli sch export netlist --format spice` 能从我们生成的原理图导出 SPICE：
  **无源件正确**（`R1 /IN /OUT 1k`），但**有源件只出占位节点**（`U1 __U1`、`Q1 __Q1`、`D1 __D1`），
  因为符号里没有 SPICE 模型。
- ngspice 的 `wrdata` 在 `set wr_singlescale` + `set wr_vecnames` 下输出**带表头的列式数据**
  （AC：`frequency vm(out) vp(out)`；瞬态：`time v(out)`；`.op`：一行、首列是伪 scale）。
  这是机器可读取回的正道，不需要解析屏幕文本。

约束：R0 的 crate 与工具已稳定；R1 不得破坏 R0 行为（生成/ERC 在无 ngspice 时仍须可用）。

## Goals / Non-Goals

**Goals:**

- `NetlistIr` + 分析配置 → 可直接跑的 SPICE 网表，确定性、有模型、有接地归一化。
- DC / AC（含波特图）/ 瞬态 三类分析的执行与**机器可读**波形取回。
- 指标提取（工作点、增益、-3dB、相位裕度、上升时间、纹波）→ 与解析预期比对 → PASS/FAIL + 中文解读。
- 产物：CSV 波形 + 无依赖 SVG 图。
- 电路目录的可仿真子集全部落在容差内，并输出可复算的验收报告。

**Non-Goals:**

- 不做数字/混合信号、噪声/失真、蒙特卡洛、灵敏度、自动扫参寻优。
- 不做实测对比（R1.5）；不追厂商级模型精度。
- 不引入 ngspice 的共享库 FFI（保留为后续可选后端）。
- 不改变 R0 的 `.kicad_sch` / ERC 行为。

## Decisions

### D1 · SPICE 网表从 IR 生成，KiCad 导出只作交叉校验

- 备选：直接采用 `kicad-cli ... --format spice` 的输出。否决：它对我们符号里的有源件只给
  占位节点（`U1 __U1`），无法仿真；且我们要控制模型注入与接地归一化。
- 决定：`gcode-sim::netlist` 从 `NetlistIr` 生成；同时提供与 KiCad 导出的**拓扑一致性校验**
  （节点集合与器件连接，允许命名/顺序差异），用于证明原理图与仿真网表表达同一电路。

### D2 · 器件模型用注册表 + 行为级 macromodel，并显式标注

- 注册表以 `lib_id` 为键，映射到器件卡类型与模型定义：
  - 无源件：`Device:R`→`R`、`Device:C`→`C`、`Device:L`→`L`（直接用 value）。
  - 源：`Device:V`/`power:*`? 不自动注入源；源由分析配置显式给出（见 D3）。
  - 有源件：`Amplifier_Operational:LM321`→单极点运放 macromodel（开环增益/主极点/输出级）；
    `Device:Q_NMOS`→`.model` NMOS 开关级；`Device:LED`→`.model` 二极管；
    `Regulator_Linear:L7805`→行为级稳压 macromodel（B 源 `min(V(IN),5)`）。
- 无模型的 `lib_id` → **报错**，不静默生成无效卡。
- 输出中标注「行为级 macromodel，非厂商模型」，避免用户误信精度。KR2 的误差指标留给 R1.5 实测校准。

### D3 · 激励源由分析配置提供，不写进 IR

IR 只描述电路拓扑，不含激励。分析配置里显式给激励（DC 值、AC 幅度、瞬态波形）与探针：

```
Analysis {
  kind: Op | Ac { sweep: Dec{points}, start, stop } | Tran { step, stop },
  sources: [ { net: "VIN", dc: 12.0, ac: 1.0, pulse: ... } ],
  probes: ["VOUT", "OUT"],
}
```
- 备选：把源做成 IR 里的元件（`Device:V`）。否决：会让「电路」与「测试条件」耦合，
  同一电路要跑不同激励就得改设计文件。分离更干净，也便于 R1.5 复用同一电路跑不同激励。

### D4 · 节点归一化：GND→0，其余安全化并检测碰撞

- `GND`（大小写不敏感，含 `GND`/`0`）→ SPICE 节点 `0`；其余网络名去除非 `[A-Za-z0-9_]`
  并加前缀避免与数字冲突；归一化后重名 → 报错（否则会静默短路两个网络）。

### D5 · 波形取回用 wrdata（单 scale + 表头），按列名解析

- 在 `.control` 里 `set wr_singlescale` + `set wr_vecnames`，再 `wrdata <file> <probes...>`。
- 解析器**按表头名取列**，不按位置——因为 `.op` 的首列是伪 scale（实测为 `in`）。
- 备选：解析 stdout 文本表。否决：脆弱、列宽依赖终端。

### D6 · 指标提取是纯函数，可离线测试

`analysis` 模块只吃「波形向量」，输出指标；不碰进程与文件。这样：
- 指标算法（-3dB 插值、增益、相位裕度、上升时间）可用合成数据做单元测试，无需 ngspice；
- 只有 `runner` 依赖 ngspice，集成测试按环境跳过。

### D7 · SVG 手写，确定性优先

不引入绘图库：手写折线 + 坐标轴 + 网格，浮点格式化统一（复用 `fmt_num` 思路），
保证同数据 → 同字节。波特图用双面板（幅频 + 相频）。

### D8 · 配方与 R0 目录同源

`gcode-sim` 依赖 `gcode-kicad`，配方以 `catalog::CircuitEntry.id` 为键，
避免两处维护电路清单。配方含分析类型、探针、激励、**解析预期值与容差**及其推导说明。

### D9 · 依赖方向

`gcode-kicad`（IR/生成/ERC）← `gcode-sim`（仿真）← `gcode-tools`（工具）← `gcode-agent`。
`gcode-sim` 不反向依赖工具或 agent。

### D10 · ngspice 定位与失败处理

`GCODE_NGSPICE` → `PATH` 上的 `ngspice`；解析 `ngspice --version` 得版本；批处理 `-b`。
失败（不收敛/语法错/缺二进制）→ 结构化错误 + 保留原始输出片段，绝不静默返回空波形。

## Risks / Trade-offs

- [行为级模型与真实器件有偏差，可能给人错误信心] → 输出与文档显式标注「行为级」；
  验收容差按「拓扑与量级正确」设定（1%~10%），不声称器件级精度。
- [运放 macromodel 参数不当会导致直流工作点或稳定性异常] → 用「开环增益 + 主极点 + 输出级」
  的最小可用结构，并用同相/反相放大器的解析增益做验收；参数在代码里注明物理含义。
- [瞬态不收敛] → 报告 ngspice 原文与失败状态；测试里只对稳定电路做瞬态断言。
- [`.op` 输出首列是伪 scale] → 按表头解析（D5），并有针对 `.op` 的专门测试。
- [节点归一化碰撞导致静默短路] → 归一化后查重并报错（D4），有测试。
- [KiCad 与我们的节点命名不同] → 交叉校验只比较拓扑等价类（D1），不比较字面名。
- [无 ngspice 的环境无法跑集成测试] → 集成测试环境探测自动跳过并打印原因；
  网表生成与指标提取始终可离线回归。

## Migration Plan

1. 加 `gcode-sim` crate 与 workspace 成员（不触碰 R0 行为）。
2. 网表生成 + 模型注册表 + 节点归一化（离线可测）。
3. runner（探测/执行/wrdata 取回）+ 失败诊断。
4. analysis（指标 + 判定 + 解读）+ artifacts（CSV/SVG）。
5. recipes + 集成验收 + 报告。
6. 工具与提示词接入；跑全量测试与仿真验收。
7. 回滚：各步独立提交；`gcode-sim` 为纯新增，移除 workspace 成员即回到 R0 状态。

## Open Questions

- 是否为 `.dc` 扫描（`Analysis::Dc { source, start, stop, step }`）在 R1 内提供？当前列入
  验收的是 `.op`/`.ac`/`.tran`；`.dc` 可低成本加入，若不影响验收则不阻塞。
- 相位裕度只对含反馈环路的电路有意义；对无反馈电路是否输出该指标（或标记 N/A）可在实现时定。
