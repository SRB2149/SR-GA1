# GUI changes needed for loop-around IO

`sr-ga1-synth` can now use **loop-around wiring** — a physical connection on the
board from a chip output pad back to a chip input pad — as a routing resource.
This is what makes self-dependent state (counters, accumulators, LFSRs) possible
at all; see `docs/fabric-notes.md` for why the fabric alone cannot do it.

The visual programmer does not know about this yet. **A design that uses
loop-around wiring will simulate incorrectly in the GUI**: the looped input will
read whatever its stimulus says instead of the value the output is driving, so a
counter will appear not to count. The synthesiser warns about this on every run
that needs a loop, and in the report.

This note is the to-do list for closing that gap. Nothing here is urgent for
programming real silicon — the bitstream is already correct, and the required
connections are printed. It only affects simulation and display.

## 1. Carry the field through the design file

`crates/fpga-core/src/designfile.rs`

The synthesiser writes an extra, final field. It is omitted when empty, so
existing files are unchanged and the format is still byte-identical for designs
without loops:

```json
  "view": null,
  "loopback": [
    { "from": "output_2", "to": "input_0", "net": "count[0]" },
    { "from": "output_3", "to": "input_4", "net": "count[3]" }
  ]
```

`serde` ignores unknown fields, so the GUI **loads these files today** — but it
drops the field on save, silently discarding the wiring. Add to `FileSchema`:

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct LoopbackLink {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub net: String,
}

// ...last field of FileSchema, after `view`:
#[serde(default, skip_serializing_if = "Vec::is_empty")]
loopback: Vec<LoopbackLink>,
```

and a matching `Vec<LoopbackLink>` on `DesignFile`, so a load/save round trip
preserves it. Validate that `from` names a chip output and `to` a chip input,
warning rather than erroring on a stale name, as the loader does elsewhere.

## 2. Make the simulator follow the wire

`crates/fpga-core/src/sim.rs`

A loop-around wire is **combinational**, not a register. So it belongs inside
the existing Gauss-Seidel settle loop, not between ticks:

- Before each settle iteration, copy each `from` output's current value into its
  `to` input, overriding stimulus for that input.
- Keep iterating until stable, exactly as now. A loop that does not settle is a
  combinational loop and should be reported the same way the fabric's own are.

A looped input is **driven**, so the stimulus panel must stop offering it for
editing — otherwise the user sets a value and watches it be ignored. Marking it
read-only with the driving output's name shown is probably enough.

## 3. Include it in DRC

`crates/fpga-core/src/drc.rs`

`drc::combinational_loops` must add an edge from each `from` output to its `to`
input. Without this the GUI will call a design clean that oscillates in
hardware. The synthesiser's own checker does this in
`crates/sr-ga1-synth/src/loops.rs`, and `crates/sr-ga1-synth/tests/loops.rs`
has the case worth copying: with the all-default configuration, wiring
`output_0` back to `input_0` closes a loop through row 0, because every CLB
drives its own `_op` onto minor lane 0 and input `a` reads it back.

Note the distinction the test pins down: a path that crosses a flip-flop is
**not** a loop. Only the `_op -> _reg` edge may be omitted from the graph.

## 4. Carry the IO names over too

`crates/fpga-core/src/designfile.rs`

The `pinned` map names blocks, not nets, so the IO would otherwise read as
`input_8` rather than `clk`. The synthesiser writes a second optional field for
that, also last and also omitted when empty:

```json
  "io_names": {
    "input_3": "clk",
    "output_0": "count[0]",
    "output_9": "count[3]"
  }
```

Reading it and using it as the display name for the pad's net would make the
canvas match the source. The constants and the dedicated reset are deliberately
absent, having no design signal of their own.

Note that block names from the synthesiser are already allocated to be unique
*including* their derived `_op`/`_reg`/`_carry` nets, since `Design::rename`
rejects a collision — several buffers legitimately want to be called
`buf_count[3]`, so they come out as `buf_count[3]`, `buf_count[3]_2` and so on.

## 5. Show it

`crates/fpgatool/src/app/canvas.rs`, `inspector.rs`

- Draw each loop as a wire leaving the right-edge output and returning to the
  left-edge input — a labelled curve around the outside of the grid reads better
  than a line across it.
- Let the user add and remove loops in the inspector, since this is board wiring
  they physically control. Constrain the pickers to real pads, and exclude DDIO
  pads unless direction is also being handled.
- A looped output pad can still be a design output; a looped input pad cannot
  also be a design input. That asymmetry is worth surfacing in the UI.

## 6. Optional: agree the pool with the synthesiser

The synthesiser reads the *available* pads from a constraints file and picks the
pairing itself, reporting what to connect:

```toml
[loopback]
outputs = ["output_2", "output_3", "output_6", "output_7"]
inputs  = ["input_0", "input_1", "input_4", "input_5"]
```

If the GUI ever grows a board-configuration view, having it write that same file
would keep the two tools honest about which pads are physically wired. Until
then the file is hand-written, and the design file records only the pairs a
given design actually needs.

## Cross-check: the synthesiser already models this

`sr-ga1-synth --equiv` builds a flat SystemVerilog model **from the decoded
bitstream** and simulates it against the original design under Verilator, with
the loop-around wiring included as an `assign` in the testbench. `count4.sv`
passes 800 cycles that way.

So there is a working reference for the behaviour the GUI needs to reproduce:
`crates/sr-ga1-synth/src/equiv.rs` generates the fabric model, and
`equivtb.rs` wires the loops. Running with `--keep-intermediates` leaves both
files in the work directory, which is the quickest way to see exactly what a
loop-around design is supposed to do.

## Worth knowing

`_reg` reaches only horizontal lanes 2 and 3, so the output pads a register can
drive **directly** are `output_2`, `output_3` (row 0) and `output_6`, `output_7`
(row 1). Row 2's lanes 2/3 are DDIO and row 3 has no pads there, so a register in
those rows needs a vertical-ring hop and a major-mux bridge into row 0 or 1
first. That costs no CLB but does consume ring segments, which is why a 4-bit
counter's top bit is more expensive to route than its bottom bit.
