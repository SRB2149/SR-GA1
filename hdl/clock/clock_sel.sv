module Clock_Selector (
    // Programming interface
    input   logic       shift_clk,
    input   logic       shift_data_in,
    output  logic       shift_data_out,
    
    // Fabric Connections
    input   logic [3:0] bus,
    input   logic       prev_clk,
    output  logic       clk
);
    
    logic [1:0] bus_addr;
    logic       couple_to_previous;

    // Configuration register block
    logic [2:0] reg_data;
    Shift_Reg_No_Reset #(
        .DEPTH(3)
    ) reg_block_u (
        .shift_clk(shift_clk),
        .data_in(shift_data_in),
        .data(reg_data),
        .data_out(shift_data_out)
    );
    
    always_comb
    begin : Clock_MUX
        bus_addr = reg_data[1:0];
        couple_to_previous = reg_data[2];
        clk = couple_to_previous ? prev_clk : bus[bus_addr];
    end

endmodule