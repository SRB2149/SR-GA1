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
    parameter ROWS      = 5,
    parameter COLUMNS   = 5
)(
    // Programming interface
    input   logic       shift_clk,
    output  logic       shift_clk_out,
    input   logic       shift_data_in,
    output  logic       shift_data_out,
    
    // Global reset (synchronous to column clocks)
    input   logic       reset,
    
    // Column clocks
    input   logic       column_clks [COLUMNS],
    
    // Grid Buses
    input  logic [3:0] horz_bus_in [ROWS],
    output logic [3:0] horz_bus_out [ROWS],
    input  logic [3:0] vert_bus_in [COLUMNS],
    output logic [3:0] vert_bus_out [COLUMNS]
);

    logic shift_data [ROWS * COLUMNS];

    genvar r, c;
    generate
        for (r = 0; r < ROWS; r++) begin : row
            for (c = 0; c < COLUMNS; c++) begin : col
                CLB clb_inst (
                    .shift_clk(c == 0 ? shift_clk : row[r].col[c-1].clb_inst.shift_clk_out),
                    .shift_data_in((r == 0 && c == 0) ? shift_data_in : shift_data[r * COLUMNS + c - 1]),
                    .shift_data_out(shift_data[r * COLUMNS + c]),
                    .clk(r == 0 ? column_clks[c] : row[r-1].col[c].clb_inst.clk_out),
                    .reset(r == 0 ? reset : row[r-1].col[c].clb_inst.reset_out),
                    .horz_bus_in(c == 0 ? horz_bus_in[r] : row[r].col[c-1].clb_inst.horz_bus_out),
                    .vert_bus_in(r == 0 ? vert_bus_in[c] : row[r-1].col[c].clb_inst.vert_bus_out)
                );
            end
        end
    endgenerate
    
    assign shift_data_out = shift_data[ROWS * COLUMNS - 1];

endmodule