# RTL equivalence harness

Verifies that the tool's simulator agrees with the SystemVerilog in `hdl/` by
running randomly generated configurations through both and comparing chip
outputs tick by tick. Because the testbenches program the DUT through the real
scan chain using a bitstream exported by this tool, a pass also validates the
bitstream encoder against the RTL.

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
  CLB and CSB is random. Only output bits whose value cone is free of
  registers and floating rings are compared (the `MASK` constant, computed by
  a backward cone walk); those bits are pure combinational functions of the
  chip inputs, so they are immune to event-simulation clock glitches and to
  X-state registers. Configurations containing any structural combinational
  cycle are rejected outright (`drc::combinational_loops`): a non-inverting
  loop settles here but can still orbit forever in delta cycles, so it is not
  comparable. Both loop sources are covered — the DDIO direction gate and the
  vertical ring, which closes through the logic now that input mux c reads
  the combinational lane `v_in:0`. That rejection is why the generator needs
  ~10 attempts per usable random configuration.
- **`equiv_reg*` — registered template.** Column 0's clock is `input_0`,
  buffered through CLB(0,0)'s operation and snaked up the vertical ring to
  the CSB tap — a single-path (reconvergence-free) cone, so a lone `input_0`
  edge cannot glitch the clock. The registers of column 0 hold randomized
  logic and are observed on lane 3 of rows 0–2 at the right edge. Stimulus is
  two-phase: data inputs change only while the clock input is low, and the
  clock toggles alone. All 14 output bits are compared, exercising capture,
  the enable on horizontal lane 3, configured reset values, and the
  synchronous reset training sequence.

Comparison points: after each stimulus vector settles (and any resulting
clock edge commits), RTL state corresponds to this simulator's *post-commit*
settled state for the same tick — `SimState::view()` after `step()`. Tick 0
compares against the seeded reset state, matching the no-spurious-edge rule.

## The ring-rotation problem

A vertical ring configured as pure pass-through at every hop is a closed copy
cycle. In zero-delay event simulation, unequal values in such a cycle rotate
forever within a single time step and hit the simulator's delta iteration
limit — this happens transiently *during programming* for almost any
bitstream, because the shifting scan chain passes through arbitrary
intermediate configurations. Real hardware just settles.

The generated cases therefore ship with a companion `<case>.do` simulator
script that freezes each row-0 CLB's `vert_bus_in` port (every ring cycle
runs through it) at 0 for exactly the programming phase, then removes the
force and *deposits* the quiescent settled values computed by this simulator
(all inputs 0, registers at reset). A deposit yields to the next real driver
change, so nothing can go stale; segments whose true value differs (X-state
registers) are corrected by their drivers or masked. This is done at the
tool level because two things fail otherwise: ModelSim permanently detaches
a port-collapsed variable from its driver after an SV-side `force`/`release`,
and it refuses `force` on elements of the top-level unpacked
`vertical_buses` array (hence the per-instance packed port as the cut
point). A final configuration's floating rings are left settled at 0 —
exactly this simulator's model of them.

## Running

With ModelSim/Questa (tested with ModelSim ASE 18.1, `vsim` on this machine):

```
vlib work
vlog -sv hdl/regs/shift_reg_no_reset.sv hdl/mux/mux.sv hdl/clb/clb.sv     hdl/grid/clb_grid.sv hdl/io/io_controller.sv hdl/clock/clock_sel.sv     hdl/clock/clock_bank.sv hdl/top/sr-ga1.sv tb/equiv/equiv_*_tb.sv
vsim -c work.equiv_comb0_tb -do tb/equiv/equiv_comb0.do   # one per case
```

(`tb/equiv/run_all.do` lists the same commands for every generated case.
`hdl/clb/clb_blackbox.sv` must **not** be compiled — it defines a second,
empty `CLB`.)

Other simulators (Icarus, Verilator) can compile the same file list plus a
testbench, but need an equivalent of the `.do` script's ring freeze/deposit —
without it the programming phase trips their combinational-loop handling.
The validated flow is the ModelSim one above.

Each testbench prints `PASS <name>` or `FAIL <name>: N mismatch(es)` with a
line per mismatching tick/bit.

## Result

Last runs on 2026-08-28 with ModelSim ASE 18.1, against the dedicated carry
chain, the 4:1 input mux c (h2, h3, v0, carry_in), and the per-lane major mux
encoding (own v/h at codes 2/3, crossed-over v/h at codes 6/7):
`--comb 4 --reg 2 --ticks 40 --seed 11` → **6/6 PASS**;
`--comb 6 --reg 3 --ticks 60 --seed 99` → **9/9 PASS**.

Random configurations exercise the carry chain automatically, since input
mux c picks `carry_in` in a quarter of cells. The comparison-mask cone walk
follows the chain downward, so an output bit whose cone reaches a register
through a carry chain is masked out of the combinational class.

## Known model divergences (by design, documented in simulation.md)

- Chained register-derived clocks advance one stage per tick in this
  simulator, but ripple through in one settle in event simulation. The
  harness's registered class uses an input-derived clock, where both agree.
- Floating rings read 0 here and X (or rotating garbage) in RTL; the harness
  masks them out and pins them to 0 across programming.
- Event-simulation delta glitches can clock registers on multi-input changes;
  the harness's registered class avoids them structurally, and the tick model
  has no glitches at all.
