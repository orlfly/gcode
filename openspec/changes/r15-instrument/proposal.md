## Why

R0/R1 已交付「NL → 网表 → `.kicad_sch` → ERC（10/10）」与「SPICE 仿真 → 指标判定（11/11 配方）」。
但 PRD §4 的 **KR2**（「仿真结果与实测数据在同一电路上误差进入工程可接受范围：直流 ≤10%、
AC 峰值频率 ≤20%」）至今**无法判定** —— 因为整条链路从未接触过真实测量数据。
PRD §7.3 定义的正是这块能力：分级检测、功能测试闭环、硬件抽象层（HAL）、硬性安全约束、
TestReport IR。

现状（本机实测）：

- `ngspice` 42 可用（用户级安装），R1 仿真侧就绪。
- **没有任何测量硬件**：无 `/dev/ttyUSB*`、无串口设备、无 pyserial/pyvisa。
- **`sigrok-cli` 0.7.2 已按用户级装好**（从 .deb 解包到 `~/.local/opt/sigrok` + 包装脚本），
  且 `--scan` 能找到 sigrok 自带的 **`demo` 设备**（D0-D7 + A0-A4，A0 输出 ±10 V 方波）。

因此 R1.5 的诚实形态是：**把实测链路完整建起来并验证到「真实软件 + 真实采集路径」**，
用 sigrok 的 demo 设备与进程内虚拟仪器做端到端验证；真实 DMM/可编程电源的**后端代码与协议
解析也一并实现并用录制样本测试**，但本机无法给出真实板卡的 KR2 结论。

## What Changes

- 新增 **`gcode-instrument` crate**：
  - **HAL**：统一 `Instrument` trait（能力发现 / `measure()` / `source()`），后端注册与选择。
  - **安全层（硬性）**：电压 ≤48V DC、电流 ≤5A；`source()` 必须携带**用户当轮确认令牌**，
    agent 不可自签；预期值必须来自仿真或用户显式输入；读到超预期 2 倍电流立即断电熔断。
  - **Test Plan IR**：NL 测试意图 → 结构化步骤（通道 / 预期值+容差 / 需要施加的源），
    预期值可引用 `gcode-sim` 的同节点仿真值。
  - **仿真-实测对比**：偏差计算与 **KR2 判定**（直流 ≤10%、AC 峰值频率 ≤20%），FAIL 时给归因。
  - **TestReport IR**：按 PRD §7.3.5 的 schema 输出（plan_id / items / verdict / analysis / summary）。
  - **后端**：`virtual`（进程内确定性、可注入误差，用于 CI）、`sigrok`（驱动 `sigrok-cli`，
    按通道解析读数）、`serial-psu`（串口 SCPI-lite 可编程电源，RD6006 类）。
- **带电操作确认**：`source()` 与任何上电动作在工具层强制要求 `confirm_token`，缺失即拒绝。
- Agent 侧新增实测工具与提示词约束（**不得编造测量值**、带电必须确认、结论必须来自实测或仿真）。
- 验收：Test Plan → 安全闸门 → 执行 → 对比 → TestReport 全链路，用 **sigrok demo 设备**（真实
  sigrok 软件与采集路径）与 **virtual 后端**（确定性，含误差注入）双轨验证；KR2 判定逻辑用
  合成/已知偏差数据验证；真实硬件结论留待设备接入。

**Non-goals（R1.5 明确不做）**：市电/高压（>48V DC）、大电流（>5A）、需焊接探针的自动化测试、
L3 波形级（SCPI 示波器，R2+）、自动对焦/相机对位、把 demo 设备的合成数据当作真实板卡结论。

## Capabilities

### New Capabilities

- `instrument-hal`: 统一仪器抽象（能力发现、`measure()`、`source()`）、后端注册与选择、
  可用性探测；把设备差异屏蔽在 HAL 之后。
- `measurement-safety`: 硬性电压/电流上限、带电操作的确认令牌、预期值来源约束、
  超预期电流的熔断断电；所有拒绝路径都必须可解释且不可绕过。
- `test-plan`: 测试计划 IR（步骤/通道/预期值+容差/所需激励）及其生成：从仿真基准或用户输入
  取预期值，缺预期值不得生成步骤。
- `sim-measured-compare`: 实测与仿真/用户基准的偏差计算，以及 KR2 判定
  （直流工作点 ≤10%、AC 峰值频率 ≤20%），FAIL 时给出归因与排查建议。
- `test-report`: TestReport IR（PRD §7.3.5 schema）与人类可读渲染，逐项 PASS/FAIL、偏差、
  归因，并汇总整体结论。
- `instrument-workflow`: agent 侧工具（列设备/列能力、生成测试计划、执行测试、读报告）与提示词
  约束（不得编造测量值、带电必须确认、结论必须来自实测或仿真）。

### Modified Capabilities

<!-- 无：openspec/specs/ 为空（R0/R1 尚未归档）。待归档后，agent 工具注册与提示词可改为
     对 agent-runtime / sim-workflow 的 MODIFIED 增量。 -->

## Impact

**新增代码**
- `crates/gcode-instrument`：`hal`、`safety`、`plan`、`compare`、`report`、`instruments/{virtual,sigrok,serial_psu}`。
- `crates/gcode-tools`：新增 3 个实测工具（共 12 个）。
- `crates/gcode-agent`：提示词增加实测工作流与安全约束；启动期报告纳入 sigrok 探测。
- `Cargo.toml`：workspace 增加 `gcode-instrument`。

**依赖与环境**
- `sigrok-cli` 0.7.2：用户级可用（`~/.local/bin/sigrok-cli`，包装脚本设置 `LD_LIBRARY_PATH`）；
  `GCODE_SIGROK_CLI` 可覆盖。可用设备仅有内置 `demo`。
- 无真实 DMM / 可编程电源；`serial-psu` 后端按 `GCODE_PSU_PORT` 探测，本机无设备。
- 不引入 pyserial/pyvisa；串口用纯 Rust（`tokio::fs` 读写 `tty`）实现最小 SCPI-lite 子集。

**风险**
- **没有真实硬件 → KR2 无法真正判定**。→ 明确区分「机制已验证」与「物理结论未定」，
  报告与文档都不得含糊；提供 `GCODE_*` 环境变量与后端插槽，设备接入即可跑。
- **demo 设备是合成信号**，不是被测板。→ 所有用它产生的数据在报告里标注来源为 `demo`，
  不得当作真实测量结论。
- **带电操作安全**是硬约束，误判可能损坏硬件。→ 上限检查 + 确认令牌 + 熔断三重保护，
  且拒绝路径全部有测试；`source()` 默认关闭（需显式启用后端）。
- **串口协议差异**（RD6006 类各型号命令不同）。→ 只实现文档化的最小子集（读电压/电流、
  设电压/电流、输出开关），并用录制样本做解析测试；真实设备行为待接入验证。
