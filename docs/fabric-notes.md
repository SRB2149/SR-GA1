# SR-GA1 fabric notes

The architectural facts that shape `sr-ga1-synth`, and the reasoning behind the
decisions they forced. `docs/rtl-analysis.md` is the authoritative description of
the hardware; this is about what it means for a synthesiser.

## There is no LUT, so mapping is cell mapping

Eight fixed functions over three selected inputs. Technology mapping is
standard-cell mapping against a genlib, not LUT packing, and since every cell
costs exactly one CLB, ABC minimising area *is* ABC minimising CLB count.

The genlib is **derived** from `fabric.toml` rather than written down
(`src/genlib.rs`): every operation is cofactored against every way of tying its
inputs, which yields the degraded two-input forms, INV and BUF automatically. Two
things fell out of that which were not obvious:

- **Inputs the cofactor ignores are free.** `MUX2` with `c = 1` computes `a`
  regardless of `b`, so `b` needs no constant at all — cheaper than the
  `OR3(a, 0, 0)` form.
- **Two physical inputs may share one gate pin.** `MUX2(p, p, -)` is a buffer
  needing no constant whatsoever, and it is the *only* way to buffer a signal
  where no constant can be reached. It requires the signal on lane 1, the one
  lane both `a` and `b` can select.

Dropping a pin the function ignores is only sound when **one** physical input
drives it. `XOR3(a, b, b) = a`, but only while `b` and `c` are tied *together* —
which "don't care" does not express. That distinction is enforced in
`CellLibrary::derive` and tested.

## Constants are column-dependent, and column 0 is special

A constant on a major lane comes from the major mux of the CLB to the **left**.
At column 0 there is no such CLB, so the only constants available are the ones
the IO map brings in at the left edge — row 3 alone (`0, 1, 0, 1`).

Nothing can be routed *into* column 0 either: its incoming segments are driven by
the IO controller and by nothing else.

This bit four separate times during development — degraded cells, router
buffers, register enables, and clock-ring injection all placed themselves in
column 0 and then failed to route. It is now one derived predicate,
`Fabric::constant_reaches_input(mux, col, row, value)`, plus
`Placer::enable_reachable` and `Placer::clock_cost`. Reach for those rather than
re-deriving the rule.

Related: input `a` sees only the minor lanes, which carry no constant anywhere
outside row 3, so degraded cells must tie `c` or `b`. Inputs `b` and `c` overlap
on lane 2, so tying **both** to the same value costs one lane, not two.

## Horizontal flow is one-way, so cross-row movement costs a CLB

Horizontal lanes run left to right and never turn. The only horizontal-to-
vertical path is through a CLB's operation core, so any signal that must change
row, or move leftward, spends a whole CLB getting onto a vertical ring.

A signal returning from the ring comes back through a **major** mux, so it lands
only on lanes 2 or 3. An input that cannot see a major lane — input `a` — needs a
*further* CLB to move it onto a minor lane.

A cross-row connection therefore needs staging columns to its left: one to reach
a ring, one to bridge back. A destination hard against the left edge has nowhere
to do that, which is why `Placer::connection_cost` charges for `sink.col < 2`.

## Lane 3 is both a wire and a register enable

`horz_bus_in[3]` at each CLB gates that CLB's flip-flop. The routing graph models
this as **one node**: `HSeg(row, col, 3)` holding net `X` simultaneously means "X
is routed here" and "this CLB's enable is X". Single occupancy then resolves
"route a signal past" against "hold this register enabled" automatically, instead
of one silently breaking the other.

Row 3's lane 3 enters as a constant 1, which makes it the cheap home for
unconditional registers. The placer prefers it, but not absolutely: a register
elsewhere only needs an upstream major mux to drive its enable, which costs a
lane rather than a CLB.

## The carry chain is four cells, one column, upward, with no exit

`carry_out` of `(col, row)` feeds `carry_in` of `(col, row+1)`. Row 0's carry-in
is tied off and there is no wrap, so a chain is at most `rows` cells and lives in
a single column with the LSB at the bottom. Carry appears on no output mux: **the
top cell's carry-out reaches nothing at all.**

An adder is one CLB per bit (`XOR3` of the two addends and `carry_in`), which is
dramatically better than ordinary logic — `add4` is 4 CLBs mapped this way versus
11 without. `src/carry.rs` finds the operation and mux codes by searching the
fabric's own tables, and generates the Yosys `arith_map.v` that produces the
chain.

Two traps: `alumacc` must run **before** the generic `techmap`, or techmap
expands `$add` through `$alu`/`$fa`/`$lcu` into gates and the chain is never
used; and a design that reads a carry-out for anything but the next bit of the
same adder is refused, because that value cannot be produced.

## Combinational loops are constructible, so checking is mandatory

`vert_in[0]` feeds input mux `c` and vertical lane 0 carries `_op`, so a CLB
driving its own `_op` onto the ring and reading `vert_in[0]` closes a
combinational loop through its own operation core. Around nine in ten randomly
configured fabrics contain one.

`src/loops.rs` checks the **final configuration**, reading the realised mux
selects, rather than checking the routing. That way it also catches loops through
CLBs nobody routed through. The all-zero default is acyclic, and the checker
confirms it.

The flip-flop is the only edge left out of that graph. That is what makes a
legitimate registered feedback path legal while a combinational one is not.

## A register cannot depend on its own value — and what to do about it

This is the sharpest limitation in the fabric, verified by exhaustive
reachability over all 28 CLBs (`cargo run -p sr-ga1-synth --example
feedback_check`):

```
NO register output reaches any input of its own CLB.
vertical lanes carrying _op:  [0, 2]
vertical lanes carrying _reg: [1, 3]
input c reads: [HorzIn(2), HorzIn(3), VertIn(0), CarryIn]
```

The flip-flop's data input is hard-wired to its own cell's `operation_result`, so
a register whose next value depends on its current value needs `_reg` back at
that cell's inputs. The only return path to a cell is its column's ring — and
`_reg` lives on lanes 1/3, the pass-through swap keeps those separate from the
`_op` lanes 0/2, and input `c` reads lane **0**. A registered value can leave and
travel rightward; it can never come back.

Counters, accumulators and LFSRs all need exactly this.
`docs/rtl-analysis.md` §1.2 records that input `c` briefly read `v_in:3` before
moving to lane 0. That change is what removed this capability — and it is also
what *created* the combinational-loop problem above, since lane 0 carries `_op`.

There are two ways out, and the tool supports the second:

1. **Point input `c` at a registered vertical lane.** Tested on a scratch copy of
   `fabric.toml` with `v_in:3`: every CLB's `_reg` then reaches its own input `c`,
   and `toggle.sv` fits in 3 CLBs. It would also make combinational loops
   through the ring impossible. The catch is that the carry chain also occupies
   input `c`, so a counter would have to be built from ordinary logic rather than
   the chain. **This has not been applied** — `fabric.toml` is unchanged.

2. **Loop-around IO.** Wire spare chip outputs back to spare chip inputs on the
   board and declare them; the router then treats each loop as an ordinary
   routing resource. This needs no fabric change, keeps the carry chain, and also
   provides the only cheap way to move a signal leftward or across rows. With a
   pool declared, `toggle.sv` fits in 4 CLBs and `count4.sv` in 10 — four chain
   cells plus one loop per bit.

   A loop is a **wire**: it breaks no combinational path, so the loop checker
   includes it. And a loop *input* pad is driven by the board, so it cannot also
   be a design input — though a loop *output* pad may still carry a design
   output, which is the common case, since a counter's `count[0]` is both an
   output and its own feedback.

   The visual programmer does not model board wiring yet; see
   `docs/gui-loopback-support.md`.

## Verifying any of this

`sr-ga1-synth --equiv` emits the bitstream, **decodes it back**, turns the
decoded configuration into a flat SystemVerilog netlist, and simulates that
against the original design under Verilator. The check therefore covers the
bitstream encoding, the mux semantics, the operation tables, the fused
flip-flop, the lane-3 enables, the derived column clocks and the loop-around
wiring — everything in this document.

It is worth trusting for a reason: it caught a real bug in its own testbench,
where a 4-bit port was being connected bit-0-only, and it would catch any of the
claims above being wrong in the emitter.

## No clock pin

The only clock input to the chip is `shift_clk`, for programming. Every fabric
clock is derived: a signal is routed from a pad, through at least one CLB onto a
vertical ring, where the column's CSB taps it at the loop point. Budget a CLB for
clock buffering in any sequential design, and remember the lane swap when working
out which lane the tap sees.

At least one CSB must be uncoupled, or the coupling chain has no source at all.
