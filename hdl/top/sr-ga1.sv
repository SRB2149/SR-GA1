module SR_GA1 (
    // Programming interface
    input   logic       shift_clk,
    input   logic       shift_data_in,
    output  logic       shift_data_out, //DFT
    
    // Global synchronous reset (positive trigger)
    input   logic       reset,
    
    // Chip IO
    input   logic [9:0] inputs,
    output  logic [9:0] outputs,
    
    // Chip Dual-Direction IO
    input   logic [1:0] ddio_in,
    output  logic [1:0] ddio_dir,
    output  logic [1:0] ddio_out
);

    localparam COLUMNS = 7;
    // Rows are fixed from an easily configurable perspective.
    // The IO controller will need to be edited to add more rows.
    
    // CLB Routing
    logic [3:0] vertical_buses  [COLUMNS];
    logic       column_clks [COLUMNS];
    logic [3:0] to_fabric_buses [4];
    logic [3:0] from_fabric_buses [4];
    
    // Configuration Routing
    logic clb_clk_data_bridge;
    
    IO_Controller io_controller_u (
        .chip_inputs(inputs),
        .chip_outputs(outputs),
        .ddio_in(ddio_in),
        .ddio_dir(ddio_dir),
        .ddio_out(ddio_out),
        .from_fabric_buses(from_fabric_buses),
        .to_fabric_buses(to_fabric_buses)
    );

    CLB_Grid #(
        .ROWS(4),
        .COLUMNS(COLUMNS)
    ) clb_grid_u (
        .shift_clk(shift_clk),
        .shift_data_in(shift_data_in),
        .shift_data_out(clb_clk_data_bridge),
        .reset(reset),
        .column_clks(column_clks),
        .horz_bus_in(to_fabric_buses),
        .horz_bus_out(from_fabric_buses),
        .vert_bus_in(vertical_buses), // Vertical buses loop back around
        .vert_bus_out(vertical_buses) // They also act as an input to the clock bank (below)
    );
    
    Clock_Bank # (
        .CLK_NUM(COLUMNS)
    ) clk_bank_u (
        .shift_clk(shift_clk),
        .shift_data_in(clb_clk_data_bridge),
        .shift_data_out(shift_data_out),
        .buses(vertical_buses),
        .clks(column_clks)
    );

endmodule