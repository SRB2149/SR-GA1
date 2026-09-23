# Supported SystemVerilog subset — `sr-ga1-synth`

This is the contract the synthesiser enforces. Anything outside it is
rejected with a diagnostic naming the construct, the file and the line.
Nothing outside it is silently mapped: on a fabric this constrained, a
plausible-looking wrong bitstream is far worse than a refusal.

Enforcement runs in two layers, both in `crates/sr-ga1-synth/src/subset.rs`:

- a **source scan**, which catches constructs Yosys would accept and quietly
  ignore (`initial`, delays, assertions, system tasks) and can point at the
  exact line; and
- a **netlist check** on the elaborated design, which is authoritative: it
  sees what a construct actually *became*, so an inferred latch is caught
  however it was written. Yosys' `src` attributes carry the location back.

Anything neither layer rejects is left to Yosys. If Yosys cannot map it,
Yosys says so.

## Supported

| Area | Detail |
|---|---|
| Structure | `module`, ports, instantiation and hierarchy (flattened before mapping) |
| Types | `logic`, `wire`, `reg`; packed vectors; bit and part selects; concatenation and replication |
| Parameters | `parameter`, `localparam`, `generate`/`for` with `genvar` |
| Combinational | continuous `assign`; `always_comb` and `always @(*)` with blocking assignment, `if`/`else`, `case`; combinational `function` |
| Operators | bitwise, logical, reduction, comparison, shift, ternary; `+` and `-`; `*`, `/`, `%` **only with constant operands** |
| Sequential | `always_ff @(posedge clk)` with non-blocking assignment and synchronous reset |
| Constant functions | `$clog2`, `$bits`, `$size`, `$left`, `$right`, `$low`, `$high`, `$signed`, `$unsigned`, `$increment`, `$dimensions`, `$unpacked_dimensions` |

Multiple packed dimensions (`logic [3:0][1:0] x`) are accepted — they flatten
to a vector, which the fabric can route. It is *unpacked* dimensions that are
rejected.

## Rejected, and why

| Construct | Hardware reason |
|---|---|
| Unpacked arrays (`logic x [0:3]`) | The fabric has no memory; every net is a single routed bit. |
| Asynchronous reset | Reset is sampled on each column's clock. Config bit 19 holds the reset value and there is no asynchronous path. |
| Inferred latches | The CLB has one edge-triggered flip-flop and no latch. Complete every branch, or give the signal a default. |
| Tri-state, `inout`, `tri*`, `wand`, `wor`, `supply*` | Every lane segment has exactly one driving mux, so multi-driver conflicts are impossible by construction — and so is tri-state. DDIO pins are declared in the constraints file instead. |
| `*`, `/`, `%` with non-constant operands | The only arithmetic primitive is the per-column carry chain, four cells long. A general multiplier does not fit. |
| `initial` | Describes simulation-time behaviour. The configuration shift registers (`Shift_Reg_No_Reset`) are not reset, so power-on state is undefined in silicon. |
| Delays (`#5`) | Simulation-only; the fabric has no timing model. `#(...)` parameter overrides are fine. |
| `task` | Not synthesisable in this subset; use a combinational `function`. |
| Assertions (`assert`, `assume`, `cover`, `property`, `sequence`) | No hardware counterpart. |
| System tasks and functions outside the constant list | Simulation-only. |
| `interface`, `class`, `package`, `program`, `modport` | The subset is flat modules only. |
| `negedge`, dual-edge logic | Every CLB flip-flop is rising-edge triggered on its column clock; there is no edge-select bit. |
| `always_latch`, `forever`, `fork`/`join`, `wait`, `force`/`release` | No synthesisable meaning here. |
| `real`, `realtime`, `shortreal` | The fabric carries single bits. |

## Fabric-specific requirements

Beyond the language subset, three things the RTL shape forces:

1. **Exactly one reset net, synchronous, with constant reset values.** A
   non-constant reset value has nowhere to live: config bit 19 holds a
   constant. More than one reset net is an error — the chip has one global
   reset pin.
2. **Reset must take priority over the enable**, matching the CLB's
   `if (reset) ... else if (horz_bus_in[3]) ...`. A register written the
   other way round (enable gating the reset) is normalised by Yosys where
   that is safe, and rejected where it is not — an `$sdffce` surviving
   elaboration is an error naming the register.
3. **Clock nets must originate at a chip input.** The chip's only clock pin
   is `shift_clk`, which programs the configuration chain. Every fabric
   clock is derived: a signal is routed from a pad, through at least one CLB
   (the only horizontal-to-vertical path), onto a vertical ring, where the
   column's CSB taps it. A clock that is not traceable to an input pad is an
   error.

## DDIO pins are never inferred

The tool does not try to deduce bidirectional behaviour from SystemVerilog.
A design uses a DDIO pin only by declaring it in the constraints file, naming
the input net, the output net and the net driving `ddio_dir`. Those are then
routed to `ddio_in_n`, `ddio_out_n` and row 3's `ddio_dir_n` output lane.

An unconstrained DDIO pin is left at its default with its input path gated
off. A design that references a DDIO net without a matching constraint gets
a warning, not a silent connection.

## How registers are mapped

Yosys' `dfflegalize` is given exactly one target shape:

```
dfflegalize -cell $_SDFFE_PP?P_ x
```

That is: rising-edge clock, active-high synchronous reset that wins over the
enable, active-high enable, either reset value (`?`), and no required
power-on value (`x`) — which is honest, because the configuration shift
registers have none.

Every register is then fused into the cell that computes its data, because
the flip-flop's data input is hard-wired to its own CLB's `operation_result`.
Since `_op` and `_reg` are separately routable, that fusion is free even when
the driving cell has other consumers. A register whose data comes from a chip
input, from another register, or from a cell that already carries one needs
an extra buffer CLB, and the report says so.
