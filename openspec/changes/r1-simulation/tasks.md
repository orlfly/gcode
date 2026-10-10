# R1 仿真 — 实施任务

> 状态标记：`[x]` 已完成并有验证；`[ ]` 未完成（原因见行内说明）。

## 1. crate 骨架

- [ ] 1.1 新建 `crates/gcode-sim`（lib），加入 workspace members 与 workspace.dependencies
- [ ] 1.2 依赖：`gcode-kicad`（IR + 目录）、`serde`/`serde_json`、`thiserror`、`tokio`（进程）；dev: `tokio`(macros)、`serde_json`
- [ ] 1.3 `lib.rs` 导出模块骨架（netlist / models / runner / analysis / artifacts / recipes），空实现可编译

## 2. 网表生成（spec: sim-netlist）

- [ ] 2.1 节点归一化：`GND`→`0`，其余名安全化，归一化后碰撞报错
- [ ] 2.2 器件卡映射：`Device:R/C/L` → `R/C/L`，值取 IR 的 value
- [ ] 2.3 分析配置类型：`Op` / `Ac{dec,start,stop}` / `Tran{step,stop}`，含激励源与探针
- [ ] 2.4 激励源生成：DC 值、AC 幅度、PULSE/SIN 波形（`V<name> <net> 0 ...`）
- [ ] 2.5 `.control` 块：`set wr_singlescale` + `set wr_vecnames` + `run` + `wrdata`
- [ ] 2.6 确定性测试：同 IR + 同配置两次生成字节级一致
- [ ] 2.7 与 `kicad-cli --format spice` 的拓扑一致性校验（无源件电路）

## 3. 器件模型注册表（spec: sim-netlist）

- [ ] 3.1 注册表结构（按 `lib_id` 查器件卡类型 + 模型定义），无模型时返回错误并指出 `lib_id`
- [ ] 3.2 `Amplifier_Operational:LM321` 单极点运放 macromodel（含参数物理含义注释）
- [ ] 3.3 `Device:Q_NMOS` 开关级 `.model`、`Device:LED` 二极管 `.model`
- [ ] 3.4 `Regulator_Linear:L7805` 行为级稳压 macromodel（B 源 `min(V(IN),5)`）
- [ ] 3.5 模型注入测试：含 LM321 的电路网表里出现子电路定义且实例引用它
- [ ] 3.6 输出中标注「行为级 macromodel」来源

## 4. ngspice 驱动（spec: sim-runner）

- [ ] 4.1 工具链探测：`GCODE_NGSPICE` → `PATH`，解析版本，缺失/过低给可操作错误
- [ ] 4.2 批处理执行 `ngspice -b`，写临时网表到工作目录，捕获 stdout/stderr
- [ ] 4.3 `wrdata` 结果解析：**按表头名**取列（含 `.op` 伪 scale 情形）
- [ ] 4.4 波形数据结构：自变量 + 各探针序列；长度一致性校验
- [ ] 4.5 失败可诊断：不收敛/语法错 → 结构化错误 + 原始输出片段（不静默返回空）
- [ ] 4.6 集成测试：AC / OP / TRAN 三类各取回一次真实数据（无 ngspice 时跳过并提示）

## 5. 指标与解读（spec: sim-analysis）

- [ ] 5.1 指标提取纯函数：直流节点电压、通带/低频增益、-3dB 截止（插值）、相位裕度、上升时间、纹波
- [ ] 5.2 与预期/容差比对：逐项 PASS/FAIL + 实测/预期/偏差（绝对与百分比）
- [ ] 5.3 中文解读与 FAIL 归因建议（元件容差 / 模型精度 / 拓扑接错）
- [ ] 5.4 「不自己下结论」：无仿真数据时不判定，返回仿真失败
- [ ] 5.5 离线单元测试：用合成波形验证 -3dB 插值、增益、上升时间、判定边界

## 6. 产物（spec: sim-artifacts）

- [ ] 6.1 波形 CSV（表头 + 数值），同数据字节级一致
- [ ] 6.2 SVG 波特图（幅频 + 相频双面板，含坐标轴与网格），无外部依赖、确定性
- [ ] 6.3 SVG 时域/扫描曲线图
- [ ] 6.4 可预测命名（含电路标识与分析类型），覆盖前不静默丢失

## 7. 配方与验收（spec: sim-catalog）

- [ ] 7.1 为可仿真电路定义配方：分析类型、探针、激励、解析预期值 + 容差 + 推导说明
- [ ] 7.2 离线回归：配方与目录一一对应、预期值字段完整（不依赖 ngspice）
- [ ] 7.3 集成验收：每个可仿真电路跑配方并断言全部指标落容差（无 ngspice 时跳过）
- [ ] 7.4 验收报告 example：逐电路逐指标的实测/预期/偏差/判定，可落盘
- [ ] 7.5 运行验收并落盘报告到 `openspec/changes/r1-simulation/`

## 8. 工具与提示词（spec: sim-workflow）

- [ ] 8.1 新增工具：列出仿真配方 / 跑仿真并解读（含产物路径）/ 查看仿真工具链
- [ ] 8.2 注册进 `gcode_tools::all_tools()` 并保持 schema 完整
- [ ] 8.3 提示词增加仿真工作流与「数值结论必须来自仿真结果」约束
- [ ] 8.4 工具参数校验测试（缺参返回错误而非崩溃）
- [ ] 8.5 ngspice 不可用时降级：仿真工具给提示，R0 生成/ERC 工具不受影响

## 9. 收尾

- [ ] 9.1 全量 `cargo test --workspace` 通过（含 R0 全部测试不回归）
- [ ] 9.2 更新 `docs/PRD-gcode.md` 附三：R1 状态与仿真验收结果
- [ ] 9.3 `openspec validate r1-simulation --strict` 通过
