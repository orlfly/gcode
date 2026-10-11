# R1.5 实测 — 实施任务

> 状态：全部完成。证据见 `instrument-report.md` 与 `docs/PRD-gcode.md` 附四。
>
> 诚实边界：本机无真实测量硬件，因此链路与判定机制已验证，**物理板卡的 KR2 结论未判定**。

## 1. crate 骨架

- [x] 1.1 新建 `crates/gcode-instrument`，加入 workspace members 与 workspace.dependencies
- [x] 1.2 依赖：`gcode-kicad`、`gcode-sim`、`serde`、`serde_json`、`thiserror`、`tokio`（含 io-util）、`async-trait`；dev: 同上
- [x] 1.3 `lib.rs` 导出模块（hal / safety / plan / compare / report / executor / instruments）+ 测试用 env 锁

## 2. HAL（spec: instrument-hal）

- [x] 2.1 `Quantity` / `Reduce` / `Capabilities` / `MeasureRequest` / `Measurement` / `SourceRequest`；结果带 `backend`、`device`、`synthetic` 与 `source_label()`
- [x] 2.2 `Instrument` trait（`id` / `capabilities` / `measure` / `source(req, auth)`）；确认令牌作为授权参数而非请求字段
- [x] 2.3 能力发现：`Capabilities::supports()` 与上限；有测试
- [x] 2.4 后端探测：`probe_backends()` 列出三者可用性及原因；集成测试断言顺序与原因非空
- [x] 2.5 显式选择：工具的 `meter` 参数（auto/sigrok/virtual）；不可用返回可操作错误，不静默回退

## 3. 安全层（spec: measurement-safety）

- [x] 3.1 上限常量与 `check_limits()`：越界拒绝并说明触发项
- [x] 3.2 边界值（48V/5A）允许（有测试）
- [x] 3.3 `ConfirmToken::mint(user_confirmed)` 不可克隆/不可反序列化；一次会话 `arm()`，二次拒绝
- [x] 3.4 `SafeInstrument` 装饰层 + `safety::authorise()`；**执行器也调用同一函数**（纵深防御，裸后端也拦得住）
- [x] 3.5 预期值来源约束：`TestStep` 必需 `Expected`；从 JSON/工具构建时缺预期值即拒绝
- [x] 3.6 拒绝路径写入报告（`REJECTED` + 原因 + 可操作提示），有测试

## 4. 测试计划 IR（spec: test-plan）

- [x] 4.1 `TestPlan` / `TestStep` / `Expected` / `Baseline`，JSON 往返测试
- [x] 4.2 步骤含目标、测量量、预期值+容差、单位、通道/采样/归约、是否带电
- [x] 4.3 来源标记：`Baseline::Simulation{circuit,node,analysis}` / `User`，报告里展示
- [x] 4.4 构建期拒绝缺预期值（含缺单位）并指出步骤
- [x] 4.5 `TestPlan::from_recipe()`：dc_voltage→电压、fc_3db→频率；不可测指标记入 notes 而不伪造

## 5. 对比与 KR2（spec: sim-measured-compare）

- [x] 5.1 偏差计算：绝对 + 百分比；基准为 0 时报绝对偏差且**不判定**（相对容差无意义）
- [x] 5.2 KR2 阈值：`KR2_DC_PCT=10`、`KR2_AC_FREQ_PCT=20`；两端都有测试（6% PASS / 15% FAIL / 25% FAIL）
- [x] 5.3 `TolerancePolicy::{kr2_default,override_with}` + `ToleranceSource` 注明来源（含步骤自带容差）
- [x] 5.4 FAIL 归因：列出可能原因与建议；合成数据额外标注
- [x] 5.5 缺实测/缺基准 → `NOT_COMPARABLE`，不计入 PASS（有测试）

## 6. 报告 IR（spec: test-report）

- [x] 6.1 `TestReport` 对齐 §7.3.5；有测试用 serde_json 逐字段校验；额外字段为来源/偏差等补充
- [x] 6.2 PASS 项无 analysis；FAIL 项带归因（有测试）
- [x] 6.3 summary 含通过数/总数/拒绝/不可比与整体结论
- [x] 6.4 来源标注（backend/device/synthetic），区分 virtual / demo / 真实；报告对合成数据显式警告
- [x] 6.5 中文渲染 + JSON/文本双产物落盘；确定性与字段有测试

## 7. 执行器（安全 + 时序 + 熔断）

- [x] 7.1 `Executor::run(plan, auth)`：逐步骤执行；带电步骤先过安全闸门再上电
- [x] 7.2 熔断：>2× 预期电流 → 立即断电 + 终止后续步骤 + 事件与拒绝原因入报告（有测试）
- [x] 7.3 恰为 2× 不熔断（有测试）
- [x] 7.4 任何结束路径都关闭输出；测试断言最后一条命令是 `OUT=Some(false)`
- [x] 7.5 虚拟后端端到端：正常/熔断/无令牌/无电源/测量失败五条路径均有测试

## 8. sigrok 后端（spec: instrument-hal）

- [x] 8.1 构建并调用 `sigrok-cli`；`GCODE_SIGROK_CLI` 可覆盖
- [x] 8.2 逐行读数解析 + `Mean/Peak/Last` 归约；用真实 demo 输出形状做夹具测试
- [x] 8.3 三类错误分开（`Unavailable` / `DeviceNotFound` / `Parse`），各有提示与测试
- [x] 8.4 集成验证：真实 sigrok-cli demo 设备采集到 10 V 峰值（无 sigrok 时跳过并提示）
- [x] 8.5 demo 产出标记 `synthetic = true` 且报告标注「非真实测量」；优先选真设备而非 demo

## 9. serial-psu 后端（spec: instrument-hal）

- [x] 9.1 仅在 `GCODE_PSU_PORT` 指向存在的设备时可用；未配置/设备不存在分别报错
- [x] 9.2 最小 SCPI-lite 子集（读电压/电流、设压/设流、开关输出），纯 `tokio::fs` 读写 tty
- [x] 9.3 响应解析测试：裸数、带单位、科学计数、命令回显、无数字、超时无响应
- [x] 9.4 源输出经安全层（装饰层与执行器双重检查）；后端自身不做授权判断

## 10. 工具与提示词（spec: instrument-workflow）

- [x] 10.1 新增 `gcode_list_instruments` / `gcode_plan_test` / `gcode_run_test`
- [x] 10.2 注册进 `all_tools()`（共 12 个，名称唯一），schema 完整
- [x] 10.3 工具层强制确认：无 `user_confirmed` 时**不执行任何带电操作**，返回待确认清单（有测试）
- [x] 10.4 提示词加入「测量值不得编造」「带电必须确认」「合成数据不得当作真实结论」与安全上限
- [x] 10.5 无仪器时给连接/配置提示；生成/ERC/仿真不受影响（有测试）
- [x] 10.6 工具参数校验测试：缺计划、缺预期值、无令牌、未知 meter 均拒绝且可解释

## 11. 收尾

- [x] 11.1 全量 `cargo test --workspace` 通过（239 项）；KR1 重跑仍 10/10，R1 仿真仍 11/11
- [x] 11.2 `examples/instrument_acceptance.rs`：无源/带电/容差外/跨域四个用例，落盘 `instrument-report.md`
- [x] 11.3 `docs/PRD-gcode.md` 附四已回填（含两个安全缺口的记录）
- [x] 11.4 `openspec validate r15-instrument --strict` 通过

## 未闭环（需硬件或后续阶段）

1. **KR2 物理结论**：缺真实 sigrok 兼容 DMM / 可编程电源与待测板；接入后设 `GCODE_PSU_PORT`
   与 `GCODE_SIGROK_CLI` 即可重跑。当前所有数据来源（virtual、sigrok demo）均为合成。
2. **串口电源真实行为**：`serial-psu` 的命令集与解析已用录制样本测试，真实设备待验证。
3. **L2/L3 检测能力**（逻辑分析仪、SCPI 示波器）：PRD 表里属 R1.5 后期 / R2+，本轮未做。
