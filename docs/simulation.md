# Simulation semantics

How the tool's tick-based simulator models the SR-GA1 fabric, and where that
model deliberately differs from silicon. The implementation lives in
`crates/fpga-core/src/sim.rs`.

## The tick

Each tick executes in this exact order, and no other:

1. **Apply stimulus.** Every chip input reads element `tick mod len` of its
   pattern list (a single-element list is a constant, `[0,1]` is a clock).
   A missing pattern reads as constant 0. DDIO inputs are additionally gated:
   while the corresponding `ddio_dir` output is 1, the input is forced to 0 —
   this gate is part of the combinational network, not of stimulus application.
2. **Settle combinational logic to a fixed point**, using the register values
   held from the end of the previous tick.
3. **Resolve every CSB's clock** from that settled state, following the
   couple chain and vertical ring taps. A chain coupled in a full circle has
   no source and resolves to constant 0.
4. **Detect edges**: each column's resolved clock is compared against its
   value sampled at the same point on the previous tick; a 0→1 transition is
   a rising edge.
5. **Commit registers simultaneously** for every column whose clock rose,
   using the settled values from step 2: if reset is asserted the FF loads its
   configured `op_ff_reset_val`; otherwise, if its enable (horizontal lane 3
   at that CLB) settled to 1, it captures the settled operation result.

Nothing clock-dependent changes until the end of the tick. The settled state
returned for a tick therefore shows the *pre-commit* register values; a
register's new value is first visible in the following tick's settled state.

**Tick 0** applies stimulus element 0 and settles, but fires no edges: each
column's previous-clock sample is seeded with its tick-0 resolved value, so a
stimulus that begins at 1 does not produce a spurious edge. Registers start
tick 0 holding their configured reset values — no explicit reset stimulus is
needed; the intended workflow is reset once, then run.

## Settling

Combinational settling is an in-place (Gauss–Seidel) sweep over every bus
segment and CLB in fabric order, repeated until a full sweep changes nothing.
Cells are visited column by column with rows ascending, which is the carry
chain's direction, so a whole column's ripple carry resolves within a single
sweep.
The iteration cap is the total segment count plus a margin — comfortably more
than the longest possible pass-through chain, so any genuine fixed point is
reached. All values start each settle at 0.

Two consequences of the fixed-point-from-zero policy:

- **Floating pass-through rings settle at 0.** A vertical ring configured as
  pure pass-through at every hop has no driver; any constant would be
  self-consistent, and the simulator deterministically picks 0. The naming
  engine shows such rings as `<unconnected>`, and DRC warns about them.
- **Oscillating loops are detected, never hung on.** If the cap is exceeded,
  simulation stops with a `CombinationalLoop` error naming the blocks whose
  outputs were still changing. There are two ways to build one: the DDIO
  direction feedback (`ddio_dir` gating `ddio_in`), and the vertical ring — input mux c can read `vert_bus_in[0]`, which carries
  `operation_result`, so a ring configured as pass-through can feed a CLB's
  own output back into its logic. Horizontal buses still flow strictly
  left→right, and the carry chain runs strictly upward within a column
  without wrapping, so neither of those can close a cycle on its own.

  A cycle does not always oscillate — a non-inverting loop reaches a fixed
  point and settles — but it is never intentional, and event simulators can
  spin on one even where a fixed point exists. `drc::combinational_loops`
  reports every structural cycle regardless of whether it settles.

## Derived clocks advance one stage per tick

There is no external clock pin on this chip: every column clock is derived
from a fabric signal (via the CSB's ring tap) or from another CSB. Because
registers commit at the end of a tick, a CSB tapping a register's output sees
the new value on the *following* tick. Chained derived clocks therefore
advance one stage per tick rather than rippling through in a single period as
they would in silicon.

This is deterministic and self-consistent, but it does **not** model clock
skew or the race behaviour of a real ripple clock. Columns clocked by
divider-style chains will run correctly but later (in ticks) than a
skew-accurate model would predict. DRC flags every column that runs live
registers on a fabric-derived clock as an informational notice for exactly
this reason.

## Reset

Reset is a level, asserted manually during a run (or implicit in the tick-0
starting state). It is synchronous, per the RTL: it takes effect in each
column on that column's next rising clock edge, wins over the FF enable, and
loads the per-CLB configured reset value. Columns in different clock domains
therefore come out of reset on different ticks — visible in the waveform, and
correct.

## Multiple clock domains

Columns may legitimately run in different clock domains; this is a supported
design style, not an error. Edge detection and commit are evaluated per
column against the same settled state, so cross-domain register transfers are
deterministic at tick granularity (real silicon would add metastability risk
the simulator does not model).

## Agreement with the RTL

The randomized RTL-equivalence harness (this simulator vs. Verilator/Icarus
running the SystemVerilog in `hdl/`) is described in `docs/rtl-equivalence.md`
once generated; if neither tool is installed, the harness generates
self-contained testbenches and documents how to run them.
