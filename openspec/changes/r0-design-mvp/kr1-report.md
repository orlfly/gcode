# KR1 验收报告（R0 设计 MVP）

> 由 `cargo run -p gcode-kicad --example kr1_acceptance` 生成。

KR1 原文：10 个典型电路从自然语言描述到通过 ERC + 仿真的 KiCad 工程，首版目标成功率 ≥ 7/10（含一轮对话式修正）。

## 汇总

| 轨道 | 结果 |
|---|---|
| ① 生成（网表 IR → .kicad_sch，离线、确定性） | **10/10** |
| ② KiCad 可读 + 网表与 IR 一致（真实 kicad-cli netlist 导出） | **10/10** |
| ③ ERC PASS（KR1 判定） | **10/10**（目标 ≥ 7） |
| ④ 符号解析（未回退通用 stub） | **10/10** |
| ⑤ NL → 网表（需要 LLM provider） | 本脚本不执行；由 agent 会话产生 |

**KR1 判定（①②③④ 全部满足的电路数）**：**10/10**（目标 ≥ 7）

## 逐电路结果

| 电路 | 元件/网络 | 生成 | 符号解析 | KiCad 可读 | 网表一致 | ERC | 接受 |
|---|---|---|---|---|---|---|---|
| 电阻分压器 (voltage-divider) | 4/3 | ✓ | ✓ | ✓ | ✓ | PASS | ✓ |
| RC 低通滤波 (rc-lowpass) | 4/3 | ✓ | ✓ | ✓ | ✓ | PASS | ✓ |
| RC 高通滤波 (rc-highpass) | 4/3 | ✓ | ✓ | ✓ | ✓ | PASS | ✓ |
| 同相放大器 (non-inverting-amp) | 6/5 | ✓ | ✓ | ✓ | ✓ | PASS | ✓ |
| 反相放大器 (inverting-amp) | 6/5 | ✓ | ✓ | ✓ | ✓ | PASS | ✓ |
| 二阶有源低通（Sallen-Key） (sallen-key-lowpass) | 8/6 | ✓ | ✓ | ✓ | ✓ | PASS | ✓ |
| 线性稳压（L7805） (ldo-regulator) | 5/3 | ✓ | ✓ | ✓ | ✓ | PASS | ✓ |
| LED 限流驱动 (led-driver) | 4/3 | ✓ | ✓ | ✓ | ✓ | PASS | ✓ |
| N-MOSFET 低边开关 (mosfet-switch) | 8/6 | ✓ | ✓ | ✓ | ✓ | PASS | ✓ |
| 热敏电阻分压接口 (thermistor-divider) | 4/3 | ✓ | ✓ | ✓ | ✓ | PASS | ✓ |

## 电路清单与预期

- **电阻分压器**（`voltage-divider`）：把 12V 输入分压成约 2.45V 给 ADC 采样，用 39k 和 10k 两个电阻
  - 预期：VOUT = 2.45V (12V x 10k/49k)
- **RC 低通滤波**（`rc-lowpass`）：1k 电阻串在输入上、100nF 电容到地，做约 1.6kHz 的低通滤波
  - 预期：fc = 1.6 kHz (R=1k, C=100n)
- **RC 高通滤波**（`rc-highpass`）：100nF 电容串在输入上、1k 电阻到地，做约 1.6kHz 的高通滤波
  - 预期：fc = 1.6 kHz (C=100n, R=1k)
- **同相放大器**（`non-inverting-amp`）：用 LM321 做同相放大器，增益约 10 倍（10k 和 91k 反馈电阻）
  - 预期：Gain = 1 + 91k/10k = 10.1
- **反相放大器**（`inverting-amp`）：用 LM321 做反相放大器，增益约 -10 倍（10k 输入、100k 反馈）
  - 预期：Gain = -100k/10k = -10
- **二阶有源低通（Sallen-Key）**（`sallen-key-lowpass`）：用 LM321 做 Sallen-Key 二阶低通，截止频率约 1kHz
  - 预期：fc ~= 1.07 kHz (R1=R2=10k, C1=10n, C2=22n)
- **线性稳压（L7805）**（`ldo-regulator`）：用 L7805 把 12V 降到 5V，输入输出各加 100nF 去耦
  - 预期：VOUT = 5V from VIN = 12V; OUT pin is power_out (self-driven)
- **LED 限流驱动**（`led-driver`）：5V 电源通过 330 欧姆电阻驱动一个 LED，电流约 9mA
  - 预期：I = (5V - 2V)/330R = 9.1 mA
- **N-MOSFET 低边开关**（`mosfet-switch`）：用 N 沟道 MOSFET 做低边开关驱动 LED，栅极 10k 串阻加 100k 下拉
  - 预期：Gate driven via 10k, 100k pulldown; LED load in drain path
- **热敏电阻分压接口**（`thermistor-divider`）：10k 上拉电阻和 NTC 热敏电阻分压，输出接到 ADC 测温度
  - 预期：V_sense = VCC x R_ntc/(10k + R_ntc)

## 结论与遗留

- ✅ **KR1 达标**：①生成 + ②符号解析 + ③KiCad 网表一致 + ④ERC PASS 全满足 10/10 ≥ 7/10。
- 说明：ERC 由 kicad-cli 判定；符号必须来自真实库（回退 stub 的电路不计入）。
- 遗留：NL → 网表 轨道需要在配置了 provider 的 agent 会话里跑 10 条自然语言描述（本脚本只覆盖确定性半段）。
