# SR-GA1 RTL Analysis

Derived from the SystemVerilog in `hdl/`, cross-checked against the svunit tests in
`hdl/**/*_unit_test.sv`. Sources read: `clb.sv`, `clb_grid.sv`, `sr-ga1.sv`,
`clock_sel.sv`, `clock_bank.sv`, `io_controller.sv`, `shift_reg_no_reset.sv`.
First written for Phase 0 (2026-08-28); revised 2026-10-12 for the LUT3 CLB, the
8-column grid, the configurable IO controller and the programming freeze.

`clb_blackbox.sv` is an STA/synthesis blackbox stub of the same `CLB` module
(marked `/// sta-blackbox`) and carries no behaviour — ignored.

Fabric dimensions in `SR_GA1`: **8 columns × 4 rows** (`localparam COLUMNS = 8`).
`ROWS` is a grid parameter but the IO controller is hard-wired for 4 rows;
`COLUMNS` is freely parameterizable (clock bank scales with it).

Grid orientation (from the `clb_grid.sv` drawing): row 0 is at the **bottom**,
row 3 at the top. Column 0 is leftmost. Horizontal buses flow left → right
(input controllers on the left, output controllers on the right). Vertical buses
flow row 0 → row 1 → row 2 → row 3 within the RTL chain, then **loop back
around** to row 0 at the top level, tapping the clock bank at the loop point.

---

## 1. CLB internals

### 1.1 A 3-input LUT

The CLB computes `operation_result = lut[lut_sel]` with
`lut_sel = {input_a, input_b, input_c}`: an 8-bit truth table, input **`a` the
most significant** index bit. Any function of the three selected inputs is
available. The unit test's named configurations, used by the tools as presets:

| Name | LUT | Expression |
|---|---|---|
| AND3  | `0x80` | `a & b & c` |
| OR3   | `0xFE` | `a \| b \| c` |
| XOR3  | `0x96` | `a ^ b ^ c` |
| NAND3 | `0x7F` | `!(a & b & c)` |
| NOR3  | `0x01` | `!(a \| b \| c)` |
| XNOR3 | `0x69` | `!(a ^ b ^ c)` |
| AO21  | `0xEA` | `(a & b) \| c` |
| MUX2  | `0xE4` | `c ? a : b` |

The all-zero default is the constant-0 function. (Until 2026-10-02 this was a
fixed 8-operation core selected by a 3-bit `operation_select`.)

### 1.2 Input muxes

Inputs `a` and `b` are 2:1 muxes with a **1-bit** select each, choosing
between two *adjacent* horizontal bus lanes (overlapping windows — not all
lanes visible to every input):

| Input | sel = 0 | sel = 1 |
|---|---|---|
| `input_a` | `horz_bus_in[0]` | `horz_bus_in[1]` |
| `input_b` | `horz_bus_in[1]` | `horz_bus_in[2]` |

Input `c` is a 4:1 mux with a **2-bit** select. As well as its two horizontal
lanes it reaches the vertical bus and the dedicated carry chain, which is what
makes the cell a full adder (§1.4):

| `input_mux_c_sel` | Source |
|---|---|
| 0 | `horz_bus_in[2]` |
| 1 | `horz_bus_in[3]` |
| 2 | `vert_bus_in[0]` |
| 3 | `carry_in` (the cell below's carry output) |

So mux A straddles lanes 0/1 and mux B lanes 1/2; mux C straddles lanes 2/3
plus two off-bus sources.

> **Combinational loops are constructible.** `vert_bus_in[0]` is one of the
> two vertical lanes carrying `operation_result` (lanes 0 and 2 carry `op`;
> lanes 1 and 3 carry `operation_ff`). Feeding a combinational lane back into
> the logic means a vertical ring configured as pass-through can close a loop
> through a CLB's own LUT. The shortest one: CLB(c,r) drives `op`
> onto `vert_bus_out[2]`, the other three rows pass through (`minor_vert_sel`
> = 0b0001, 0b0100, 0b0001, following the 0<->2 lane swap), and the signal
> arrives back at `vert_bus_in[0]` of row r, which input mux c reads. With an
> inverting LUT that oscillates; with a non-inverting one it latches.
> DRC reports every such cycle as an error and the simulator refuses to
> settle rather than hanging. Empirically ~90% of *randomly* configured
> fabrics now contain at least one such cycle, so DRC matters much more than
> it did.

(Changed 2026-08-28: input c was a 1-bit 2:1 mux over lanes 2/3; briefly
took `vert_bus_in[3]`, the registered lane, before moving to lane 0.)

### 1.3 Flip-flop

One DFF (`operation_ff`), positive-edge triggered on the column clock:

```systemverilog
always_ff @ (posedge clk)
    if (reset)              operation_ff <= op_ff_reset_val;
    else if (horz_bus_in[3]) operation_ff <= operation_result;
```

- **Input is hard-wired to `operation_result`** — the FF always registers the
  operation output; there is no independent FF data mux.
- **`horz_bus_in[3]` is the FF write-enable.** This is a load-bearing,
  non-obvious fact: horizontal lane 3 at each CLB gates whether that CLB's
  register captures on the clock edge. (Confirmed by `test_operation_ff`.)
- **Bypassable**: yes, in the sense that `operation_result` and `operation_ff`
  are separately routable outputs; the FF is never in the combinational path.
- **Reset** is global and **synchronous**: it wins over the enable, and loads
  the configured `op_ff_reset_val` (config bit 24) on the column's next rising
  clock edge. Default 0, like every config bit.
- The CLB has **no other clock-related configuration** — no edge select, no
  gating, no clock mux. Its clock is hard-wired to the column clock chain,
  which enters at the top row and is passed down (`clk` in → `clk_out`).

### 1.4 The carry chain

`carry = (a + b + c)[1]`, the carry-out of the 3-input full adder, leaves the
CLB on a dedicated `carry_out` port. Each cell has a matching `carry_in`
input, reachable as source 3 of input mux c.

**Chain topology** (in `clb_grid.sv`): one chain per column, running from
row 0 upward — `carry_out` of `(col, row)` feeds `carry_in` of
`(col, row+1)`. Row 0's `carry_in` is tied to constant 0, and the chain does
**not** wrap at the top: a wrap would close a combinational loop through the
adder core, since `carry_in` feeds `input_c` which feeds `carry`. Chains are
therefore `ROWS` (4) cells long and independent per column.

Carry is **not** routable onto the buses — it appears on no output mux. The
only way off the chain is through the next cell's logic (e.g. selecting
carry-in on input c and reading the result on `operation_result`).

**Building an adder**: configure a column's cells with `a` and `b` on the
horizontal lanes, input c = `carry_in`, and LUT = XOR3 (`0x96`). XOR3 of
(a, b, carry_in) is the sum bit on `operation_result`, and `carry_out` is the
matching carry — a textbook full-adder cell per CLB, rippling up the column.

No carry-specific config bits exist beyond the 2-bit input mux c select.

(Added 2026-08-28: previously carry-out was an ordinary routable source on
the major output muxes and there was no carry-in.)

### 1.5 Full ordered per-CLB config bit list (25 bits)

Shift-register indices (index 0 is nearest `shift_data_in`, i.e. holds the
**last** bit shifted in):

| Index | Field | Width | Meaning |
|---|---|---|---|
| 0 | `input_mux_a_sel` | 1 | 0: h0, 1: h1 |
| 1 | `input_mux_b_sel` | 1 | 0: h1, 1: h2 |
| 2–3 | `input_mux_c_sel[1:0]` | 2 | see §1.2 (bit 2 = LSB) |
| 4–11 | `lut[7:0]` | 8 | see §1.1 (bit 4 = `lut[0]`) |
| 12–13 | `minor_horz_sel[1:0]` | 2 | see §2.3 |
| 14–17 | `minor_vert_sel[3:0]` | 4 | see §2.3 |
| 18–20 | `major_horz2_sel[2:0]` | 3 | see §2.3 (bit 18 = LSB) |
| 21–23 | `major_horz3_sel[2:0]` | 3 | see §2.3 (bit 21 = LSB) |
| 24 | `op_ff_reset_val` | 1 | FF synchronous reset value |

Total: **25 bits per CLB**. All defaults 0.

---

## 2. Routing

### 2.1 Bus counts and topology

- **4 horizontal buses per row**, entering at column 0 from the input
  controller, chained CLB→CLB left to right, exiting at the last column into
  the output controller. Point-to-point segmented routing: each CLB owns muxes
  that decide, per lane, whether the incoming segment continues or is replaced.
- **4 vertical buses per column**, chained row 0 → row 3, and then **looped
  back** (`vert_bus_out` → `vert_bus_in` in `SR_GA1`), so each column's
  vertical bus is a closed ring. The clock bank taps the ring at the loop
  point (the value after row 3's output muxes).
- Routing is **segmented nearest-neighbour**, not a crossbar: every lane's
  next segment is driven by exactly one mux in the CLB it passes through, so
  multi-driver conflicts are impossible by construction.

### 2.2 The vertical snake

The vertical pass-throughs **swap lane pairs at every hop**: an incoming
signal on lane 2 continues on lane 0, lane 0 continues on lane 2, and likewise
1↔3. (Comment in the RTL: "Snaking vertical bus allows for 4 signals to
propagate with 2 able to be put onto the horizontal bus output at once.")
A pass-through signal therefore alternates lanes 0↔2 (or 1↔3) as it travels
down the column and around the loop. Combined with the ring topology this must
be traced carefully by the naming engine.

### 2.3 Output muxes — complete legality table

Eight output muxes per CLB (2 minor horizontal, 4 minor vertical, 2 major
horizontal). "pass" marks pass-through codes (net name propagates); "new"
marks codes that introduce a new signal.

**Minor horizontal** (1-bit selects, from `minor_horz_sel`):

| Output | sel = 0 | sel = 1 |
|---|---|---|
| `horz_bus_out[0]` | `operation_result` (new: `_op`) | `horz_bus_in[0]` (pass) |
| `horz_bus_out[1]` | `operation_result` (new: `_op`) | `horz_bus_in[1]` (pass) |

**Minor vertical** (1-bit selects, from `minor_vert_sel`):

| Output | sel = 0 | sel = 1 |
|---|---|---|
| `vert_bus_out[0]` | `operation_result` (new: `_op`) | `vert_bus_in[2]` (pass, lane swap) |
| `vert_bus_out[1]` | `operation_ff` (new: `_reg`) | `vert_bus_in[3]` (pass, lane swap) |
| `vert_bus_out[2]` | `operation_result` (new: `_op`) | `vert_bus_in[0]` (pass, lane swap) |
| `vert_bus_out[3]` | `operation_ff` (new: `_reg`) | `vert_bus_in[1]` (pass, lane swap) |

**Major horizontal** (two independent 8:1 muxes, 3-bit selects
`major_horz2_sel` → `horz_bus_out[2]` and `major_horz3_sel` → `horz_bus_out[3]`,
each paired with its own bus lane):

| Code | `horz_bus_out[2]` source | `horz_bus_out[3]` source | Naming |
|---|---|---|---|
| 0 | constant 0 | constant 0 | reserved `fixed_zero` |
| 1 | constant 1 | constant 1 | reserved `fixed_one` |
| 2 | `vert_bus_in[2]` | `vert_bus_in[3]` | pass (vertical→horizontal bridge) |
| 3 | `horz_bus_in[2]` | `horz_bus_in[3]` | pass (own-lane continuation) |
| 4 | `operation_result` | `operation_result` | new: `_op` |
| 5 | `operation_ff` | `operation_ff` | new: `_reg` |
| 6 | `vert_bus_in[3]` | `vert_bus_in[2]` | pass (vertical crossover) |
| 7 | `horz_bus_in[3]` | `horz_bus_in[2]` | pass (horizontal crossover) |

Each mux pairs with its own lane at codes 2/3 and crosses over to the other
major lane's pair at codes 6/7, so any of the four bus signals {v2, v3, h2,
h3} can reach either major lane. Carry is not here — it leaves on the
dedicated chain (§1.4).

(Changed 2026-08-28: previously both muxes shared one list —
{0, 1, v2, v3, h3, op, ff, carry} — with no lane-2 pass-through, then briefly
carried `carry` at code 6.)

### 2.4 Separately routable CLB outputs — confirmed set

Exactly three: `operation_result` (`_op`), `operation_ff` (`_reg`), `carry`
(`_carry`). Reachability differs per output:

- `_op`: horz lanes 0, 1 (minor), vert lanes 0, 2 (minor), horz lanes 2, 3 (major).
- `_reg`: vert lanes 1, 3 (minor), horz lanes 2, 3 (major).
- `_carry`: **no bus at all** — only the dedicated carry chain to the cell
  above, where it is readable as source 3 of input mux c (§1.4).

### 2.5 Asymmetries worth knowing

- **Both major lanes pass through and can cross over.** Each major mux
  continues its own lane (code 3) and can take the other major lane instead
  (code 7), so signals on lanes 2 and 3 can continue, fork, or swap lanes at
  any CLB. Codes 2 and 6 do the same for the two major vertical lanes.
- **The only horizontal→vertical path is through a CLB's logic**; vertical
  lanes carry a local `_op`/`_reg` or a swapped vertical pass-through.
  Conversely `vert_bus_in[0]` reaches the logic directly through input mux c
  (§1.2). Since lane 0 carries `operation_result`, that closes the vertical
  ring through the logic and makes combinational loops constructible — the
  fabric is no longer feed-forward by construction.
- Since `horz_bus_in[3]` is also the FF enable (§1.3), routing on lane 3 does
  double duty: it both carries a signal and enables registers in the CLBs it
  passes.

---

## 3. CSB (Clock_Selector) and the clock network

One `Clock_Selector` per column, instantiated in `Clock_Bank` at the bottom of
the fabric. **3 config bits each**:

| Index | Field | Width |
|---|---|---|
| 0–1 | `bus_addr[1:0]` | 2 |
| 2 | `couple_to_previous` | 1 |

Behaviour: `clk = couple_to_previous ? prev_clk : bus[bus_addr]`.

- **Sources**: the prompt said "4 sources"; the RTL gives effectively **five**:
  the four vertical bus lanes of its own column (selected by `bus_addr` when
  `couple_to_previous` = 0), or the previous CSB's output (when
  `couple_to_previous` = 1, `bus_addr` ignored).
- **The bus tap point**: the CSB sees the column's vertical ring value at the
  loop point — i.e. after row 3's output muxes, before the signal re-enters
  row 0 — matching the prompt's "end of the column's vertical bus chain,
  before that bus loops back".
- **Chain order confirmed**: `prev_clk[i] = clks[i-1]`, and
  `prev_clk[0] = clks[CLK_NUM-1]` — column N couples from column N−1, column 0
  from the last column, forming a loop. (Confirmed by `test_clk_passthrough`:
  CSB6 coupled to CSB5.)
- **Distribution**: the selected clock drives `column_clks[c]` into the top
  row's CLB and is passed combinationally down the column (`clk_out = clk`),
  as is reset — moved there on 2026-10-10 to ease routing at the bottom — so a
  CSB clocks **every CLB in its column** on a dedicated network. The clock
  never touches the routing fabric in the drive direction; the only crossing
  is the CSB *taking* its input from the vertical ring. Confirmed: no output
  mux anywhere lists a clock as a source.
- Clocks cannot be observed from the fabric — confirmed; no path drives a
  clock net onto any bus.

### ⚠ No external clock source exists

**Discrepancy with the prompt**: the prompt mentions "a dedicated external
clock source … e.g. `CSB0_clock`". The RTL has **no such input**. `SR_GA1`'s
only clock pin is `shift_clk` (programming only). Every column clock is
derived from a vertical bus signal or from another CSB — all fabric clocks are,
in hardware terms, ripple/derived clocks, typically toggled from a chip input
pin routed through the fabric. A fully-coupled CSB loop (all
`couple_to_previous` = 1) has no source at all and is a DRC error.

Note the all-zero default: every CSB selects its column's vertical lane 0 at
the tap point.

---

## 4. IO controller

The `IO_Controller` holds **64 config bits** in its own shift register, which
sits in the chain between the CLB grid and the clock bank. Chip IO: 10 inputs,
10 outputs, 2 dual-direction (DDIO) pins.

**DDIO read-back.** `ddio_in_pre_mux[k] = ddio_dir[k] ? ddio_out[k] : ddio_in[k]`:
while a DDIO pad is an output, the fabric reads its own output back.

**Input side (left edge)** — drives `horz_bus_in` per row at column 0. Lanes
0/1 of every row are 16:1 input muxes (4-bit selects `input_mux_sel{row}{a,b}`)
over:

| Code | Source |
|---|---|
| 0–9 | `chip_inputs[0..9]` |
| 10–11 | `ddio_in_pre_mux[0..1]` |
| 12–15 | `chip_outputs[0..3]` — read back on-chip |

Lanes 2/3 are fixed:

| Row | Lane 2 | Lane 3 |
|---|---|---|
| 0 | `chip_inputs[2]` | `chip_inputs[3]` |
| 1 | `chip_inputs[6]` | `chip_inputs[7]` |
| 2 | `ddio_in_pre_mux[0]` | `ddio_in_pre_mux[1]` |
| 3 | `chip_inputs[8]` | `chip_inputs[9]` |

There are **no constants** at the left edge any more (until 2026-10-03 row 3
entered as `0,1,0,1`), so lane 3 at column 0 — the FF enable — is always a chip
input. Codes 12–15 are the on-chip loop: a chip output, which comes from the
right edge, re-enters on the left.

**Output side (right edge)** — `chip_outputs[0..3]`, `ddio_out[0..1]` and
`ddio_dir[0..1]` are 16:1 output muxes over `from_fabric_buses`, code
`row * 4 + lane`. `chip_outputs[4..9]` are fixed to lanes 6, 7, 10, 11, 14, 15:
rows 1–3, lanes 2/3.

**Freeze.** While `freeze_fabric` is set (§6.4) every chip output and DDIO
output/direction is forced to 0, so DDIO pads are inputs during programming.

**Config register** (index 0 nearest `shift_data_in`): `input_mux_sel0a` (0–3),
`0b`, `1a`, `1b`, `2a`, `2b`, `3a`, `3b` (28–31), `output_mux_sel0..3` (32–47),
`output_mux_sel_ddio0/1` (48–55), `output_mux_sel_ddio_dir0/1` (56–63).

Reserved names: `input_0`…`input_9`, `output_0`…`output_9`, `ddio_in_0/1`,
`ddio_out_0/1`, `ddio_dir_0/1`, and `fixed_zero`/`fixed_one` for the major-mux
constants.

---

## 5. Reset

- Single global `reset` input, distributed combinationally to every CLB
  (fanned out down each column via `reset_out` pass-throughs) — all CLBs see
  it in the same cycle.
- It is sampled **synchronously** on each column's clock, so columns in
  different clock domains come out of reset on their own clock edges.
- Per-CLB reset value: config bit 24 (`op_ff_reset_val`).
- Reset also clears the programming freeze (§6.4) on its falling edge.
- The configuration shift registers (`Shift_Reg_No_Reset`) are **not** reset —
  power-on config is undefined in silicon; the tool's model defaults every
  field to 0.

---

## 6. Bitstream / configuration scan chain

### 6.1 Chain topology (data-flow order)

One serial chain through the whole chip:

```
shift_data_in
  → CLB(row0,col0) → CLB(row0,col1) → … → CLB(row0,col7)     ┐ row-major
  → CLB(row1,col0) → …                → CLB(row3,col7)        ┘ raster
  → IO controller (64 bits)
  → CSB0 → CSB1 → … → CSB7
  → shift_data_out
```

The grid raster is confirmed by the drawing in `clb_grid.sv` (cell 000 at
row 0/col 0, snaking 000→001→…) and the generate block's
`shift_data_chain[r-1][COLUMNS-1]` row wrap. The IO controller and then the
clock bank follow the grid (`clb_io_data_bridge`, `io_clk_data_bridge` in
`sr-ga1.sv`).

Within each cell, `Shift_Reg_No_Reset` shifts from index 0 toward index
DEPTH−1, and `data_out = data[DEPTH-1]`.

### 6.2 Transmission order (what the exported file must contain)

Because first-shifted bits travel deepest, the configuration bit that must be
transmitted **first** is the one destined for the deepest register bit — CSB7's
`couple_to_previous` — and the bit transmitted **last** lands in CLB(0,0)'s
`input_mux_a_sel`. The exact serial order, after the start bit (§6.4), is:

1. For each CSB from **column 7 down to column 0**: bits in descending index
   order — `couple_to_previous` (2), `bus_addr[1]` (1), `bus_addr[0]` (0).
2. The IO controller's 64 bits in descending index order.
3. For each CLB from **(row 3, col 7) backwards along the raster to
   (row 0, col 0)**: its 25 bits in descending index order — bit 24
   (`op_ff_reset_val`) first, bit 0 (`input_mux_a_sel`) last.

Both unit tests program exactly this way, and the RTL equivalence harness
(`docs/rtl-equivalence.md`) programs the whole `SR_GA1` top level this way.

### 6.3 Total bit counts

| Fabric | CLB bits | IO bits | CSB bits | Total |
|---|---|---|---|---|
| 8 × 4 (current) | 32 × 25 = 800 | 64 | 8 × 3 = 24 | **888** |
| 12 × 4 | 48 × 25 = 1200 | 64 | 12 × 3 = 36 | 1300 |
| 16 × 4 | 64 × 25 = 1600 | 64 | 16 × 3 = 48 | 1712 |

A transmission is one bit longer: the start bit.

### 6.4 Programming sequence and the freeze

`SR_GA1` holds a `freeze_fabric` flop clocked by `shift_clk`: cleared
asynchronously on the falling edge of `reset`, set by the first `shift_clk` edge
that sees `shift_data_in` high while `reset` is high. While frozen, the chip
outputs and DDIO outputs/directions are forced to 0, the vertical rings are
gated to 0 at the top level, and the clock bank's wrap-around coupling is gated
too — so nothing can oscillate through a loop while the chain shifts through
arbitrary intermediate configurations.

So a bitstream is sent as: hold `reset` high with `shift_data_in` low; shift
**a start bit of 1** followed by the 888 configuration bits (the start bit sets
the freeze and has been shifted out of the far end by the time the last bit is
in); release `reset` to unfreeze. A synchronous design then asserts `reset`
again and clocks its registers to load their reset values. Both tools prepend
the start bit when exporting and require it when importing.

---

## 7. Findings that deviate from the prompt's expectations

1. **A LUT after all.** Phase 0 found a fixed 8-operation core; the CLB has
   since become a LUT3 (§1.1), and the inspector edits its truth table.
2. **The carry chain is dedicated, not routed** (§1.4): one chain per column
   running upward, no wrap, reachable only through input mux c. Carry-out is
   not a bus source. (The RTL originally had no carry chain at all; it was
   added on 2026-08-28.)
3. **`horz_bus_in[3]` is the FF write-enable** — a significant behaviour the
   prompt didn't mention; it affects simulation, DRC, and the inspector (§1.3).
4. **No external clock pin.** The prompt's `CSB0_clock` default-name case has
   no RTL counterpart; every clock is fabric-derived (§3). The naming rule
   needs a decision here (see open questions).
5. **CSB has 5 effective sources** (4 bus lanes + previous CSB via an
   overriding couple bit), not 4 (§3).
6. **Major lanes 2 and 3 continue via their own mux and can cross over**
   (code 7 swaps them); each major mux bridges only its own vertical lane
   (§2.5).
7. **Vertical buses are closed rings with lane-swapping** (0↔2, 1↔3 per hop)
   (§2.2) — pass-through name tracing must handle both the swap and ring
   cycles (a vertical ring fed only by its own pass-throughs is a floating
   loop, a DRC case analogous to the CSB loop).
8. **Nothing is ever electrically floating.** All-zero config drives `_op`
   (the constant-0 LUT) onto minor lanes and constant 0 onto major lanes. "`<unconnected>`" in the
   UI is a modelling notion (e.g. a lane fed by nothing meaningful /
   constant-0 default), not an electrical one.
9. **Two modules named `CLB`** — `clb_blackbox.sv` is an STA stub; only
   `clb.sv` is behavioural.

## 8. Open questions before Phase 1

1. **Clock default names**: with no external clock source, what should an
   unnamed column clock be called? Proposal: the resolved name of the vertical
   bus net the sourcing CSB selects (per the pass-through rule), falling back
   to `CSB{n}_clock` only if that net is itself unnameable.
2. **`<unconnected>` semantics**: given nothing floats (finding 8), confirm
   that "unconnected" should mean "carrying only a default constant-0 from a
   major mux left at its default" vs. showing those as `fixed_zero`.
3. **Row-3 constants** (resolved, then made moot): row 3 entered as 0/1/0/1 and
   displayed as `fixed_zero`/`fixed_one`. The configurable IO controller no
   longer brings any constant in.
