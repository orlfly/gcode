# R1.5 实测 — 实施任务

> 状态标记：`[x]` 已完成并有验证；`[ ]` 未完成/阻塞（原因见行内说明）。

## 1. crate 骨架

- [ ] 1.1 新建 `crates/gcode-instrument`，加入 workspace members 与 workspace.dependencies
- [ ] 1.2 依赖：`gcode-kicad`、`gcode-sim`、`serde`、`serde_json`、`thiserror`、`tokio`、`async-trait`；dev: `tokio`(macros)
- [ ] 1.3 `lib.rs` 导出模块（hal / safety / plan / compare / report / instruments）

## 2. HAL（spec: instrument-hal）

- [ ] 2.1 `Quantity` / `Capabilities` / `MeasureRequest` / `Measurement` / `SourceRequest` 类型；结果带 `backend`、`device`、`synthetic`
- [ ] 2.2 `Instrument` trait（`id` / `capabilities` / `measure` / `source`）
- [ ] 2.3 能力发现：查询仪器返回支持的测量量与上限
- [ ] 2.4 后端探测：列出 virtual / sigrok / serial-psu 是否可用及原因
- [ ] 2.5 显式选择后端；不可用时返回可操作错误，不静默回退

## 3. 安全层（spec: measurement-safety）

- [ ] 3.1 上限常量与检查：电压 ≤48V、电流 ≤5A，越界拒绝并说明触发项
- [ ] 3.2 边界值（48V/5A）允许
- [ ] 3.3 `ConfirmToken`：仅工具层可铸造、一次性消耗；缺失/已用 → 拒绝
- [ ] 3.4 `SafeInstrument` 装饰层：在进后端前检查上限与令牌；任何后端都无法绕过
- [ ] 3.5 预期值来源约束：无 `Expected` 的步骤不纳入计划
- [ ] 3.6 拒绝路径在报告中有记录（不是静默跳过）

## 4. 测试计划 IR（spec: test-plan）

- [ ] 4.1 `TestPlan` / `TestStep` / `Expected` / `Baseline` 类型，可 JSON 往返
- [ ] 4.2 步骤含目标、测量量、预期值+容差、单位、是否带电
- [ ] 4.3 预期值来源标记（仿真含电路/节点/分析标识，或用户输入）
- [ ] 4.4 构建期拒绝缺预期值的步骤并指出该步骤
- [ ] 4.5 从 `gcode-sim` 配方生成计划（仿真基准 → 预期值）

## 5. 对比与 KR2（spec: sim-measured-compare）

- [ ] 5.1 偏差计算：绝对 + 百分比；基准为 0 时只报绝对偏差
- [ ] 5.2 KR2 阈值判定：直流 ≤10%、AC 峰值频率 ≤20%
- [ ] 5.3 阈值可覆盖，且输出注明「默认 KR2」或「覆盖值」
- [ ] 5.4 FAIL 归因（器件容差/焊接接触/拓扑不符/模型偏差/测量条件）
- [ ] 5.5 缺基准或缺实测 → 标记不可比且不计入 PASS

## 6. 报告 IR（spec: test-report）

- [ ] 6.1 `TestReport` 对齐 PRD §7.3.5：`plan_id` / `items[step,target,expected{value,tol},measured,unit,verdict,analysis?]` / `summary`
- [ ] 6.2 PASS 项无 analysis；FAIL 项带归因
- [ ] 6.3 summary 含通过数/总数与整体结论
- [ ] 6.4 来源标注（后端/设备，区分 virtual / demo / 真实）
- [ ] 6.5 中文可读渲染 + JSON/文本落盘，同输入字节级一致

## 7. 执行器（安全 + 时序 + 熔断）

- [ ] 7.1 `Executor::run(plan, meter, supply?)`：逐步骤执行，带电步骤先验令牌再上电
- [ ] 7.2 熔断：实测电流 > 2× 预期 → 立即断电 + 终止后续步骤 + 报告记录事件
- [ ] 7.3 正常范围内不熔断
- [ ] 7.4 任何结束路径（成功/熔断/拒绝/异常）都必须关闭输出
- [ ] 7.5 用虚拟后端做端到端测试（含熔断、拒绝、异常三条路径）

## 8. sigrok 后端（spec: instrument-hal）

- [ ] 8.1 构建命令并调用 `sigrok-cli`（`GCODE_SIGROK_CLI` 可覆盖）
- [ ] 8.2 解析 `<通道>: <值> <单位> <类型>` 逐行读数
- [ ] 8.3 错误分类：CLI 缺失 / 设备不存在 / 解析失败，各自给出不同可操作提示
- [ ] 8.4 集成验证：对真实 sigrok-cli 的 demo 设备采集一次（无 sigrok 时跳过并提示）
- [ ] 8.5 来源标注：demo 设备产出的结果 `synthetic = true` 且报告标注来源为 demo

## 9. serial-psu 后端（spec: instrument-hal）

- [ ] 9.1 仅在 `GCODE_PSU_PORT` 设置时可用；未配置时报不可用并说明
- [ ] 9.2 最小 SCPI-lite 子集：`MEAS:VOLT?` / `MEAS:CURR?` / `VOLT <v>` / `CURR <c>` / `OUT 0|1`
- [ ] 9.3 响应解析用录制样本测试（含异常/超时样本）
- [ ] 9.4 源输出经安全层：上限 + 令牌（不绕过）

## 10. 工具与提示词（spec: instrument-workflow）

- [ ] 10.1 新增工具：列仪器与能力 / 生成测试计划 / 执行测试并返回报告
- [ ] 10.2 注册进 `gcode_tools::all_tools()`，schema 完整
- [ ] 10.3 工具层强制确认令牌：无令牌则拒绝任何带电执行并提示用户确认
- [ ] 10.4 提示词：不得编造测量值、带电必须确认、结论必须来自实测或仿真
- [ ] 10.5 无仪器时降级：给连接/配置提示，不影响生成/ERC/仿真
- [ ] 10.6 工具参数校验测试（缺参、超限、无令牌均拒绝且可解释）

## 11. 收尾

- [ ] 11.1 全量 `cargo test --workspace` 通过（R0/R1 不回归）
- [ ] 11.2 验收 example：用虚拟后端 + sigrok demo 设备跑完整链路并落盘报告
- [ ] 11.3 `docs/PRD-gcode.md` 附四：R1.5 状态、已验证范围与「无硬件所以 KR2 未判定」
- [ ] 11.4 `openspec validate r15-instrument --strict` 通过
