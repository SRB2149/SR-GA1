# SR-GA1 fabric notes

The architectural facts that shape `sr-ga1-synth`, and the reasoning behind the
decisions they forced. `docs/rtl-analysis.md` is the authoritative description of
the hardware; this is about what it means for a synthesiser.

## Every CLB is a 3-input LUT

The CLB computes `lut[{a, b, c}]` over three selected inputs, with input `a` as
the most significant index bit. Any function of up to three inputs is one CLB,
so technology mapping is LUT-count minimisation.

The tool still hands ABC a genlib rather than asking for LUT mapping, because the
library carries something LUT mapping does not: for each function, every
*placement* of its pins on the physical inputs (`src/genlib.rs`). That matters
because the three inputs see different lanes:

| input | sees |
|---|---|
| `a` | horizontal lanes 0, 1 |
| `b` | horizontal lanes 1, 2 |
| `c` | horizontal lanes 2, 3, vertical lane 0, the carry chain |

So where a pin sits decides what can feed it, and placement chooses the
placement along with the position. The LUT value for each placement is computed
from the fabric's own index order, so nothing assumes which input is the most
significant. An input a function ignores is a don't-care: nothing is tied off.

The library is every function of one, two or three pins that depends on all of
them — 2 + 10 + 218 gates. Full-width functions take the `fabric.toml` preset
name when one matches (`AND3`, `XOR3`, `MUX2`, ...).

A buffer is a LUT copying whichever input the signal arrived on. That costs a CLB
and nothing else, which is why the router can spend one wherever a cell is free.

## Constants come only from major muxes, and never into column 0

A constant on a horizontal lane comes from the major mux (lanes 2/3) of the CLB
to the **left**. The minor lanes never carry one, and at column 0 there is no CLB
to the left: the IO controller brings in chip inputs and read-back outputs,
never constants.

With a LUT, logic never needs a constant — a function ignores the inputs it does
not read. The two consumers left are:

- **Register enables.** `horz_bus_in[3]` gates the flip-flop, so a register that
  captures every edge needs a constant 1 on lane 3, from the major mux to its
  left. In column 0, lane 3 is a chip input (`input_3`, `input_7`, `ddio_in_1`,
  `input_9` by row), so an always-enabled register there is impossible; the
  placer treats it as infeasible. `Fabric::constant_reaches_lane` is the one
  predicate for this.
- **Unused DDIO directions,** which are tied to 0 so the pad stays an input
  (below).

## Lane 3 is both a wire and a register enable

The routing graph models `HSeg(row, col, 3)` as **one node**: holding net `X`
means both "X is routed here" and "this CLB's enable is X". Single occupancy then
resolves "route a signal past" against "hold this register enabled"
automatically, instead of one silently breaking the other.

## The IO controller is routing

The left edge is not hard-wired any more. Each row's lanes 0 and 1 come from a
16:1 input mux over the ten chip inputs, the two DDIO pads, and `output_0..3`
read back; lanes 2 and 3 are fixed pins (`input_2/3`, `input_6/7`, the DDIO pads,
`input_8/9`). On the right, `output_0..3` and the DDIO out and direction pads
each pick any right-edge lane; `output_4..9` are fixed to lanes 2/3 of rows 1–3.
The controller has its own 64 configuration bits, between the CLBs and the CSBs
in the chain.

The routing graph has a node per pad and an edge per mux choice, each edge
carrying its IO configuration write, so the router picks lanes for pins exactly
as it picks everything else. Placement still chooses *which* pad a signal uses,
pricing pads by the rows and lanes they can reach: a fixed pin is preferred when
it lands where the signal is wanted, since a muxed pin spends one of only eight
input-mux lanes, and those lanes are what on-chip loops need too.

## A register reaches its own cell again through the IO controller

The flip-flop's data input is hard-wired to its own cell's LUT, so a register
whose next value depends on its current value needs `_reg` back at that cell's
inputs. Inside the fabric that never happens: `cargo run -p sr-ga1-synth
--example feedback_check` confirms that vertical lanes 0/2 carry `_op`, lanes
1/3 carry `_reg`, the pass-through swap keeps the pairs apart, and input `c`
reads lane 0.

The IO controller closes the gap. A value leaves on one of `output_0..3`, and the
controller reads that output back onto any row's lane 0 or 1 at the left edge.
The router uses this by itself — an `OnChipLoop` edge, priced well below a CLB
and above a wire — so:

| design | CLBs, board wiring (old fabric) | CLBs, on-chip (now) |
|---|---|---|
| `toggle.sv` | 4 | 2 |
| `count4.sv` | 10 | 5 |

An output used as a loop still drives its pin. A design output that is also fed
back (a counter's `count[0]`) can share the pad. Board loop-around wiring is
still supported as an extra resource; see `docs/loop-around.md`.

## DDIO pads read themselves back

While a DDIO pad's direction is 1 it drives its output, and its fabric-side input
reads that same output back rather than the pin. The direction comes from an
output mux like any other, and the all-zero configuration points it at row 0
lane 0 — so an unused DDIO is not automatically an input. The synthesiser ties
the direction of every DDIO the design does not use to a constant 0 (a sink on
the constant-0 net), and the visual programmer's DRC warns about a direction
following a live signal unless the design has named the pad.

## Horizontal flow is one-way, so cross-row movement costs something

Horizontal lanes run left to right and never turn. A signal that must change row,
or move leftward, either passes through a CLB's LUT onto a vertical ring — a whole
CLB — or leaves on an output the IO controller reads back.

A signal returning from the ring comes back through a **major** mux, so it lands
only on lanes 2 or 3, and `_reg` reaches only those lanes horizontally too. An
input that cannot see a major lane — input `a` — needs a further CLB to move such
a signal onto a minor lane; the placer charges for that.

## The carry chain is one column, upward; a tap gets a carry out

`carry_out` of `(col, row)` feeds `carry_in` of `(col, row+1)`. Row 0's carry-in
is tied off and there is no wrap, so a chain lives in a single column, at most
`rows` cells, with the LSB at the bottom. Carry appears on no output mux, so on
its own the top cell's carry-out reaches nothing.

An adder is one CLB per bit (`XOR3`, LUT `0x96`, of the two addends and
`carry_in`). `src/carry.rs` derives that LUT value and the carry mux code from the
fabric's own tables, and generates the Yosys `arith_map.v` that produces the
chain.

**A carry needed outside its chain is tapped.** The packer splits each adder into
*segments* a column can hold. A segment whose top carry is needed — a carry-out
port, other logic, or the next segment of an adder wider than a column — ends in
a **carry tap**: one more cell directly above its top bit, input `c` on
`carry_in`, LUT copying it (`BUF_C`, `0xAA`). The carry is then an ordinary net,
and the next segment's least significant bit takes it as its carry-in on a lane
through input `c`, exactly as an adder with a carry-in port does. Such a segment
is not tied to the edge row, so the placer may start it on any row it fits. A bit
whose addends are both 0 — the width extension Yosys adds to hold a final carry —
is packed as a tap as well, since its sum is its carry-in.

So a segment that needs its carry out holds at most `rows - 1` bits. A 4-bit adder
with carry-in and carry-out is two segments (3 bits + tap, 1 bit + tap); a 5-bit
adder is 3 + tap and 2 + tap. What limits wide adders in practice is operand
bandwidth: each row brings at most three pad signals within reach of inputs `a`
and `b` (lanes 0/1 through muxes, lane 2 fixed — and row 2's lane 2 is DDIO), so
two independent 5-bit operands need routing buffers and may not fit.

The trap: `alumacc` must run **before** the generic `techmap`, or techmap expands
`$add` through `$alu`/`$fa`/`$lcu` into gates and the chain is never used.

## Combinational loops are constructible, so checking is mandatory

`vert_in[0]` feeds input mux `c` and vertical lane 0 carries `_op`, so a CLB
driving its own `_op` onto the ring and reading `vert_in[0]` closes a loop
through its own LUT. The IO controller adds more ways round: an output read back
on-chip, and a DDIO in output mode.

`src/loops.rs` checks the **final configuration**, reading the realised mux
selects, the IO controller's selects and any board wiring. A LUT passes on only
the inputs its truth table reads, so a don't-care input left at its default
select closes no loop — which is what used to make the visual programmer's DRC
report loops in designs the synthesiser called clean. The flip-flop is the only
edge left out of the graph, which is what makes a registered feedback path
legal while a combinational one is not.

## Verifying any of this

`sr-ga1-synth --equiv` emits the bitstream, **decodes it back**, turns the
decoded configuration into a flat SystemVerilog netlist — IO controller muxes,
on-chip loops and DDIO read-back included — and simulates that against the
original design under Verilator. The check therefore covers the bitstream
encoding, the LUT bit order, the mux semantics, the fused flip-flop, the lane-3
enables, the derived column clocks and any board wiring.

Independently, `crates/fpga-core/examples/rtl_equiv_gen.rs` checks the visual
programmer's simulator against the RTL itself, programming the `SR_GA1` top level
through the real chain with its start bit; see `docs/rtl-equivalence.md`.

## No clock pin

The only clock input to the chip is `shift_clk`, for programming. Every fabric
clock is derived: a signal is routed from a pad, through at least one CLB onto a
vertical ring, where the column's CSB taps it at the loop point. Budget a CLB for
clock buffering in any sequential design, and remember the lane swap when working
out which lane the tap sees. The column clock enters each column at its top row
and is passed downward; the tools model it as reaching every row at once.

At least one CSB must be uncoupled, or the coupling chain has no source at all.
