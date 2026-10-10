# PRD: Gcode — 自然语言电路设计与制造助手

> 状态：v0.2 · 2026-10-10（v0.1 曾用名 KAgents，已正式定名 Gcode；新增架构约束节）

## 0. 架构约束（v0.2 新增）

- **主题框架**：基于 `/opt/workspace/jcode`（Jcode 本体，Rust workspace，82 个 crate）构建，复用其 agent runtime、Tool trait（`jcode-tool-core`）、TUI/CLI、会话与权限体系
- **CLI 入口**：Rust 实现，以新二进制 `gcode` 落在 Jcode workspace 内（`[[bin]] name = "gcode"`），领域能力以 `gcode-*` crate 系列实现：
  - `gcode-kicad`：s-expression 解析/生成、`.kicad_sch`/`.kicad_pcb` 读写、ERC/DRC 桥接（kicad-cli）
  - `gcode-sim`：ngspice 驱动（DC/AC/瞬态）、波形取回与解读
  - `gcode-instrument`：Sigrok/串口实测采集、仿真-实测对比
  - `gcode-laser`：Gerber→GRBL G-code、SVG 走刀预览、参数知识库
  - `gcode-tools`：实现 `jcode_tool_core::Tool` trait，把上述能力注册为 agent 工具
- **交互形态**：CLI 对话（复用 Jcode CLI 骨架），文件系统为与 KiCad GUI 的同步媒介，无 GUI 插件

## 1. 摘要

Gcode 是一个 CLI 形态的专业 code agent，让用户用自然语言完成 KiCad 电路设计、SPICE 仿真验证、实际电路测试和激光 PCB 制造。KiCad 10 的开放格式（s-expression、kicad-cli、IPC API、内置 ngspice）首次让这类 agent 成为可能。

## 2. 联系人

- 发起人/产品：本项目所有者
- 工程与验证：待组建

## 3. 背景

硬件开发流程由多个割裂的工具组成：KiCad 画图、SPICE 仿真、万用表/示波器实测、FlatCAM + LaserGRBL 激光打样。每一步都要手工操作，参数靠经验试错，实测结果要人肉比对。软件侧已有成熟的 AI code agent，但电路设计领域还没有——因为此前 EDA 工具不开放。KiCad 10 改变了这一点：文件是文本格式、CLI 可 headless 运行、仿真可编程驱动，LLM agent 首次能端到端操作整个流程。

## 4. 目标

让电子工程师和爱好者用对话完成「想法 → 验证过的设计 → 实测数据 → 可制造的 PCB」。

**关键结果**：
- KR1：10 个典型电路（放大、滤波、电源、传感器接口等）从自然语言描述到通过 ERC + 仿真的 KiCad 工程，首版目标成功率 ≥ 7/10（含一轮对话式修正）
- KR2：仿真结果与实测数据在同一电路上误差进入工程可接受范围（首版暂定：直流工作点误差 ≤ 10%，AC 峰值频率误差 ≤ 20%；随 R1.5 实测数据修正）
- KR3：从 KiCad 项目到激光雕刻 G-code 成功打样一块双层板，无需 FlatCAM

## 5. 市场细分

**S1 · 软硬结合工程师**（首选）：会编程、电子设计经验有限的开发者（IoT/嵌入式）。想自己做硬件但被 EDA 工具和仿真配置的专业门槛挡住。

**S2 · KiCad 现有用户**：用 agent 提效——快速搭原型、自动跑仿真、解读 ERC/DRC、生成制造文件。

**S3 · 硬件爱好者/maker**：有激光雕刻机，想在家快速打样电路板。

约束：CLI 形态（不做 GUI 插件）；KiCad 仅此一家（架构留接口）；激光制造仅支持 GRBL 生态。

## 6. 价值主张

### S1 软硬结合工程师
- **Who**：会写代码但画不熟练原理图的工程师
- **Why**：需要为项目做电路原型，但 EDA 工具链和仿真配置门槛高
- **What before**：照抄参考设计、论坛问人、盲改参数，仿真不会配就不配
- **How**：对话描述需求 → agent 建 KiCad 工程、配仿真、跑 ERC，结果用人话解读并给修改建议
- **What after**：几小时内得到设计正确性初步验证过的电路，不用先学一个月工具
- **替代品**：ChatGPT 问答（无工程产出）、教程自学（周期长）；选我们因为直接产出可制造的设计

> 一句话：你负责想电路要做什么，Gcode 负责让它变成验证过的设计。

### S2 KiCad 用户
- **Who**：日常使用 KiCad 的电子工程师
- **Why**：重复性操作（搭原型、配仿真、导出文件、查 ERC）占用大量时间
- **What before**：手动点击完成每个步骤，ERC 报错逐条查手册
- **How**：对话式批处理：「导出 Gerber、跑 DRC、把所有丝印移出阻焊区」一次完成
- **What after**：把工具操作时间换成设计思考时间
- **替代品**：自己写 KiCad Python 脚本（门槛高、不可复用）；选我们因为自然语言即脚本

### S3 激光打样 maker
- **Who**：拥有激光雕刻机的爱好者
- **Why**：想快速在家做 PCB 原型
- **What before**：KiCad → FlatCAM（陈旧难用）→ 反复试错烧板
- **How**：设计完成后一句话生成 G-code，含参数推荐和雕刻预览
- **What after**：从设计到上机一次成功，不再烧板
- **替代品**：FlatCAM + 手动试错；选我们因为流程一体化且参数有知识沉淀

## 7. 解决方案

### 7.1 关键流程

```
用户(CLI 对话) ──→ Gcode agent
                      ├─ P0 设计：NL → 结构化网表 → 确定性生成 .kicad_sch → ERC → 解读
                      ├─ P1 仿真：NL → ngspice 配置（DC/AC/瞬态）→ 波形/波特图 → 解读建议
                      ├─ P1.5 实测：驱动测量硬件采集真实电路数据 → 与仿真对比
                      ├─ P2 制造：设计 → Gerber → GRBL G-code + 走刀预览 + 参数推荐
                      └─ P3 闭环：仿真/实测失败 → 修改建议 → 迭代
用户随时可用 KiCad GUI 打开查看
```

### 7.2 关键功能（MVP）

1. **NL 原理图引擎**：自然语言 → 结构化电路描述（网表级中间表示）→ 确定性生成 `.kicad_sch`；NL 生成后经验证层自查，不直接输出 s-expression
2. **ERC/DRC 解读**：跑 kicad-cli 检查，把错误翻译成人话并给修复建议
3. **NL 仿真驱动**：对话配置 DC/AC/瞬态分析，输出波形图和指标解读
4. **实测数据采集**：通过可编程测量硬件（首期支持：串口控制的可编程电源/万用表，如 Sigrok 兼容设备；后续 SCPI 仪器）采集真实电路数据，agent 对比仿真与实测并分析偏差
5. **激光 G-code 生成**：Gerber/钻孔 → GRBL G-code（隔离雕刻/钻孔/板框），含 SVG 走刀预览
6. **激光参数知识库**：板材 → 功率/速度/遍数推荐起点
7. **对话式迭代**：所有修改以 diff 呈现，确认后写回

### 7.3 实测与功能测试模块（gcode-instrument）

> 产品定位：让 agent 不只会「纸上仿真」，还能驱动真实测量硬件验证物理电路板，并给出仿真-实测偏差分析与 PASS/FAIL 判定。

#### 7.3.1 检测能力分级

| 级别 | 能力 | 硬件依赖 | 阶段 |
|---|---|---|---|
| L0 无源检测 | 通断测试、电阻测量、短路排查 | 万用表（Sigrok 兼容 DMM） | R1.5 首期 |
| L1 有源直流 | 电压/电流采集、功耗测量、工作点验证 | 可编程电源（串口控制）+ DMM | R1.5 首期 |
| L2 动态测试 | PWM 占空比/频率、逻辑电平、简单时序验证 | 逻辑分析仪（Sigrok fx2/lad） | R1.5 后期 |
| L3 波形级 | 示波器采集、纹波/上升沿测量、频响实测 | SCPI 示波器（pyvisa） | R2+ |

**明确不做**：市电/高压电路（≤48V DC 上限）、大电流功率测试（≤5A）、需焊接探针的自动化测试。

#### 7.3.2 功能测试流程（闭环）

```
NL 描述测试意图（"验证 3.3V 轨供电正常、LED 驱动输出频率 1kHz"）
  → agent 生成结构化测试计划（Test Plan IR：步骤/通道/预期值/容差）
  → 用户确认（带电操作强制确认，不可跳过）
  → 硬件执行层逐步骤执行，采集数据
  → 与仿真基准比对（引用 gcode-sim 的同节点仿真值）
  → 输出测试报告：每项 PASS/FAIL、偏差值、偏差原因分析（元件容差/焊点/设计错误分类）
  → FAIL 时联动 P3 闭环：给出修改建议回到设计/仿真
```

#### 7.3.3 硬件抽象层（HAL）

统一 trait（Rust `gcode-instrument` 内），屏蔽设备差异：

```rust
trait Instrument {
    fn capabilities(&self) -> Capabilities;   // 可测什么
    async fn measure(&self, req: MeasureRequest) -> Result<Measurement>;
    async fn source(&self, req: SourceRequest) -> Result<()>;  // 可编程电源输出（需确认令牌）
}
// 实现：SigrokDmm（CLI 驱动 sigrok-cli）、SerialPps（串口 SCPI-lite）、ScpiInstrument（pyvisa 桥/Rust scpi）
```

- 首期设备目标：任一 Sigrok 支持的 DMM + 一款串口可编程电源（如 RD6006 类）
- 设备能力发现：启动时探测并声明支持哪些测试级别，agent 据此过滤可生成的测试计划

#### 7.3.4 安全约束（硬性）

1. 电压上限 48V DC、电流上限 5A，超限请求直接拒绝
2. `source()` 操作（给被测板上电/改电压）必须携带用户当轮确认令牌，agent 不可自签
3. 测试计划中的预期值必须来自仿真结果或用户显式输入，agent 不可凭空设定
4. 异常熔断：读到超预期 2 倍以上的电流立即切断电源输出并终止测试

#### 7.3.5 测试报告 schema（TestReport IR）

```json
{
  "plan_id": "tp-001",
  "items": [
    {"step": 1, "target": "3V3 rail", "expected": {"value": 3.3, "tol": 0.1},
     "measured": 3.31, "unit": "V", "verdict": "PASS"},
    {"step": 2, "target": "LED PWM freq", "expected": {"value": 1000, "tol": 50},
     "measured": 987, "unit": "Hz", "verdict": "PASS"},
    {"step": 3, "target": "opamp gain @1kHz", "expected": {"value": 10, "tol": 0.5},
     "measured": 6.2, "unit": "x", "verdict": "FAIL",
     "analysis": "增益偏低，疑似反馈电阻实际值偏差或 R2 未焊好，建议优先排查 R2"}
  ],
  "summary": "2/3 PASS，1 FAIL 已归因"
}
```

### 7.4 技术总览

- KiCad s-expression 解析器（自研或复用 kicad-python）
- `kicad-cli` headless 导出与检查；KiCad IPC API 备选
- ngspice 驱动层（复用 KiCad 仿真模型格式）
- 实测：gcode-instrument HAL（Instrument trait）+ Sigrok CLI 集成起步，SCPI 为 R2+ 扩展
- GRBL G-code 生成器 + SVG 渲染
- LLM 编排：NL → 中间表示为 agent 层；中间表示 → 文件为确定性代码

### 7.5 假设与开放问题

**假设**：
- NL → 结构化网表的正确率可通过验证层 + ERC/仿真闭环达到可用（核心赌注）
- 常用元件的 KiCad 标准库覆盖率足够首期场景（实测 20 个典型电路验证）
- Sigrok 兼容设备能覆盖基本「检测/功能测试」需求
- 用户接受文件系统作为 agent 与 KiCad GUI 的同步媒介

**开放问题**：
- 实测能力的安全边界：agent 能否自动控制带电电路？（P1.5 需要明确的操作确认机制）
- 仿真-实测偏差解读的专业责任边界（给建议，不做结论性保证）
- 商业模式：开源核心 + 增值（参数知识库/云仿真）待验证

## 8. 发布计划

| 阶段 | 内容 | 相对周期 |
|---|---|---|
| **R0 · 设计 MVP** | NL 原理图引擎（10 类典型电路）+ ERC 解读 + diff 修改 | 1-2 月 |
| **R1 · 仿真** | NL 仿真驱动（DC/AC/瞬态）+ 波形解读；数字/混合信号后置 | R0 后 1-2 月 |
| **R1.5 · 实测** | Sigrok 集成：电压/电流采集、简单功能测试（通断、逻辑电平），仿真-实测对比报告 | R1 后 1 月 |
| **R2 · 制造** | 激光 G-code 生成 + 走刀预览 + 参数推荐（KiCad→激光打样闭环） | R1.5 后 1-2 月 |
| **R3 · 全链闭环** | 设计→仿真→实测→制造打通，失败自动建议迭代 | R2 后 |

**明确不做（第一年）**：
- GUI 或 KiCad 内嵌插件
- 其他 EDA 工具（Altium/Eagle）
- 非 GRBL 的 CNC/激光控制器
- PCB 自动布局布线（agent 只做原理图级设计，布局留给用户或后续）
- 元件采购/供应链集成
- 自动控制高压/市电电路（实测能力以低压直流为限）

---

## 附：R0 技术验证清单（环境基线 2026-10-10）

- 本机环境：Ubuntu 24.04.5，8 核 / 7.5G RAM / 37G 可用磁盘
- 当前未安装：kicad、kicad-cli、ngspice、python sexpdata/pyvisa —— R0 启动时需先装 KiCad 10 + ngspice
- 待验证项：
  1. `kicad-cli` headless 导出 Gerber/网表 + ERC 运行
  2. s-expression 解析选型：自研 vs `kiutils`/`kicad-python`，用 20 个开源 KiCad 项目实测覆盖率
  3. ngspice headless 驱动：从网表跑 DC/AC/瞬态并取回波形数据
  4. NL→网表中间表示的 schema 设计（先于任何 UI/CLI 工作）

---

## 附二：R0 实施记录（2026-10-10 更新）

> 本节由 R0 实施回填。任务分解见 `openspec/changes/r0-design-mvp/`；
> 验收数据见 `openspec/changes/r0-design-mvp/kr1-report.md`。

### 环境现状（与上方基线不同）

- 本机为 Ubuntu 24.04.5 LTS，**已安装 KiCad 10.0.7（含 `/usr/bin/kicad-cli`）**，
  有 `sch erc` / `sch export netlist` / `sch upgrade` 子命令；222 个符号库与 48 个官方 demo
  工程可直接用作测试语料。
- 符号库与原理图格式均为 KiCad 10 代（库 version `20251024`，原理图 token `20260306`）。
  生成器跟到该 token：ERC / 网表导出均已在 KiCad 10 上逐类验证。
- ngspice 仍未安装（属 R1 范围）。

### 待验证项结论

1. **`kicad-cli` headless 导出 + ERC**
   - 网表导出：✅ 已验证。10 类电路的生成物全部被 `kicad-cli sch export netlist` 成功读取，
     且导出的网络节点与网表 IR **逐条一致**（10/10）。
   - ERC：✅ 已验证。`kicad-cli sch erc --format json` 对 10 类电路全部 **PASS（10/10）**，
     无 violations / unconnected / parity。
2. **s-expression 解析选型**
   - 决定：**自研解析器**（零依赖、可控、无需 Python 桥）。
   - 实测覆盖率：**309/309 个真实 KiCad 文件解析成功（100%）**，共 190 MiB，
     包含 `demos/`、`symbols/`、`footprints/`。测试见
     `crates/gcode-kicad/tests/parser_coverage.rs`。
3. **ngspice headless 驱动**：R1 事项，未执行。
4. **NL→网表 schema**：✅ 已定型为 `NetlistIr`（components/nets JSON）。
   - 生成前强制结构校验（重复位号、未知组件、空网络、空引脚、空 lib_id、网络重名、
     跨网络短路等 9 类错误），校验失败不落盘。
   - 输出确定性：等价 IR（任意输入顺序）生成字节级一致的文件。
   - 已内置 10 类典型电路目录（KR1 的基础）。

### R0 当前状态

| 能力 | 状态 |
|---|---|
| NL→网表 IR 契约 + 校验 | ✅ 完成 |
| 确定性 `.kicad_sch` 生成（嵌入真实符号库定义，含 `extends` 继承） | ✅ 完成 |
| 引脚级真实连接（KiCad 网表可复现 10/10） | ✅ 完成 |
| ERC 运行 + 中文解读 + 修复建议 + 自查循环 | ✅ 完成（ERC 10/10 PASS） |
| 电路目录（10 类）+ 离线回归 + KiCad 集成验证 | ✅ 完成 |
| 对话式 diff 修改（确认后写回、可回退、能读回既有文件） | ✅ 完成 |
| Agent 运行时（隔离 home、6 个工具、提示词契约） | ✅ 完成 |
| **KR1（确定性链路 + ERC）** | ✅ **达标 10/10（目标 ≥ 7）** |

### 重要发现：KiCad 10 符号库漂移（已修复）

- `Device:Q_NMOS_GSD` **在 KiCad 10 已不存在**，替代品 `Device:Q_NMOS` 的**引脚号从数字变成字母**
  （`D`/`G`/`S`）。旧写法会让生成器静默回退到两脚通用 stub，而 stub 反过来让 ERC 报 PASS——
  一个真实的**假通过**风险。
- 已修复并加了两道护栏测试（所有 `lib_id` 必须可解析、所有引用引脚必须存在于符号），
  验收脚本也把「回退 stub」列为不可接受（exit 4）。这正是 PRD §7.5「常用元件标准库覆盖率」
  假设的第一次真实碰撞，证明验证层能拦住它。

### 后续阻塞项

1. **NL → 网表 的真实成功率**：需要在配置了 provider 的 agent 会话里跑 10 条自然语言描述。
   本轮 10/10 是「参考实现 + 确定性链路 + ERC」的成绩，**不代表 NL 理解率**。
2. R1（仿真）需安装 ngspice 并新增 `gcode-sim` crate。

---

## 附三：R1 实施记录（2026-10-10 更新）

> 本节由 R1 实施回填。任务分解见 `openspec/changes/r1-simulation/`；
> 验收数据见 `openspec/changes/r1-simulation/sim-report.md`。

### 环境

- **ngspice 42 可用，且无需 root**：`apt-get download ngspice` + `dpkg -x` 解包后安装到
  `~/.local/bin/ngspice`。`libngspice.so.0` 随 KiCad 已存在（R1 未使用其 FFI，保留为备选后端）。
- KiCad 10 仅用于 **SPICE 导出交叉校验**，仿真本身不依赖 KiCad。

### 交付

新增 `crates/gcode-sim`：

| 模块 | 内容 |
|---|---|
| `netlist` | `NetlistIr` + 分析配置 → 确定性 SPICE 网表；`GND`→节点 `0`、节点名安全化并查重；激励与分析卡 |
| `models` | `lib_id` → 器件卡 + **行为级 macromodel**；未知器件直接报错 |
| `runner` | ngspice 探测/版本门控、批处理执行、`wrdata` 波形取回（**按列名**解析） |
| `analysis` | 指标提取（工作点、增益、形状感知的 -3dB、上升时间、纹波、极值）+ 与解析预期比对 + 中文解读 |
| `artifacts` | 波形 CSV + 无依赖 SVG（波特图/时域），确定性 |
| `recipes` | 10 类电路共 11 个仿真配方，预期值均为**解析推导**并注明依据 |

Agent 侧新增 3 个工具（`gcode_list_sim_recipes` / `gcode_run_simulation` /
`gcode_check_sim_toolchain`），提示词加入「**数值结论必须来自仿真结果**」与
「有源器件是行为级模型、不是器件级精度」的诚实边界。

### 仿真验收（ngspice-42）

| 项 | 结果 |
|---|---|
| 仿真配方通过 | **11/11** |
| 指标项通过 | **12/12** |
| 与 KiCad 自身 SPICE 导出的一致性（无源电路） | **3/3 拓扑一致** |

典型偏差：RC 低通截止频率 1587.7 Hz vs 解析 1591.5 Hz（-0.24%）；
Sallen-Key 1119.9 Hz vs 解析 1123 Hz（-0.28%）；分压器与稳压器偏差 < 0.01%。

### 仿真发现并修复的真实问题

R1 最大的价值是**把仿真当成验证器**，它当场抓出了 R0 里几个肉眼看不出的错误：

1. **同相放大器增益接错**：R0 的反馈电阻与到地电阻接反，实际增益 1.11 而非标称 10.1。
   仿真实测 0.111 V（预期 1.01 V）暴露了它 —— 这正是 KR2 想要的那类错误。
2. **Sallen-Key 阻尼过低**：原 C1/C2 使 Q≈0.34（过阻尼），-3dB 只有 f0 的 0.38 倍。
   交换 C1/C2 后 Q≈0.74，接近 Butterworth，-3dB 落在 f0 附近。
3. **热敏电阻值不可仿真**：`NTC_10k` 不是数字；改为标称值 `10k` 并在模型里注明。
4. **MOSFET 用错器件卡**：SPICE 里 `Q` 是 BJT，MOSFET 必须用 `M` 且需要衬底节点。
5. **level-1 MOS 不接受 `Cgs`/`Cgd`**（要 `CGSO`/`CGDO`），传错会直接中断仿真。
6. **`vp()` 返回弧度而非角度**：按角度假设会导致相位裕度算出 -4649°。
7. **开关关断时漏极是浮空节点**：其电压由漏电决定（实测 4.04 V 而非 5 V），
   不适合作为判据；改为测供电侧节点（无电流 → 无压降 → 严格等于 VCC）。

### R1 状态

| 能力 | 状态 |
|---|---|
| DC 工作点 / AC 扫频（含波特图）/ 瞬态 | ✅ 完成 |
| 机器可读波形取回（按列名，兼容 `.op` 的伪 scale） | ✅ 完成 |
| 指标提取 + 解析预期比对 + 中文解读与归因 | ✅ 完成 |
| 波形 CSV + 无依赖 SVG 产物 | ✅ 完成 |
| 器件模型注册表（行为级，显式标注） | ✅ 完成 |
| 配方与验收报告（11 配方 / 12 指标全绿） | ✅ 完成 |
| Agent 工具与提示词契约 | ✅ 完成 |
| **KR2（仿真 vs 实测误差 ≤10%/20%）** | ⏸ 未判定：需要 R1.5 的实测数据 |

### 后续

1. **R1.5 实测**：Sigrok 集成 + 仿真-实测对比报告，届时才能判定 KR2。
2. **R2 制造**：Gerber → GRBL G-code + 走刀预览 + 参数知识库。
3. 可选增强：数字/混合信号仿真、`.dc` 扫描、ngspice 共享库 FFI 后端。
