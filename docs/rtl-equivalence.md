# RTL equivalence harness

Verifies that the tool's simulator agrees with the SystemVerilog in `hdl/` by
running randomly generated configurations through both and comparing chip
outputs tick by tick. Because the testbenches program the `SR_GA1` top level
through the real scan chain using a bitstream exported by this tool — start bit
and all — a pass also validates the bitstream encoder, the chain order (CLBs,
IO controller, CSBs), the LUT bit order and the IO controller's muxes against
the RTL.

## Generating

```
cargo run -p fpga-core --example rtl_equiv_gen -- \
    [--fabric fabric.toml] [--out tb/equiv] [--comb 4] [--reg 2] \
    [--ticks 40] [--seed 1]
```

This writes self-checking testbenches `tb/equiv/equiv_*_tb.sv`, each carrying
an embedded bitstream (in transmission order), a stimulus schedule, the
simulator's expected outputs, and a comparison mask. Two classes:

- **`equiv_comb*` — fully random configurations.** Every config bit of every
  CLB, CSB and the IO controller is random, so random LUTs, input muxes
  (on-chip loops included) and output muxes are all exercised. Only output
  bits whose value cone is free of
  registers and floating rings are compared (the `MASK` constant, computed by
  a backward cone walk); those bits are pure combinational functions of the
  chip inputs, so they are immune to event-simulation clock glitches and to
  X-state registers. Configurations containing any structural combinational
  cycle are rejected outright (`drc::combinational_loops`): a non-inverting
  loop settles here but can still orbit forever in delta cycles, so it is not
  comparable. Every loop source is covered — the vertical ring (input mux c
  reads the combinational lane `v_in:0`), on-chip loops through the IO
  controller, and DDIO read-back. That rejection is why the generator needs
  tens of attempts per usable random configuration.
- **`equiv_reg*` — registered template.** Column 0's clock is `input_0`,
  buffered through CLB(0,0)'s LUT (`BUF_A`) and snaked up the vertical ring to
  the CSB tap — a single-path (reconvergence-free) cone, so a lone `input_0`
  edge cannot glitch the clock. The registers of column 0 hold random LUTs fed
  from random pads (never the clock) and are observed on lane 3 of rows 0–2 at
  the right edge (`output_3` muxed onto row 0, `output_5`, `output_7`). Stimulus is
  two-phase: data inputs change only while the clock input is low, and the
  clock toggles alone. All 14 output bits are compared, exercising capture,
  the enable on horizontal lane 3, configured reset values, and the
  synchronous reset training sequence.

Comparison points: after each stimulus vector settles (and any resulting
clock edge commits), RTL state corresponds to this simulator's *post-commit*
settled state for the same tick — `SimState::view()` after `step()`. Tick 0
compares against the seeded reset state, matching the no-spurious-edge rule.

## Programming, and the ring-rotation problem

A vertical ring configured as pure pass-through at every hop is a closed copy
cycle. In zero-delay event simulation, unequal values in such a cycle rotate
forever within a single time step and hit the simulator's delta iteration
limit — and during programming the shifting scan chain passes through
arbitrary intermediate configurations, so it happens for almost any bitstream.

The hardware now solves this itself. The testbench follows `hdl/top/sr-ga1.sv`:
reset is held high with `shift_data_in` low, the stream's leading start bit
sets `freeze_fabric` on the first shift clock, and while frozen the top level
gates the vertical rings (and the chip outputs, and the clock bank's coupling
wrap) to 0. Every ring is cut for the whole of programming. Releasing reset
clears the freeze, and the rings come up from a consistent all-zero state that
settles. The old ModelSim `force`/deposit workaround is gone; each case's
`.do` script is just `run -all`.

A final configuration's floating rings stay at 0 — exactly this simulator's
model of them.

## Running

With ModelSim/Questa (tested with ModelSim ASE 18.1, `vsim` on this machine):

```
vsim -c -do tb/equiv/run_all.do                            # compiles everything
vsim -c work.equiv_comb0_tb -do tb/equiv/equiv_comb0.do   # then one per case
```

`run_all.do` compiles `hdl/` and every testbench into `work`, then starts the
first case; each case's own `.do` ends with `quit -f`, so run the cases one
`vsim` at a time. `hdl/clb/clb_blackbox.sv` must **not** be compiled — it
defines a second, empty `CLB`.

Other simulators (Icarus, Verilator) can compile the same file list plus a
testbench; with the freeze in the RTL they no longer need a ring workaround,
but the validated flow is the ModelSim one above.

Each testbench prints `PASS <name>` or `FAIL <name>: N mismatch(es)` with a
line per mismatching tick/bit.

## Result

Last runs on 2026-10-12 with ModelSim ASE 18.1, against the LUT3 CLB, the 8×4
grid and the configurable IO controller, programmed through the full 889-bit
stream: `--comb 6 --reg 4 --ticks 40 --seed 3` → **10/10 PASS**;
`--comb 20 --reg 6 --ticks 60 --seed 11` → **26/26 PASS**.

Random configurations exercise the carry chain automatically, since input
mux c picks `carry_in` in a quarter of cells. The comparison-mask cone walk
follows the chain downward, so an output bit whose cone reaches a register
through a carry chain is masked out of the combinational class.

## Known model divergences (by design, documented in simulation.md)

- Chained register-derived clocks advance one stage per tick in this
  simulator, but ripple through in one settle in event simulation. The
  harness's registered class uses an input-derived clock, where both agree.
- Floating rings read 0 here and X (or rotating garbage) in RTL; the harness
  masks them out, and the freeze holds them at 0 across programming.
- Event-simulation delta glitches can clock registers on multi-input changes;
  the harness's registered class avoids them structurally, and the tick model
  has no glitches at all.
