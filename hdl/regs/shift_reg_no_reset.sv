// A Synchronous, resetless, shift register of configurable depth.
// Data is shifted in on the positive edge of the shift_clk and is accessible either via data_out or data.
// Data is shifted in from index 0 to index DEPTH-1

module Shift_Reg_No_Reset #(
    DEPTH = 1
) (
    input   logic             shift_clk,
    
    input   logic             data_in,
    output  logic [DEPTH-1:0] data,
    output  logic             data_out
);
    
    always_ff @ (posedge shift_clk)
    begin
        data <= {data[DEPTH-2:0], data_in};
    end
    
    assign data_out = data[DEPTH-1];

endmodule