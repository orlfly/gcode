# R0 设计 MVP — 实施任务

> 状态标记：`[x]` 已完成并有验证；`[ ]` 未完成/阻塞（原因见行内说明）。
> 详细证据见 `kr1-report.md` 与 `docs/PRD-gcode.md` 附二。

## 1. 环境与前置验证

- [ ] 1.1 安装 KiCad 10：**阻塞**。本机已有 KiCad 7.0.11（`/usr/bin/kicad-cli`，无 `sch erc`）；升级需 sudo 密码（不可交互）或 flatpak 用户级安装（已后台启动下载，见 `$JCODE_SCRATCH_DIR/r0/install.log`）
- [ ] 1.2 headless ERC 跑通：**阻塞**于 1.1（KiCad 7 无 ERC 子命令）。已用手写最小原理图验证 kicad-cli 可加载，并确认 ERC 报告 JSON 结构（用于解析层测试夹具）
- [x] 1.3 生成物可被真实 kicad-cli 识别 + 格式 token 回填：**完成**。10/10 电路可被 `kicad-cli sch export netlist` 读取，且网表与 IR 逐条一致；格式 token 定为 `20230121`（KiCad 7）——后续版本均可读，验证方式见 `tests/kicad_integration.rs`
- [x] 1.4 s-expression 覆盖率实测：**完成**。自研解析器在 **309/309 个真实 KiCad 文件（100%，190 MiB，demos+symbols+footprints）** 上全部解析成功（`tests/parser_coverage.rs`）
- [x] 1.5 PRD 验证清单回填：**完成**，见 `docs/PRD-gcode.md` 附二

## 2. 网表 IR 契约（spec: netlist-ir）

- [x] 2.1 必填字段校验（缺 `lib_id`/`name` 指名报错）
- [x] 2.2 `IrError` 覆盖 9 类：空位号、空 lib_id、重复位号、空网络名、网络重名、未知组件、空引脚、空网络、跨网络短路
- [x] 2.3 工具入口强制先校验、错误原文回给 agent、校验失败不落盘（有测试）
- [x] 2.4 规范化（`normalized()`）使输出对输入顺序不敏感，字节级一致（有测试）
- [x] 2.5 10 类电路拓扑模板（见 5.1），全部通过 `validate()`

## 3. 原理图生成器（spec: schematic-generation）

- [x] 3.1 符号库解析模块：`GCODE_KICAD_SYMBOL_DIR` 覆盖 + 多候选路径（含 flatpak 路径），按 `lib_id` 解析真实符号定义
- [x] 3.2 嵌入真实符号定义；取不到时回退通用 stub 并显式警告
- [x] 3.3 确定性 UUIDv5（内容派生），替换随机 uuid_v4
- [x] 3.4 「同 IR + 同布局两次生成字节级一致」测试（覆盖 10 类电路）
- [x] 3.5 布局按符号真实包围盒分列排布，重叠检测测试（10 类电路全绿）
- [x] 3.6 回读保真测试：生成 → 解析 → 位号集合与网络名集合与输入一致
- [x] 3.7 真实 kicad-cli 集成测试：生成物可被 KiCad 读取且网表逐条正确（10/10）；无 kicad-cli 时自动跳过并提示；ERC 部分受版本门控

## 4. ERC 解读与自查循环（spec: erc-verification）

- [x] 4.1 kicad-cli 探测与版本门控（默认最低 10.0.0，`GCODE_KICAD_MIN_VERSION` 可覆盖；低于要求返回明确错误并拒绝执行）
- [x] 4.2 `ErcReport` 扩展：error/warning 区分、计数、稳定汇总、`to_json()`；**修正 `type` 字段反序列化**（原实现用 `type_` 未 rename，违规类型永远解析不到）
- [x] 4.3 违规类型 → 中文解释 + 修复建议静态表（覆盖 21 类高频违规）
- [x] 4.4 未知违规类型兜底（保留原始描述 + 通用建议）
- [x] 4.5 自查循环（生成→ERC→修正→重生成），带上限；PASS 结束 / 上限处列残留（两向测试 + 放弃路径测试）
- [x] 4.6 解读接入 agent：`gcode_run_erc` 输出中文解释与修复建议（可选 `json=true` 取机器可读摘要）

## 5. 电路目录与 KR1 验收（spec: circuit-catalog）

- [x] 5.1 10 类电路目录（分压、RC 低通/高通、同相/反相放大、Sallen-Key、L7805 稳压、LED 限流、MOSFET 开关、热敏分压），含 E24 取值与用途描述
- [x] 5.2 确定性夹具（10 个 golden `.kicad_sch`）落在 `crates/gcode-kicad/tests/fixtures/circuits/`
- [x] 5.3 离线回归：golden 夹具逐字节比对 + 目录/夹具无孤儿，无 LLM、无 kicad-cli 依赖
- [x] 5.4 集成测试：有 kicad-cli 时 10 类夹具网表全部正确（ERC 断言受版本门控）
- [x] 5.5 端到端验收脚本 `examples/kr1_acceptance.rs`：生成 / KiCad 可读+网表一致 / ERC 三轨，输出通过数、通过率与逐条失败原因
- [x] 5.6 运行验收并落盘报告：`kr1-report.md`。**①生成 10/10、②网表一致 10/10；③ERC 未测量**（本机 kicad-cli 7 无 ERC）→ KR1 尚未判定，阻塞项已记录

## 6. 对话式编辑（spec: conversational-edit）

- [x] 6.1 IR diff 结构 + `diff(old,new)` 实现与测试（元件增删、字段变更、网络与连接增删）
- [x] 6.2 diff 渲染为中文可读文本（含元件/引脚/网络）
- [x] 6.3 「确认后写回」：`stage` 不落盘、`confirm` 才写；未确认不落盘测试
- [x] 6.4 增量身份稳定：未改动元素 UUID 跨重写/插入保持不变（有测试）
- [x] 6.5 回退：写入前 `discard`、写入后 `revert` 恢复变更前内容（有测试）
- [x] 6.6 `gcode_apply_edit` 工具：先 `confirm=false` 出 diff，再 `confirm=true` 写回

## 7. Agent 运行时与集成（spec: agent-runtime）

- [x] 7.1 `gcode_tools::all_tools()` 注册 6 个工具（生成/ERC/读回/编辑/符号检索/工具链检查），schema 完整
- [x] 7.2 系统提示词重写：工具清单、「UNDERSTAND→SYMBOLS→NETLIST→VERIFY→REPORT」顺序、禁止直接写 s-expression、ERC 违规修法、安全边界
- [x] 7.3 隔离测试：`GCODE_HOME` 重定向 config/sessions/logs/tmp/runtime，默认 home 为 `~/.gcode` 且不在 `~/.jcode` 之下
- [x] 7.4 启动期工具链探测：打印 kicad-cli 版本与符号库路径；缺失/过低只禁用 ERC，不影响生成
- [x] 7.5 工具参数校验测试：缺必填参数返回错误而非崩溃；非法 JSON 同样

## 8. 收尾与文档

- [x] 8.1 全量 `cargo test --workspace` 通过（86 项：unit + golden + kicad-cli 集成 + 覆盖率），集成轨按环境自动跳过
- [x] 8.2 `docs/PRD-gcode.md` 附二更新 R0 状态与实测结果
- [x] 8.3 `kr1-report.md` 记录验收数据、结论与遗留问题
- [x] 8.4 `openspec validate r0-design-mvp --strict` 通过

## 本轮未能闭环的项（需外部条件）

1. **KiCad ≥ 10 的 ERC 实测**：需要 sudo 或 flatpak 安装完成；这是 KR1 判定的唯一阻塞。
2. **NL → 网表 的真实成功率**：需要在配置了 provider 的 agent 会话里跑 10 条自然语言描述。
3. **ngspice**：R1 范围，未开始。
