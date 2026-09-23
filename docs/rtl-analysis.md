# SR-GA1 RTL Analysis (Phase 0)

Derived from the SystemVerilog in `hdl/`, cross-checked against the svunit tests in
`hdl/**/*_unit_test.sv`. Sources read: `clb.sv`, `clb_grid.sv`, `sr-ga1.sv`,
`clock_sel.sv`, `clock_bank.sv`, `io_controller.sv`, `mux.sv`, `shift_reg_no_reset.sv`.

`clb_blackbox.sv` is an STA/synthesis blackbox stub of the same `CLB` module
(marked `/// sta-blackbox`) and carries no behaviour — ignored.

Fabric dimensions in `SR_GA1`: **7 columns × 4 rows** (`CLB_Grid #(.ROWS(4), .COLUMNS(7))`).
`ROWS` is a grid parameter but the IO controller is hard-wired for 4 rows;
`COLUMNS` is freely parameterizable (clock bank scales with it).

Grid orientation (from the `clb_grid.sv` drawing): row 0 is at the **bottom**,
row 3 at the top. Column 0 is leftmost. Horizontal buses flow left → right
(input controllers on the left, output controllers on the right). Vertical buses
flow row 0 → row 1 → row 2 → row 3 within the RTL chain, then **loop back
around** to row 0 at the top level, tapping the clock bank at the loop point.

---

## 1. CLB internals

### 1.1 There is no LUT

The prompt anticipated a LUT; the RTL instead has a **fixed 8-function
operation core** selected by a 3-bit `operation_select`. All eight functions are
computed combinationally from the three selected inputs `a`, `b`, `c`, and an
8:1 mux picks one:

| `operation_select` | Function | Expression |
|---|---|---|
| 0 (`000`) | AND3  | `a & b & c` |
| 1 (`001`) | OR3   | `a \| b \| c` |
| 2 (`010`) | XOR3  | `a ^ b ^ c` |
| 3 (`011`) | NAND3 | `!(a & b & c)` |
| 4 (`100`) | NOR3  | `!(a \| b \| c)` |
| 5 (`101`) | XNOR3 | `!(a ^ b ^ c)` |
| 6 (`110`) | AO21  | `(a & b) \| c` |
| 7 (`111`) | MUX2  | `c ? a : b` |

The unit test names them AND3/OR3/…/AO21/MUX2 — the tool should use those
names. (Code 6 was originally SUM1, the full-adder sum bit; it was changed to
AO21 on 2026-08-28 since SUM1 duplicated XOR3.)

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
> through a CLB's own operation core. The shortest one: CLB(c,r) drives `op`
> onto `vert_bus_out[2]`, the other three rows pass through (`minor_vert_sel`
> = 0b0001, 0b0100, 0b0001, following the 0<->2 lane swap), and the signal
> arrives back at `vert_bus_in[0]` of row r, which input mux c reads. With an
> inverting operation that oscillates; with a non-inverting one it latches.
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
  the configured `op_ff_reset_val` (config bit 19) on the column's next rising
  clock edge. Default 0, like every config bit.
- The CLB has **no other clock-related configuration** — no edge select, no
  gating, no clock mux. Its clock is hard-wired to the column clock chain
  (`clk` in → `clk_out` pass-through down the column from the CSB).

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
horizontal lanes, input c = `carry_in`, and operation = XOR3. XOR3 of
(a, b, carry_in) is the sum bit on `operation_result`, and `carry_out` is the
matching carry — a textbook full-adder cell per CLB, rippling up the column.

No carry-specific config bits exist beyond the 2-bit input mux c select.

(Added 2026-08-28: previously carry-out was an ordinary routable source on
the major output muxes and there was no carry-in.)

### 1.5 Full ordered per-CLB config bit list (20 bits)

Shift-register indices (index 0 is nearest `shift_data_in`, i.e. holds the
**last** bit shifted in):

| Index | Field | Width | Meaning |
|---|---|---|---|
| 0 | `input_mux_a_sel` | 1 | 0: h0, 1: h1 |
| 1 | `input_mux_b_sel` | 1 | 0: h1, 1: h2 |
| 2–3 | `input_mux_c_sel[1:0]` | 2 | see §1.2 (bit 2 = LSB) |
| 4–6 | `operation_select[2:0]` | 3 | see §1.1 (bit 4 = LSB) |
| 7–8 | `minor_horz_sel[1:0]` | 2 | see §2.3 |
| 9–12 | `minor_vert_sel[3:0]` | 4 | see §2.3 |
| 13–15 | `major_horz2_sel[2:0]` | 3 | see §2.3 (bit 13 = LSB) |
| 16–18 | `major_horz3_sel[2:0]` | 3 | see §2.3 (bit 16 = LSB) |
| 19 | `op_ff_reset_val` | 1 | FF synchronous reset value |

Total: **20 bits per CLB**. All defaults 0. (Was 19 before input mux c was
widened on 2026-08-28.)

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
- **Distribution**: the selected clock drives `column_clks[c]` into row 0's
  CLB and is passed combinationally down the column (`clk_out = clk`), so a
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

## 4. IO controllers

The `IO_Controller` is purely combinational and holds **zero config bits**.
Chip IO: 10 inputs, 10 outputs, 2 dual-direction (DDIO) pins.

**Input side (left edge)** — drives `horz_bus_in` per row at column 0:

| Row | Lane 0 | Lane 1 | Lane 2 | Lane 3 |
|---|---|---|---|---|
| 0 | `chip_inputs[0]` | `chip_inputs[1]` | `chip_inputs[2]` | `chip_inputs[3]` |
| 1 | `chip_inputs[4]` | `chip_inputs[5]` | `chip_inputs[6]` | `chip_inputs[7]` |
| 2 | `chip_inputs[8]` | `chip_inputs[9]` | `ddio_in[0]`* | `ddio_in[1]`* |
| 3 | constant 0 | constant 1 | constant 0 | constant 1 |

\* DDIO inputs are gated: forced to 0 when the corresponding `ddio_dir` bit is
1 (pin configured as output).

Row 3's constant `4'b1010` means: free `fixed_zero` on lanes 0/2, `fixed_one`
on lanes 1/3 — and, importantly, **row 3's FF enables (lane 3) default to 1**
at the left edge, while rows 0–2's registers are enabled by whatever chip
input or routed signal sits on their lane 3.

**Output side (right edge)** — reads `horz_bus_out` at the last column:

| Row | Lane 0 | Lane 1 | Lane 2 | Lane 3 |
|---|---|---|---|---|
| 0 | `outputs[0]` | `outputs[1]` | `outputs[2]` | `outputs[3]` |
| 1 | `outputs[4]` | `outputs[5]` | `outputs[6]` | `outputs[7]` |
| 2 | `outputs[8]` | `outputs[9]` | `ddio_out[0]` | `ddio_out[1]` |
| 3 | `ddio_dir[0]` | `ddio_dir[1]` | — unused | — unused |

`ddio_dir` (row 3 lanes 0/1) both steers the DDIO pads and gates the DDIO
input paths on row 2.

Suggested reserved names: `input_0`…`input_9`, `output_0`…`output_9`,
`ddio_in_0/1`, `ddio_out_0/1`, `ddio_dir_0/1`, `fixed_zero`, `fixed_one`.

---

## 5. Reset

- Single global `reset` input, distributed combinationally to every CLB
  (fanned out down each column via `reset_out` pass-throughs) — all CLBs see
  it in the same cycle.
- It is sampled **synchronously** on each column's clock, so columns in
  different clock domains come out of reset on their own clock edges.
- Per-CLB reset value: config bit 19 (`op_ff_reset_val`).
- The configuration shift registers (`Shift_Reg_No_Reset`) are **not** reset —
  power-on config is undefined in silicon; the tool's model defaults every
  field to 0.

---

## 6. Bitstream / configuration scan chain

### 6.1 Chain topology (data-flow order)

One serial chain through the whole chip:

```
shift_data_in
  → CLB(row0,col0) → CLB(row0,col1) → … → CLB(row0,col6)     ┐ row-major
  → CLB(row1,col0) → …                → CLB(row3,col6)        ┘ raster
  → CSB0 → CSB1 → … → CSB6
  → shift_data_out
```

The grid raster is confirmed by the drawing in `clb_grid.sv` (cell 000 at
row 0/col 0, snaking 000→001→…) and the generate block's
`shift_data_chain[r-1][COLUMNS-1]` row wrap. The clock bank sits after the
grid (`clb_clk_data_bridge` in `sr-ga1.sv`).

Within each cell, `Shift_Reg_No_Reset` shifts from index 0 toward index
DEPTH−1, and `data_out = data[DEPTH-1]`.

### 6.2 Transmission order (what the exported file must contain)

Because first-shifted bits travel deepest, the bit that must be transmitted
**first** is the one destined for the deepest register bit — CSB6's
`couple_to_previous` — and the bit transmitted **last** lands in CLB(0,0)'s
`input_mux_a_sel`. The exact serial order is:

1. For each CSB from **column 6 down to column 0**: bits in descending index
   order — `couple_to_previous` (2), `bus_addr[1]` (1), `bus_addr[0]` (0).
2. For each CLB from **(row 3, col 6) backwards along the raster to
   (row 0, col 0)**: its 20 bits in descending index order — bit 19
   (`op_ff_reset_val`) first, bit 0 (`input_mux_a_sel`) last.

This is not a guess: both unit tests program exactly this way — `configure()`
in `clb_unit_test.sv` shifts `config_val[19]` first, and
`clock_bank_unit_test.sv`'s `test_config` shows the first-sent 3-bit group
landing in CSB6 with its first bit at index 2.

### 6.3 Total bit counts

| Fabric | CLB bits | CSB bits | Total |
|---|---|---|---|
| 7 × 4 (current) | 28 × 20 = 560 | 7 × 3 = 21 | **581** |
| 12 × 4 | 48 × 20 = 960 | 12 × 3 = 36 | 996 |
| 16 × 4 | 64 × 20 = 1280 | 16 × 3 = 48 | 1328 |

---

## 7. Findings that deviate from the prompt's expectations

1. **No LUT.** Fixed 8-operation core with a 3-bit select (§1.1). The "truth
   table / equation editor" in Phase 4 should instead be an operation picker
   (showing the resulting truth table read-only is still possible).
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
   onto minor lanes and constant 0 onto major lanes. "`<unconnected>`" in the
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
3. **Row-3 constants**: lanes 0–3 of row 3 enter as 0/1/0/1. Should these
   display as `fixed_zero`/`fixed_one` (shared with major-mux codes 0/1), or
   as distinct reserved names?
