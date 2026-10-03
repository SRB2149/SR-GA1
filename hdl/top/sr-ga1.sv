// The top module of the SR-GA1 FPGA
//
// Bitstream information:
//  Length: 828 bits
//  Composition:
//  Bit ranges | Configuration area
// ------------+------------------------
//     0-799   | CLB LUT and mux configs
//   800-863   | IO controller config
//   864-887   | Clock bank configs
//
// To program:
// 1) Assert reset after setting shift_data_in to 0
// 2) Send bitstream including high start bit to freeze outputs when programming via shift_clk and shift_data_in
// 3) Deassert reset to unfreeze the outputs and allow internal oscillation
// If combinational: PROGRAMMING SEQUENCE END
// If synchronous requiring reset: CONTINUE
// 4) Assert reset
// 5) Clock the relevant flip-flops
// 6) Deassert reset
// PROGRAMMING SEQUENCE END
//
// Programming notes:
// - Always ensure that in the programmer software data is sent to the programmer IO before the clock signal is
//   to ensure that the correct value is presented when the positive clock edge arrives
// - Data is written on the positive edge of shift_clk

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

    localparam COLUMNS = 8;
    // Rows are fixed from an easily configurable perspective.
    // The IO controller will need to be edited to add more rows.
    
    // CLB Routing
    logic [(4*COLUMNS)-1:0] vertical_buses;
    logic [(4*COLUMNS)-1:0] pregate_vertical_buses;
    logic [COLUMNS-1:0]     column_clks;
    logic [15:0]            to_fabric_buses;
    logic [15:0]            from_fabric_buses;
    
    // Programming anti-osc/output protection
    logic freeze_fabric;
    
    // Configuration Routing
    logic clb_io_data_bridge;
    logic io_clk_data_bridge;
    
    IO_Controller io_controller_u (
        .shift_clk(shift_clk),
        .shift_data_in(clb_io_data_bridge),
        .shift_data_out(io_clk_data_bridge),
        .freeze_fabric(freeze_fabric),
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
        .shift_data_out(clb_io_data_bridge),
        .reset(reset),
        .column_clks(column_clks),
        .horz_bus_in(to_fabric_buses),
        .horz_bus_out(from_fabric_buses),
        .vert_bus_in(vertical_buses),         // Vertical buses loop back around
        .vert_bus_out(pregate_vertical_buses) // They also act as an input to the clock bank (below)
    );
    
    always_comb
    begin : vertical_feedback_gating
        vertical_buses = freeze_fabric ? '0 : pregate_vertical_buses;
    end
    
    Clock_Bank # (
        .CLK_NUM(COLUMNS)
    ) clk_bank_u (
        .shift_clk(shift_clk),
        .shift_data_in(io_clk_data_bridge),
        .shift_data_out(shift_data_out),
        .buses(vertical_buses),
        .clks(column_clks)
    );
    
    // Programming anti-oscillation/output protection controller
    // Synchronous to the shift clock
    always_ff @ (posedge shift_clk, negedge reset)
    begin : prog_protection
        if (!reset)
        begin
            freeze_fabric <= '0;
        end
        else if (reset && shift_data_in)
        begin
            freeze_fabric <= '1;
        end
    end

endmodule