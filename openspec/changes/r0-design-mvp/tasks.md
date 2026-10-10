## 1. 环境与前置验证（解除阻塞）

- [ ] 1.1 在开发机安装 KiCad 10（含 `kicad-cli`）；记录 `kicad-cli version` 输出与安装路径
- [ ] 1.2 用一个手写最小 `.kicad_sch` 跑通 `kicad-cli sch erc --format json`，确认 headless 可用并保存样例 JSON 报告到 `crates/gcode-kicad/tests/fixtures/`
- [ ] 1.3 用 `kicad-cli sch export netlist` / 打开向导验证当前 `generate_schematic` 产物能否被 KiCad 10 识别；记录真实文件格式 token 与 `generator_version`（回填 `schematic.rs` 常量）
- [ ] 1.4 采集 20 个开源 KiCad 工程，跑 `sexpr` 覆盖率脚本，输出解析成功率与失败语法清单（PRD 附录待验证项 2）
- [ ] 1.5 依据 1.2/1.3 结果把 PRD 附录「R0 技术验证清单」逐项回填结论到 `docs/PRD-gcode.md`

## 2. 网表 IR 契约（spec: netlist-ir）

- [ ] 2.1 为 `NetlistIr` 反序列化补必填字段校验（缺 `lib_id`/`name` 返回指名字段的错误）并加测试
- [ ] 2.2 扩展 `IrError` 覆盖空位号、空引脚号、重复位号、未知组件、空网络五类，错误信息包含组件/网络/引脚定位
- [ ] 2.3 在 `gcode_generate_schematic` 工具入口强制先 `validate()`，并把校验错误原文返回给 agent；加「校验失败不落盘」测试
- [ ] 2.4 让 `to_sexpr`/`pin_nets` 对输入顺序不敏感（连接排序），加等价 IR 字节级一致测试
- [ ] 2.5 为 10 类电路补齐拓扑模板（见 5.1），每个模板产出 IR 必须通过 `validate()`

## 3. 原理图生成器（spec: schematic-generation）

- [ ] 3.1 新增符号库解析模块：定位 KiCad `*.kicad_sym`（`GCODE_KICAD_SYMBOL_DIR` 覆盖 + 多候选路径），按 `lib_id` 解析真实符号定义
- [ ] 3.2 生成器改用真实符号定义嵌入 `lib_symbols`；解析不到时回退现有 stub 并显式输出警告
- [ ] 3.3 引入内容派生的确定性 UUID（uuidv5：`lib_id`/`reference`/`pin`/`net` + 稳定序号），替换随机 `uuid_v4()`
- [ ] 3.4 加「同 IR + 同布局两次生成字节级一致」测试（覆盖组件、网络、UUID、lib_symbols 顺序）
- [ ] 3.5 布局保证不重叠：网格/列布局校验，加重叠检测测试（含 10 类电路规模）
- [ ] 3.6 加回读保真测试：生成 → `Sexpr::parse` → 组件位号集合与网络名集合与输入 IR 一致
- [ ] 3.7 用真实 `kicad-cli` 加集成测试：生成的 `.kicad_sch` 可被 KiCad 10 读取且 ERC 无解析错误（无 kicad-cli 时自动跳过并提示）

## 4. ERC 解读与自查循环（spec: erc-verification）

- [ ] 4.1 `kicad-cli` 探测与版本门控：`GCODE_KICAD_CLI` → `PATH`，解析版本，低于 KiCad 10 最低版本时返回明确错误且拒绝执行
- [ ] 4.2 扩展 `ErcReport`：区分 error/warning、统计违规/未连接/一致性项，输出稳定可测的汇总结构
- [ ] 4.3 建 `violation type → 中文解释 + 修复建议模板` 静态映射表（覆盖高频类型），每条建议包含涉及元件/网络占位
- [ ] 4.4 实现未知违规类型兜底：保留原始 `description` + 通用排查建议
- [ ] 4.5 新增自查循环能力（生成→ERC→修正→重生成），带最大轮数上限；以 PASS 结束或在上限处列出残留违规，加两向测试
- [ ] 4.6 暴露 `gcode_interpret_erc`（或扩展现有 ERC 工具输出）使解读与人话建议进入 agent 上下文

## 5. 电路目录与 KR1 验收（spec: circuit-catalog）

- [ ] 5.1 定义 10 类电路目录（分压器、RC 低通、RC 高通、同相放大器、反相放大器、Sallen-Key 低通、LDO/78xx 稳压、LED 限流、MOSFET 开关、传感器分压接口），含标准 E24 取值与用途描述
- [ ] 5.2 为每类电路建立确定性夹具（IR + 预期产物摘要 + 预期 ERC 结论）存放于 `crates/gcode-kicad/tests/fixtures/circuits/`
- [ ] 5.3 加离线回归测试：遍历 10 类夹具生成并通过结构断言与回读断言，无 LLM/无 kicad-cli 依赖
- [ ] 5.4 加集成测试：有 kicad-cli 时 10 类夹具全部 ERC PASS
- [ ] 5.5 实现「NL 描述 → 生成 → ERC」端到端验收脚本（允许至多一轮修正），输出通过数/通过率/逐条失败原因
- [ ] 5.6 运行 KR1 验收并落盘报告（目标 ≥ 7/10）；未达标则记录失败明细并回到 2.x/3.x 迭代

## 6. 对话式编辑（spec: conversational-edit）

- [ ] 6.1 定义 IR diff 结构（组件增删、取值变更、网络连接增删改）与 `diff(old, new)` 实现及测试
- [ ] 6.2 把 diff 渲染为可读文本（含涉及的元件/引脚/网络）并加渲染测试
- [ ] 6.3 实现「确认后写回」：未确认时不触碰文件；加未确认不落盘测试
- [ ] 6.4 借助确定性 UUID 实现增量写回：仅改动元素变化，未改动元素标识与连接保持稳定；加稳定性测试
- [ ] 6.5 实现回退：写入前放弃与写入后恢复变更前内容；加回退测试
- [ ] 6.6 暴露编辑工具（如 `gcode_apply_edit`）把 diff → 确认 → 写回接入 agent 流程

## 7. Agent 运行时与集成（spec: agent-runtime）

- [ ] 7.1 扩充 `gcode_tools::all_tools()` 注册 3.x/4.x/6.x 新增工具并保持 schema 描述完整
- [ ] 7.2 更新 `GCODE_SYSTEM_PROMPT`：明确「理解→网表→验证→报告」顺序、禁止直接写 s-expression、ERC 解读的使用方式
- [ ] 7.3 加隔离测试：设置 `GCODE_HOME=/x/y` 启动后所有持久化写入落在该目录，且不触碰 `~/.jcode`
- [ ] 7.4 加启动期工具链探测：无 kicad-cli 时给出可操作提示但不影响非 ERC 功能
- [ ] 7.5 加工具参数校验测试（缺失必填参数返回错误而非崩溃）

## 8. 收尾与文档

- [ ] 8.1 跑全量 `cargo test --workspace` 与集成测试，确认离线轨全绿、集成轨按环境跳过逻辑正确
- [ ] 8.2 更新 `docs/PRD-gcode.md` 的 R0 状态与 KR1 实测结果
- [ ] 8.3 在 `openspec/changes/r0-design-mvp/` 记录 KR1 验收报告链接与遗留问题
- [ ] 8.4 归档变更前确认：`openspec validate r0-design-mvp --strict` 通过
