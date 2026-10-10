## Context

见 `proposal.md - Why`。当前 R0 已有可运行骨架，本设计只描述在既有骨架上要做的架构决策与改动方向。

现状（`crates/`）：

- `gcode-kicad::sexpr`：s-expression 解析/写出器，尚无真实 KiCad 工程覆盖率实测。
- `gcode-kicad::netlist`：`NetlistIr`（components/nets）与校验，仅有分压模板。
- `gcode-kicad::schematic::generate_schematic`：确定性生成 `.kicad_sch`，但存在两个关键问题——
  - `lib_symbol_stub` 只生成「通用两脚方框」符号，不加载真实 KiCad 符号库，因此引脚名/引脚号/电源脚语义丢失，ERC 无法得到有意义的结果；
  - UUID 由随机 `uuid_v4()` 生成，破坏「同 IR 同输出」的确定性要求。
- `gcode-kicad::erc`：调用 `kicad-cli sch erc` 并解析 JSON，但只有 `summary()`，没有解读与修复建议。
- `gcode-tools`：仅 `gcode_generate_schematic`、`gcode_run_erc` 两个工具。
- `gcode-agent`：`gcode` 二进制 + `GCODE_HOME` 隔离 + 系统提示词覆盖。

约束：

- 上游 `jcode` 为 path 依赖且只读（`/opt/workspace/jcode`），只能在 `gcode-*` crate 与二进制入口内扩展。
- 目标格式为 **KiCad 10**；本机尚未安装 `kicad-cli`，CI/开发机需要环境门控。
- 无 GUI 插件；文件系统是 agent 与 KiCad GUI 的同步媒介。

## Goals / Non-Goals

**Goals:**

- 让「NL → NetlistIr → `.kicad_sch` → ERC → 修正」闭环在 10 类典型电路上可自动验收（`circuit-catalog`）。
- 生成物在 **确定性与 ERC 有效性**两方面达标：同 IR 字节级一致；ERC 能识别真实网络（依赖真实符号定义）。
- ERC 违规产出可读解读与可执行修复建议，并由 agent 循环消费（`erc-verification`）。
- 修改以 diff 呈现、确认后增量写回（`conversational-edit`）。

**Non-Goals:**

- 不做仿真（R1）、实测（R1.5）、Gerber/激光 G-code（R2）、PCB 布局布线。
- 不追求最优自动布局（只要确定性、不重叠、可读）；自动走线优化后置。
- 不引入 KiCad IPC/Python 依赖；R0 只用文件格式 + `kicad-cli`。
- 不在 R0 做元件库全量索引；先解决「已知常用元件能取到真实符号定义」。

## Decisions

### D1 · IR 为唯一契约，LLM 不接触 s-expression

沿用 PRD §7.5 的核心赌注：`NetlistIr` 是 LLM 与确定性代码之间唯一边界。

- 备选：让 LLM 直接产出 `.kicad_sch` 文本。否决：错误率不可控、无法确定性校验。
- 影响：`netlist-ir` 的校验必须是**生成前硬门禁**，且校验错误信息要能被 LLM 直接用于自我修正（指出组件/网络/引脚）。

### D2 · 真实符号定义优先，stub 仅兜底

ERC 的语义依赖符号里的引脚定义（引脚号、`power_in`、`input/output` 等）。因此：

- 新增符号解析层：从本地 KiCad 符号库（`*.kicad_sym`，路径可由 `GCODE_KICAD_SYMBOL_DIR` 覆盖）按 `lib_id` 取出真实符号定义并嵌入生成的 `lib_symbols`。
- 取不到时回退到现有通用 stub，并在工具输出中**显式警告**（避免静默产出无效 ERC 结果）。
- 备选：手工维护精简符号表。否决：覆盖率与维护成本都不可接受；备选：调用 KiCad IPC。否决：R0 不引入 UI/IPC 依赖。

### D3 · 确定性标识由内容派生

放弃随机 UUID，改为**内容派生的确定性 UUIDv5**：种子 = `lib_id` / `reference` / `pin` / `net name` + 稳定序号。

- 满足 `netlist-ir::确定性输出` 与 `schematic-generation::输出可复现`。
- 副作用：增量编辑时未改动元素的 UUID 天然稳定，直接支撑 `conversational-edit::增量编辑`。

### D4 · 网络用全局标签表达，R0 不做导线布线

现有实现用 global label 连接网络点。R0 保留该策略：确定性、实现简单、ERC 可识别。

- 备选：真实导线布线。否决：需要几何布局与冲突消解，收益低、风险高；后置为优化项（R0 之后）。

### D5 · ERC 解读用确定性映射表，LLM 只做兜底措辞

- 维护 `violation type → 中文解释 + 修复建议模板` 的静态表（可测试、可回归）。
- 未知类型走兜底：输出原始 `description` + 通用排查建议，不丢信息。
- LLM 的角色是在对话中把结构化结论组织成建议，**不让 LLM 决定 PASS/FAIL**（判定必须来自 ERC 报告）。

### D6 · 编辑以 IR diff 为中间表示

- 修改一律表达为「旧 IR → 新 IR」的结构化 diff（组件/取值/网络连接增删改）。
- diff 经用户确认后，才用 D3 的确定性 UUID + 增量写回策略更新文件。
- 实现上优先在 IR 层计算 diff，再对 `.kicad_sch` 做**增量**更新；无法增量时回退整体重写，但必须保持未改动元素标识稳定。

### D7 · 电路目录 = 夹具 + 可选 LLM 验收双轨

- **离线轨**：10 类电路的确定性夹具（IR + 预期产物摘要 + ERC 预期），无需 LLM/KiCad即可跑结构回归。
- **在线轨**：NL 描述 → agent → 生成 → ERC 的端到端验收，产出 KR1 通过率；依赖 `kicad-cli` 与 provider。
- 两轨分离，保证 CI 默认可离线运行。

### D8 · 工具链门控

- 探测顺序：`GCODE_KICAD_CLI` → `PATH` 上的 `kicad-cli`。
- 解析 `kicad-cli version`，低于 KiCad 10 最低版本时给出明确提示并**拒绝执行 ERC**（不 panic）。
- 需求 `agent-runtime::工具链探测与版本门控`。

### D9 · 保持 jcode 上游只读

- 所有扩展落在 `gcode-kicad` / `gcode-tools` / `gcode-agent`。若需要 jcode 未暴露的能力（如工具确认 UI），优先用现有 `Tool`/权限机制实现，必要时再评估最小化上游改动。

## Risks / Trade-offs

- [KiCad 10 文件格式 token 与 `generator_version` 需与真实 `kicad-cli` 对齐，猜错会导致文件打不开] → 第一批任务先在真机安装 KiCad 10，用 `kicad-cli` 打开/导出验证生成物；把版本 token 做成常量并加集成测试。
- [符号库路径与格式（`*.kicad_sym`）随安装方式变化] → `GCODE_KICAD_SYMBOL_DIR` 覆盖 + 多候选路径探测 + 取不到时降级并警告；集成测试用真实库跑通至少 R/C/LM358/78xx/MOSFET 等常用件。
- [ERC 结果依赖完整符号定义，stub 下 ERC 可能「假通过」或「假报错」] → D2 的真实符号优先 + 未知符号显式警告；验收以集成（真实库 + kicad-cli）为准。
- [无 kicad-cli 的环境无法跑集成测试] → 集成测试按环境探测自动跳过并输出提示；离线结构回归始终可跑。
- [在线 KR1 验收受 LLM 非确定性影响] → 固定提示词与温度、允许至多一轮修正、记录逐条失败明细；把结果落盘为可复现报告。
- [增量写回实现复杂，可能破坏文件稳定] → 先实现「IR diff + 整体重写但 UUID 稳定」的可用版本，再迭代到真正增量；以「未改动元素标识稳定」作为验收点而非实现细节。

## Migration Plan

1. **环境就绪**：安装 KiCad 10（含 `kicad-cli`），验证 headless ERC 可用；记录版本 token。
2. **解析器实测**：拿开源 KiCad 工程跑 `sexpr` 覆盖率，确定是否需扩展语法（PRD 附录待验证项 2）。
3. **生成器修复**：D2 真实符号嵌入 + D3 确定性 UUID；用 `kicad-cli` 回读验证。
4. **ERC 解读**：D5 映射表 + 循环；集成测试覆盖典型违规。
5. **电路目录**：D7 双轨夹具与验收脚本。
6. **对话式编辑**：D6 diff + 确认 + 增量写回。
7. **回滚策略**：各步骤独立提交；生成物为纯文本文件，回归失败可退回上一提交的生成器实现，不影响既有工程文件。无数据迁移。

## Open Questions

- 10 类电路的具体取值/拓扑清单（本设计按 `circuit-catalog` 的 10 类为准，逐类取值可在实现时按标准 E24 系收敛，不影响架构）。
- ERC 解读表是否需要覆盖 KiCad 10 新增的违规类型全集（可先覆盖高频类，按实测补全）。
- 是否把 KR1 在线验收做成 CI job（依赖 provider 凭据，可能长期保持手动触发）。
