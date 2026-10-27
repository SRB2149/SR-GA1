# Loops back to the left edge, and named IO

Horizontal lanes only flow left to right, and a register's value never comes back
to its own cell through the fabric (`docs/fabric-notes.md` explains why). Anything
with self-dependent state — counters, accumulators, LFSRs — therefore needs a way
from the right edge back to the left. There are two, and both tools model both.

## On-chip loops

The IO controller's input muxes (lanes 0 and 1 of every row) can select
`output_0..3`: whatever the controller is driving onto those chip outputs is read
straight back onto the left edge. That needs no board wiring — it is
configuration, part of the bitstream.

- **In the synthesiser** it is an ordinary routing resource (`OnChipLoop` in the
  routing graph), used automatically and priced between a wire and a CLB. A
  4-bit counter uses one per bit and fits in 5 CLBs. The report lists them under
  `on-chip loops`. An output used this way still drives its pin, and a design
  output that is also fed back (a counter's `count[0]`) can share the pad.
- **In the visual programmer** a lane whose input mux reads an output is drawn
  as a dashed wire leaving the right edge, running below the grid and re-entering
  on the left, labelled `on-chip: output_N → row R lane L`. The IO inspector sets
  it from either edge tile.
- **Simulator and DRC** follow it like any other wire: a loop that does not
  settle is `SimError::CombinationalLoop`, and DRC reports a cycle closed through
  it.

A DDIO pad in output mode is a third, narrower path: its fabric-side input reads
its own output back. Both tools model that; the synthesiser does not plan loops
through it.

## Board loop-around wiring

A physical wire on the board from a chip output pad back to a chip input pad. It
costs no configuration bits, and it is only needed when the on-chip loops run
out.

### In the design file

Two optional fields, written last and omitted when empty:

```json
  "view": null,
  "loopback": [
    { "from": "output_5", "to": "input_4", "net": "acc[3]" }
  ],
  "io_names": {
    "input_1": "clk",
    "output_3": "count[0]"
  }
```

`crates/fpga-core/src/designfile.rs` and `crates/sr-ga1-synth/src/designjson.rs`
define this field-for-field alike, so files round-trip between the two tools. On
load, pad names resolve to `IoPad { input_side, index }` values — chip pins, not
lanes, since which lane a pin reaches is IO controller configuration; an unknown
pad is a warning, not an error, as with stale `pinned` names.

### In the synthesiser

Declare the pads that are physically wired in the constraints file and the tool
picks the pairing itself, reporting which connections to make:

```toml
[loopback]
outputs = ["output_4", "output_5", "output_6", "output_7"]
inputs  = ["input_0", "input_4", "input_5", "input_6"]
```

See `docs/constraints.md`. `--equiv` includes the wiring as an `assign` in the
generated testbench (`crates/sr-ga1-synth/src/equivtb.rs`); on-chip loops are part
of the generated fabric model itself.

### In the visual programmer

- **Canvas** draws each board loop as a solid polyline leaving the output pad's
  lane, running outside the grid and re-entering on every lane that reads the
  input pad, with an arrowhead and a `board: X → Y` label. It carries its net's
  identity colour and live value like any other wire.
- **IO inspector** (`crates/pnrgui/src/app/io_panel.rs`): a board wire is owned by
  the input it drives — pick the driving output in the combo box under a pin the
  row reads — and the output side lists every loop leaving the right edge.
- **Simulator** resolves a board wire inside the settle sweep, like an on-chip
  loop. A DDIO pad in output mode still reads its own output, not the wire.
- **Stimulus panel** disables a board-driven input: it is driven, so offering it
  for editing would only invite setting a value and watching it be ignored.

## Named IO

A pad name is a **display alias**. The pad keeps the reserved identity the fabric
gave it, so stimulus keys, trace ids, VCD signal names and the bitstream are
untouched and renaming can never break an existing design. The two sides are not
symmetric: an *input* pad name renames a net, because the net originates there
(`NetOrigin::Io`), so bus labels follow automatically; an *output* pad name is a
label on the pad, since the net arriving there is named after whatever drives it.
A lane fed by an on-chip loop carries the name of the net leaving on that output.

Block names and pad names share a namespace only where they must. A cell
producing `count[0]` and the pad `count[0]` leaves on are the same signal seen at
two places, so naming both is correct, not a clash — see the comment on
`Design::net_names`.

Naming a DDIO direction pad is also how a design says it means that pad to turn
into an output: DRC otherwise warns when a direction follows a live signal.
