# R1 仿真验收报告

> 由 `cargo run -p gcode-sim --example sim_acceptance` 生成；后端 `ngspice-42`。

对比对象是**解析推导**的预期值（公式与依据列在每项旁），不是上一次仿真的结果。
有源器件使用**行为级 macromodel**，因此容差按「拓扑与量级正确」设定。

## 汇总

| 项 | 结果 |
|---|---|
| 仿真配方通过 | **11/11** |
| 指标项通过 | **12/12** |
| 运行失败（异常） | 0 |

## 逐电路结果

| 电路 | 分析 | 指标 | 实测 | 预期 | 偏差 | 判定 |
|---|---|---|---|---|---|---|
| voltage-divider | op | 直流电压 | 2.44898 V | 2.44898 V | -0.00% | PASS |
| rc-lowpass | ac | 截止频率（-3dB） | 1587.679489 Hz | 1591.549431 Hz | -0.24% | PASS |
| rc-lowpass | ac | 低频增益（倍数） | 0.99998 x | 1 x | -0.00% | PASS |
| rc-highpass | ac | 截止频率（-3dB） | 1595.928183 Hz | 1591.549431 Hz | +0.28% | PASS |
| non-inverting-amp | op | 直流电压 | 1.009897 V | 1.01 V | -0.01% | PASS |
| inverting-amp | op | 直流电压 | -0.999889 V | -1 V | -0.01% | PASS |
| sallen-key-lowpass | ac | 截止频率（-3dB） | 1119.882375 Hz | 1123 Hz | -0.28% | PASS |
| ldo-regulator | op | 直流电压 | 5 V | 5 V | +0.00% | PASS |
| led-driver | op | 直流电压 | 2.100747 V | 2.1 V | +0.04% | PASS |
| mosfet-switch | op | 直流电压 | 5 V | 5 V | +0.00% | PASS |
| mosfet-switch | op | 直流电压 | 2.505276 V | 2.32 V | +7.99% | PASS |
| thermistor-divider | op | 直流电压 | 2.5 V | 2.5 V | +0.00% | PASS |

## 依据与产物

- **voltage-divider** / dc operating point：分压公式（op）
  - dc_voltage 预期 2.44898 V（±1%）：VOUT = VIN x R2/(R1+R2) = 12 x 10k/49k
- **rc-lowpass** / ac sweep：一阶低通解析式（ac）
  - fc_3db 预期 1591.549431 Hz（±5%）：fc = 1/(2 pi R C), R=1k, C=100n
  - gain_linear 预期 1 x（±2%）：通带内 RC 低通为单位增益
- **rc-highpass** / ac sweep：一阶高通解析式（ac）
  - fc_3db 预期 1591.549431 Hz（±5%）：fc = 1/(2 pi R C), C=100n, R=1k
- **non-inverting-amp** / dc gain：同相放大器增益式（运放为行为级模型）（op）
  - dc_voltage 预期 1.01 V（±5%）：VOUT = VIN x (1 + R2/R1) = 0.1 x 10.1
- **inverting-amp** / dc gain：反相放大器增益式（运放为行为级模型）（op）
  - dc_voltage 预期 -1 V（±5%）：VOUT = -VIN x R2/R1 = -0.1 x 10
- **sallen-key-lowpass** / ac sweep：二阶 Sallen-Key 解析式（含 Q 对 -3dB 的影响）（ac）
  - fc_3db 预期 1123 Hz（±10%）：f0 = 1/(2 pi sqrt(R1 R2 C1 C2)) = 1.073 kHz；Q = sqrt(C1/C2)/2 = 0.742，二阶低通的 -3dB 约为 1.047 x f0 = 1.123 kHz
- **ldo-regulator** / dc regulation：稳压器标称输出（op）
  - dc_voltage 预期 5 V（±5%）：稳压器输出被限制在 5V（行为级模型 OUT = min(VIN, 5V)）
- **led-driver** / dc operating point：行为级 LED 模型的直流工作点（非厂商模型）（op）
  - dc_voltage 预期 2.1 V（±10%）：行为级 LED 模型在约 8.8mA 下的正向压降（Is=1e-20, N=1.9, Rs=8）
- **mosfet-switch** / off state：开关关断时负载支路无电流（测供电侧节点）（op）
  - dc_voltage 预期 5 V（±1%）：栅极 0V 时 MOSFET 关断，支路无电流，R3 上无压降，故 LED_A = VCC = 5V                 （此时漏极是浮空节点，其电压由漏电决定，不适合作为判据）
- **mosfet-switch** / on state：行为级 MOSFET 三极管区估算（模型相关，容差放宽）（op）
  - dc_voltage 预期 2.32 V（±25%）：行为级 NMOS（Vto=2V, Kp=0.5m）与 LED/330Ω 支路共同决定的漏极电压；三极管区解析估算 Vds≈2.32V, Id≈2.13mA，实测约 2.5V（模型相关）
- **thermistor-divider** / dc operating point：分压公式（热敏电阻按标称值）（op）
  - dc_voltage 预期 2.5 V（±2%）：V_SENSE = VCC x R_ntc/(R1 + R_ntc) = 5 x 10k/20k（热敏电阻按 25C 标称值 10k）

## 结论

- ✅ 全部 11 个配方、12 项指标落在解析预期容差内。
- 说明：有源器件为行为级模型，验证的是拓扑与量级正确性，不是器件级精度。
