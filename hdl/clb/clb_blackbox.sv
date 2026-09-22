/// sta-blackbox

module CLB (
    // Programming interface
    input   logic       shift_clk,
    output  logic       shift_clk_out,
    input   logic       shift_data_in,
    output  logic       shift_data_out,
    
    // Fabric Connections
    input   logic       clk,
    input   logic       reset,
    input   logic       carry_in,
    input   logic [3:0] horz_bus_in,
    input   logic [3:0] vert_bus_in,
    output  logic       clk_out,
    output  logic       reset_out,
    output  logic       carry_out,
    output  logic [3:0] horz_bus_out,
    output  logic [3:0] vert_bus_out
);

endmodule