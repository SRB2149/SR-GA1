# How `sr-ga1-synth` works

A standalone command-line flow from SystemVerilog to a configuration bitstream.
It shares no code with the visual programmer; both read the same `fabric.toml`,
and the golden tests in `crates/sr-ga1-synth/tests/golden/` are what hold the two
output formats together.

`docs/fabric-notes.md` explains *why* the stages are shaped the way they are.
This describes what each one does.

## The flow

```
SystemVerilog
  ├─ source scan ..................... subset.rs      reject what Yosys would ignore
  ├─ Yosys: elaborate ................ yosys.rs       read_verilog, proc, flatten, opt, fsm
  │    └─ subset check ............... subset.rs      on the elaborated netlist
  ├─ Yosys: alumacc + arith_map ...... carry.rs       adders onto the carry chain
  ├─ Yosys: dfflegalize .............. yosys.rs       one flop shape
  ├─ ABC: map against fabric.genlib .. genlib.rs      every <=3-input function
  ├─ pack ............................ pack.rs        fuse flops, thread chains, assign enables
  │
  ├─ outer search loop ............... flow.rs        many seeds, keep the best
  │    ├─ place + assign pins ........ place.rs       simulated annealing
  │    ├─ build the routing problem .. backend.rs     nets, pads, enables, DDIO ties
  │    ├─ route ...................... route.rs       PathFinder over rrg.rs
  │    ├─ emit configuration ......... backend.rs     every edge names its own write
  │    └─ loop check ................. loops.rs       refuse any combinational cycle
  │
  ├─ equivalence check (--equiv) ..... equiv*.rs      decode the bits, compare under Verilator
  └─ outputs ......................... bitstream.rs, designjson.rs, report.rs
```

Each stage is a separate module with plain data on its boundaries, so a failing
stage can be reproduced from a dump (`--keep-intermediates` writes the Yosys
netlists, the genlib, the implementation table, the arithmetic map and, with
`--equiv`, the generated fabric model and testbench).

## The pieces that carry the weight

**`fabric.rs`** is the only thing that knows the hardware. Dimensions, config
field layout, mux legality, the LUT and its index order, the IO controller and
the scan chain all come from `fabric.toml`; nothing downstream hard-codes 8×4 or
a lane number. Where a rule about the fabric is needed in more than one place it
lives here as a derived predicate — `constant_reaches_lane`,
`lanes_for_source`, `lanes_for_output`, `register_feedback_possible`,
`vert_pass_target` — rather than being re-derived.

**`genlib.rs`** derives the cell library instead of shipping one: every function
of up to three pins that uses all of them, each with every *physical*
implementation — a placement of its pins on inputs `a`/`b`/`c` and the LUT value
that computes it from there. Placement picks among them so a pin sits on an
input that can see the lane its signal arrives on, without re-running ABC.

**`rrg.rs`** is the routing resource graph, and the piece everything else trusts.
Every node is a place a signal can be, every edge is a mux code, and **each edge
carries the configuration write it implies** — so a routed path already *is* a
bitstream, with no second translation step that could disagree with the first.
Two modelling decisions do a lot of work: lane 3's register enable is the *same
node* as lane 3's wire, so single occupancy resolves "route past" against "hold
enabled" by itself; and a buffer is a priced edge through a CLB's LUT, so the
router allocates its own vertical hops. Chip pads are nodes too: the IO
controller's input and output muxes are edges carrying IO configuration writes,
and a chip output the controller can read back is an `OnChipLoop` edge to the
left edge, so feedback needs no special handling.

**`route.rs`** is PathFinder: rip up one net at a time and reroute it against
everyone else's current routes, with history accumulating on contested nodes.
Congestion is always a real conflict here, because every lane segment is driven
by exactly one mux.

**`loops.rs`** reads the *realised* mux selects out of the finished
configuration, not the routing, so it also catches cycles through CLBs nobody
routed through. The IO controller's selects, on-chip loops, DDIO read-back and
board wires are all included as combinational edges, and a LUT contributes only
the inputs its truth table reads. The flip-flop is the only edge left out, which
is what makes registered feedback legal and combinational feedback not.

## The outer loop

Placement and routing are one search, not two stages. A single annealing run
costs milliseconds at this size, so `flow.rs` places from many seeds, routes
each, and keeps the first result that is complete, configurable and loop-free.
An attempt is scored on `unrouted + congested` — both make a routing unusable,
and scoring on unrouted sinks alone lets a congested attempt set the bar at zero
and stall the search.

`--effort` sets the annealing sweeps, the attempt count and the routing
iterations; `--time-budget` bounds the whole thing. Everything is deterministic:
the same input and seed produce a byte-identical bitstream, which is tested.

## What is deliberately not automatic

- **DDIO is never inferred.** A pad is used only when the constraints file names
  its input, output and direction nets.
- **Loop-around wiring is never assumed.** It is board wiring the tool cannot
  see, so it is declared, and the pairing the router chose is reported for
  soldering. On-chip loops through the IO controller need no declaration: they
  are configuration, and the router uses them by itself.
- **Constraints are never silently ignored.** A pin lock, placement or clock
  assignment naming something the design does not have is an error with a line
  number. See `docs/constraints.md`.
- **Nothing outside the subset is mapped.** See `docs/sv-subset.md`; a plausible
  wrong bitstream is worse than a refusal.

## Verification

- **Golden format tests** reproduce GUI-exported design JSON and bitstreams
  byte-for-byte, which is the only thing keeping two independent codebases on one
  format.
- **Equivalence checking** (`--equiv`) emits the bitstream, decodes it back,
  generates a flat SystemVerilog netlist from the decoded configuration, and
  simulates it against the original design under Verilator. It therefore covers
  the encoding as well as the mapping.
- **Structural tests** assert the routing graph against the documented
  architecture — lane swapping, ring closure, carry isolation, input windows,
  enable-as-node — rather than against itself.
- **Determinism and loop-freedom** are asserted across the corpus in
  `crates/sr-ga1-synth/tests/`.

## Non-goals

No timing-driven optimisation: the fabric has no timing model, so reported
"depth" is a structural count of LUTs, not a delay. No partial
reconfiguration and no incremental synthesis.
