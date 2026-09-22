//
//                    GRID LAYOUT
//
//             Vertical        Output
//         H   _|_    _|_    _|_    _|_   H
//      3  o _|012|__|013|__|014|__|015|_ o
//         r  |_ _|  |_ _|  |_ _|  |_ _|  r
//         i    |      |      |      |    i
//         z   _|_    _|_    _|_    _|_   z
//      2  o _|008|__|009|__|010|__|011|_ o
//         n  |_ _|  |_ _|  |_ _|  |_ _|  n
//         t    |      |      |      |    t
//         a   _|_    _|_    _|_    _|_   a
//      1  l _|004|__|005|__|006|__|007|_ l
//            |_ _|  |_ _|  |_ _|  |_ _|  
//         I    |      |      |      |    O
//         n   _|_    _|_    _|_    _|_   u
//      0  p _|000|__|001|__|002|__|003|_ t
//         u  |_ _|  |_ _|  |_ _|  |_ _|  p
//         t    |      |      |      |    u
//             Vertical         Input     t
//
//              0      1      2      3
//
//      The numbers in each box represent the order the CLBs are loaded.
//      Data enters at 000 and snakes around to the next highest cell,
//      so 000 -> 001 -> 002 -> 003 -> 004 and so on (raster pattern).

module CLB_Grid #(
    parameter int ROWS    = 4,
    parameter int COLUMNS = 7
)(
    // Programming interface
    input   logic       shift_clk,
    input   logic       shift_data_in,
    output  logic       shift_data_out,

    // Global reset (synchronous to column clocks)
    input   logic       reset,

    // Column clocks
    input   logic [COLUMNS-1:0]     column_clks,

    // Grid Buses
    input   logic [(4*ROWS)-1:0]    horz_bus_in,
    output  logic [(4*ROWS)-1:0]    horz_bus_out,
    input   logic [(4*COLUMNS)-1:0] vert_bus_in,
    output  logic [(4*COLUMNS)-1:0] vert_bus_out
);

    // One entry per grid cell for every signal that chains between neighbors.
    // NOTE: assumes CLB has clk_out/reset_out relay outputs, based on your
    // original code reading them from neighboring instances -- adjust the
    // port names below if that's not actually CLB's interface.
    logic [ROWS-1:0][COLUMNS-1:0][3:0] horz_chain;
    logic [ROWS-1:0][COLUMNS-1:0][3:0] vert_chain;
    logic [ROWS-1:0][COLUMNS-1:0]      clk_chain;
    logic [ROWS-1:0][COLUMNS-1:0]      reset_chain;
    logic [ROWS-1:0][COLUMNS-1:0]      carry_chain;
    logic [ROWS-1:0][COLUMNS-1:0]      shift_clk_chain;
    logic [ROWS-1:0][COLUMNS-1:0]      shift_data_chain;

    genvar r, c;
    generate
        for (r = 0; r < ROWS; r++) begin : row
            for (c = 0; c < COLUMNS; c++) begin : col

                logic [3:0] horz_in_sel;
                logic       shift_clk_in_sel;
                logic       shift_data_in_sel;
                logic       clk_in_sel;
                logic       reset_in_sel;
                logic       carry_in_sel;
                logic [3:0] vert_in_sel;

                // Horizontal chain: edge cell pulls from the grid's own
                // input port; interior cells pull from the cell to the left.
                if (c == 0) begin : horz_edge
                    assign horz_in_sel      = horz_bus_in[(4*r) +: 4];
                    assign shift_clk_in_sel = shift_clk;
                end else begin : horz_interior
                    assign horz_in_sel      = horz_chain[r][c-1];
                    assign shift_clk_in_sel = shift_clk_chain[r][c-1];
                end

                // Shift-register chain snakes through every cell in
                // row-major order, wrapping from the end of one row to
                // the start of the next.
                if (r == 0 && c == 0) begin : shift_first_cell
                    assign shift_data_in_sel = shift_data_in;
                end else if (c == 0) begin : shift_row_wrap
                    assign shift_data_in_sel = shift_data_chain[r-1][COLUMNS-1];
                end else begin : shift_interior
                    assign shift_data_in_sel = shift_data_chain[r][c-1];
                end

                // Vertical chain: edge cell (top row) pulls from the grid's
                // own input ports; interior cells pull from the cell above.
                // The carry chain starts at 0 in every column and does not
                // wrap like the vertical bus does -- a wrap would close a
                // combinational loop through the adder core.
                if (r == 0) begin : vert_edge
                    assign clk_in_sel   = column_clks[c];
                    assign reset_in_sel = reset;
                    assign carry_in_sel = 1'b0;
                    assign vert_in_sel  = vert_bus_in[(4*c) +: 4];
                end else begin : vert_interior
                    assign clk_in_sel   = clk_chain[r-1][c];
                    assign reset_in_sel = reset_chain[r-1][c];
                    assign carry_in_sel = carry_chain[r-1][c];
                    assign vert_in_sel  = vert_chain[r-1][c];
                end

                CLB clb_u (
                    .shift_clk      (shift_clk_in_sel),
                    .shift_clk_out  (shift_clk_chain[r][c]),
                    .shift_data_in  (shift_data_in_sel),
                    .shift_data_out (shift_data_chain[r][c]),
                    .clk            (clk_in_sel),
                    .clk_out        (clk_chain[r][c]),
                    .reset          (reset_in_sel),
                    .reset_out      (reset_chain[r][c]),
                    .carry_in       (carry_in_sel),
                    .carry_out      (carry_chain[r][c]),
                    .horz_bus_in    (horz_in_sel),
                    .horz_bus_out   (horz_chain[r][c]),
                    .vert_bus_in    (vert_in_sel),
                    .vert_bus_out   (vert_chain[r][c])
                );

                // Drive the grid's own output ports from the far edge cells.
                if (c == COLUMNS-1) begin : horz_out_edge
                    assign horz_bus_out[(4*r) +: 4] = horz_chain[r][c];
                end
                if (r == ROWS-1) begin : vert_out_edge
                    assign vert_bus_out[(4*c) +: 4] = vert_chain[r][c];
                end

            end
        end
    endgenerate

    assign shift_data_out = shift_data_chain[ROWS-1][COLUMNS-1];

endmodule