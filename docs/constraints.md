# The constraints file

TOML, passed with `--constraints`. Every section is optional; an absent file
means the tool chooses everything itself.

Everything is validated twice: against the fabric when the file is read, and
against the design once it has been elaborated. A constraint that names a pad,
a position or a signal that does not exist is an error with a line number, not
a shrug — silently ignoring one looks exactly like honouring it.

```toml
# Pads physically wired output-back-to-input on the board. The tool pairs them
# itself and reports the connections the design actually needs.
[loopback]
outputs = ["output_2", "output_3", "output_6", "output_7"]
inputs  = ["input_0", "input_1", "input_4", "input_5"]
# allow_ddio = true   # only if you are also handling ddio_dir yourself

# Pin locks: a design signal on a named chip pad.
[pins]
clk = "input_8"
"count[0]" = "output_2"

# Dual-direction pads. DDIO is never inferred from SystemVerilog; declaring it
# here is the only way to use one. `pin` indexes the fabric's DDIO list.
[[ddio]]
pin = 0
in  = "sda_in"
out = "sda_out"
dir = "sda_dir"

# Pin a cell to a CLB, written "col,row". Cells are named after the signal they
# produce, as shown in the report's placement section.
[placement]
"count[3]" = "4,3"

# Which column's CSB sources each clock domain.
[clocks]
clk = 5
```

## What each section costs you

**`[loopback]`** is a pool, not a fixed pairing. See `docs/fabric-notes.md` for
why it exists. The two ends are not symmetric: a pool *output* pad may still
carry a design output — the loop only taps it — but a pool *input* pad is driven
by the board wire and cannot also be a design input.

**`[pins]`** removes a pad from automatic assignment. Bear the routing in mind
when choosing: `_reg` reaches only horizontal lanes 2 and 3, so locking a
registered signal to a lane-0 or lane-1 output pad forces a buffer CLB, and may
be impossible if the cell is in the last column. The global reset cannot be
locked at all — it is a dedicated pin fanned out in hardware and takes no pad.

**`[[ddio]]`** binds three design nets to the pad's three roles. The input net
must be a design input; the output and direction nets must be design outputs. An
unconstrained DDIO pad stays at its default with its input path gated off, and a
design that references a DDIO net without declaring it gets a warning.

**`[placement]`** pins a cell and stops the annealer moving it. A cell in a carry
chain has almost no freedom: a chain occupies consecutive ascending rows of one
column with the least significant bit at the bottom, so pinning one member fixes
the column for all of them, and pinning it to the wrong row is refused with the
row it should be in.

**`[clocks]`** names the column whose CSB taps the ring for that domain. The
clock still has to be routable onto that column's ring, which needs a spare CLB
in the clock's own row to drive it there — so column 0 rarely works, having no
upstream CLB to supply the buffer's constants.

## Reading the result back

The report's `constraints` section lists everything that was pinned down, and
the pin assignment marks each signal `automatic`, `locked by constraint` or
`DDIO declaration`, so it is always clear which parts of the result were choices
and which were inputs.
