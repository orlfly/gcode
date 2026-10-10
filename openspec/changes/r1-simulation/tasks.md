# R1 仿真 — 实施任务

> 状态：全部完成。证据见 `sim-report.md` 与 `docs/PRD-gcode.md` 附三。

## 1. crate 骨架

- [x] 1.1 新建 `crates/gcode-sim`（lib），加入 workspace members 与 workspace.dependencies
- [x] 1.2 依赖：`gcode-kicad`、`serde`/`serde_json`、`thiserror`、`tokio`；dev: `tokio`(macros)、`serde_json`
- [x] 1.3 `lib.rs` 导出模块（netlist / models / runner / analysis / artifacts / recipes）

## 2. 网表生成（spec: sim-netlist）

- [x] 2.1 节点归一化：`GND`→`0`，其余名安全化（含数字开头加前缀），归一化后碰撞报错
- [x] 2.2 器件卡映射：`Device:R/C/L/Thermistor` → `R/C/L`，值取 IR 的 value
- [x] 2.3 分析配置类型：`Op` / `Ac{sweep,points,start,stop}` / `Tran{step,stop}`，含激励与探针
- [x] 2.4 激励源生成：DC / AC / PULSE / SIN
- [x] 2.5 `.control` 块：`set wr_singlescale` + `set wr_vecnames` + `run` + `wrdata`
- [x] 2.6 确定性测试：同 IR + 同配置两次生成字节级一致
- [x] 2.7 与 `kicad-cli --format spice` 的拓扑一致性校验（无源电路 3/3 一致，见 `tests/kicad_spice_crosscheck.rs`）

## 3. 器件模型注册表（spec: sim-netlist）

- [x] 3.1 注册表结构（按 `lib_id` 查卡类型 + 模型），无模型时报错并列出已知器件
- [x] 3.2 `Amplifier_Operational:LM321` 单极点运放 macromodel（A0=100k、主极点 10Hz、GBW≈1MHz；参数物理含义已注释；含 `Rbias` 避免 VCC 端口悬空）
- [x] 3.3 `Device:Q_NMOS` 用 **M 卡**（衬底接源极）+ level-1 `.model`；`Device:LED` 二极管 `.model`
- [x] 3.4 `Regulator_Linear:L7805` 行为级稳压 macromodel（B 源 `min(V(IN),5)`）
- [x] 3.5 模型注入测试：LM321 电路网表含子电路定义且实例引用；MOSFET 用 M 卡
- [x] 3.6 输出中标注「BEHAVIOURAL macromodels」并逐器件列出

## 4. ngspice 驱动（spec: sim-runner）

- [x] 4.1 工具链探测：`GCODE_NGSPICE` → `PATH`，解析版本，缺失/过低给可操作错误（含无需 root 的安装方法）
- [x] 4.2 批处理执行 `ngspice -b`，捕获 stdout/stderr，运行前清除旧产物
- [x] 4.3 `wrdata` 结果**按表头名**解析（含 `.op` 伪 scale 情形）
- [x] 4.4 波形结构：自变量 + 各探针序列；行长一致性校验
- [x] 4.5 失败可诊断：不收敛/语法错 → 结构化错误 + 原始输出片段（绝不静默返回空）
- [x] 4.6 集成测试：AC / OP / TRAN 三类各取回真实数据（含相位为弧度的断言）

## 5. 指标与解读（spec: sim-analysis）

- [x] 5.1 指标提取纯函数：直流电压、低频增益（线性/dB）、形状感知 -3dB（低通/高通）、相位裕度（近似）、上升时间、纹波、极值
- [x] 5.2 与预期/容差比对：逐项 PASS/FAIL + 实测/预期/绝对与百分比偏差
- [x] 5.3 中文解读与 FAIL 归因建议（元件容差 / 模型精度 / 拓扑接错）
- [x] 5.4 「不自己下结论」：无仿真数据不判定；空预期列表也不算通过
- [x] 5.5 离线单元测试：合成波形验证 -3dB 插值、高通形状、上升时间、纹波、判定边界

## 6. 产物（spec: sim-artifacts）

- [x] 6.1 波形 CSV（表头 + 数值），同数据字节级一致
- [x] 6.2 SVG 波特图（幅频 dB + 相频角度双面板，坐标轴/网格/图例），无外部依赖、确定性
- [x] 6.3 SVG 时域/扫描曲线图
- [x] 6.4 可预测命名 `<circuit>.<tag>.csv|svg`

## 7. 配方与验收（spec: sim-catalog）

- [x] 7.1 11 个配方覆盖全部 10 类电路：分析类型、探针、激励、解析预期值 + 容差 + 推导说明
- [x] 7.2 离线回归：配方与目录一一对应、探针/激励必须是电路真实网络、预期值字段完整（不依赖 ngspice）
- [x] 7.3 集成验收：每个配方跑真实 ngspice 并断言全部指标落容差（无 ngspice 时跳过并提示）
- [x] 7.4 验收报告 example：逐电路逐指标的实测/预期/偏差/判定 + 依据，可落盘
- [x] 7.5 运行验收并落盘 `openspec/changes/r1-simulation/sim-report.md`（11/11 配方、12/12 指标）

## 8. 工具与提示词（spec: sim-workflow）

- [x] 8.1 新增工具：`gcode_list_sim_recipes` / `gcode_run_simulation` / `gcode_check_sim_toolchain`
- [x] 8.2 注册进 `gcode_tools::all_tools()`（共 9 个工具，名称唯一），schema 完整
- [x] 8.3 提示词增加仿真工作流与「数值结论必须来自仿真结果」+「行为级模型非器件级精度」约束
- [x] 8.4 工具参数校验测试（缺参/未知电路/缺 analysis 返回提示而非崩溃）
- [x] 8.5 ngspice 不可用时降级：仿真工具给提示，R0 生成/ERC 工具不受影响（有测试）

## 9. 收尾

- [x] 9.1 全量 `cargo test --workspace` 通过（162 项；R0 的 KR1 仍 10/10）
- [x] 9.2 `docs/PRD-gcode.md` 附三：R1 状态、仿真验收结果、7 项真实问题与修复
- [x] 9.3 `openspec validate r1-simulation --strict` 通过

## 未闭环（需后续阶段）

1. **KR2（仿真 vs 实测误差）**：需要 R1.5 的实测数据（Sigrok 采集），当前只能给出仿真侧结论。
2. **NL → 仿真配置 的真实成功率**：需要在配置了 provider 的 agent 会话里用自然语言跑一遍。
3. 数字/混合信号、`.dc` 扫描、ngspice 共享库 FFI：R1 明确不做。
