// A small configurable logic block.
// Configure by shifting data into the configuration register block.
// Index | Function
//   0   | input_mux_a_sel
//   1   | input_mux_b_sel
//   2   | input_mux_c_sel[0]
//   3   | input_mux_c_sel[1]
//   4   | lut[0]
//   5   | lut[1]
//   6   | lut[2]
//   7   | lut[3]
//   8   | lut[4]
//   9   | lut[5]
//   10  | lut[6]
//   11  | lut[7]
//   12  | minor_horz_sel[0]
//   13  | minor_horz_sel[1]
//   14  | minor_vert_sel[0]
//   15  | minor_vert_sel[1]
//   16  | minor_vert_sel[2]
//   17  | minor_vert_sel[3]
//   18  | major_horz2_sel[0]
//   19  | major_horz2_sel[1]
//   20  | major_horz2_sel[2]
//   21  | major_horz3_sel[0]
//   22  | major_horz3_sel[1]
//   23  | major_horz3_sel[2]
//   24  | op_ff_reset_val
//
// Layout (pins not necessarily in exact shown positions, just done to show which edge each is on)
//                  v v v v
//                  | | | |
//                  b b b b
//                  u u u u     c
//                  s s s s     a
//                  | | | |     r
//                  o o o o     r
//                  u u u u   r y
//                  t t t t   e |
//                  - - - - c s o
//                  0 1 2 3 l e u
//                  - - - - k t t
//                     
//                 _|_|_|_|_|_|_|_
// h_bus_in[0]   -|               |- h_bus_out[0]
// h_bus_in[1]   -|               |- h_bus_out[1]
// h_bus_in[2]   -|    SR-GA1     |- h_bus_out[2]
// h_bus_in[3]   -|    CLB        |- h_bus_out[3]
// shift_clk     -|               |- shift_clk_out
// shift_data_in -|_ _ _ _ _ _ _ _|- shift_data_out
//                  | | | | | | |
//
//                  v v v v c r c
//                  | | | | l e a
//                  b b b b k s r
//                  u u u u | e r
//                  s s s s o t y
//                  | | | | u | |
//                  i i i i t o i
//                  n n n n   u n
//                  - - - -   t
//                  0 1 2 3
//                  - - - -

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

    // Configuration register block
    logic [24:0] reg_data;
    Shift_Reg_No_Reset #(
        .DEPTH(25)
    ) reg_block_u (
        .shift_clk(shift_clk),
        .data_in(shift_data_in),
        .data(reg_data),
        .data_out(shift_data_out)
    );
    
    // Input signals
    logic       input_mux_a_sel, input_mux_b_sel;
    logic [1:0] input_mux_c_sel;
    logic [3:0] input_c_mux_in;
    logic       input_a, input_b, input_c;
    
    assign input_mux_a_sel = reg_data[0];
    assign input_mux_b_sel = reg_data[1];
    assign input_mux_c_sel = reg_data[3:2];
    
    // LUT/Arithmetic signals
    logic [7:0] lut;
    logic [2:0] lut_sel;
    logic       carry;
    logic       operation_result;
    
    assign lut = reg_data[11:4];
    
    // Output signals
    logic [7:0] output_mux_h2_in, output_mux_h3_in;
    logic [3:0] minor_vert_sel;
    logic [2:0] major_horz3_sel, major_horz2_sel;
    logic [1:0] minor_horz_sel;
    
    assign minor_horz_sel = reg_data[13:12];
    assign minor_vert_sel = reg_data[17:14];
    assign major_horz2_sel = reg_data[20:18];
    assign major_horz3_sel = reg_data[23:21];
    assign shift_clk_out = shift_clk;   // Shift clock passthrough into deeper cells
    assign clk_out = clk;               // Clock passthrough into deeper cells
    assign reset_out = reset;           // Reset passthrough into deeper cells
    assign carry_out = carry;           // Carry chain into the cell above
    
    // Register
    logic operation_ff, op_ff_reset_val;
    assign op_ff_reset_val = reg_data[24];
    
    // Input multiplexers
    always_comb
    begin : input_muxes
        input_a = input_mux_a_sel ? horz_bus_in[1] : horz_bus_in[0];
        input_b = input_mux_b_sel ? horz_bus_in[2] : horz_bus_in[1];
        
        input_c_mux_in = {      // INDEX
            carry_in,           //   3
            vert_bus_in[0],     //   2
            horz_bus_in[3],     //   1
            horz_bus_in[2]      //   0
        };
        
        input_c = input_c_mux_in[input_mux_c_sel];
    end
    
    //LUT and Carry Generator
    always_comb
    begin : arith_lut_core
        lut_sel = {input_a, input_b, input_c}; //LUT as XOR3 when used in adders
        carry = input_a & input_b | (input_a ^ input_b) & input_c;
        operation_result = lut[lut_sel];
    end
    
    // Register (synchronous reset)
    always_ff @ (posedge clk)
    begin
        if (reset)
        begin
            operation_ff <= op_ff_reset_val;
        end
        else
        begin
            if (horz_bus_in[3])
            begin
                operation_ff <= operation_result;
            end
        end
    end
    
    // Minor output multiplexers
    always_comb
    begin : minor_output_muxes
        horz_bus_out[0] = minor_horz_sel[0] ? horz_bus_in[0] : operation_result;
        horz_bus_out[1] = minor_horz_sel[1] ? horz_bus_in[1] : operation_result;
        
        // Snaking vertical bus allows for 4 signals to propagate with 2 able to
        // be put onto the horizontal bus output at once.
        vert_bus_out[0] = minor_vert_sel[0] ? vert_bus_in[2] : operation_result;
        vert_bus_out[1] = minor_vert_sel[1] ? vert_bus_in[3] : operation_ff;
        vert_bus_out[2] = minor_vert_sel[2] ? vert_bus_in[0] : operation_result;
        vert_bus_out[3] = minor_vert_sel[3] ? vert_bus_in[1] : operation_ff;
    end
    
    // Major output multiplexers
    always_comb
    begin : major_output_muxes
        output_mux_h2_in = {    // INDEX
            horz_bus_in[3],     //   7
            vert_bus_in[3],     //   6
            operation_ff,       //   5
            operation_result,   //   4
            horz_bus_in[2],     //   3
            vert_bus_in[2],     //   2
            1'b1,               //   1
            1'b0                //   0
        };
        
        horz_bus_out[2] = output_mux_h2_in[major_horz2_sel];
        
        output_mux_h3_in = {    // INDEX
            horz_bus_in[2],     //   7
            vert_bus_in[2],     //   6
            operation_ff,       //   5
            operation_result,   //   4
            horz_bus_in[3],     //   3
            vert_bus_in[3],     //   2
            1'b1,               //   1
            1'b0                //   0
        };
        
        horz_bus_out[3] = output_mux_h3_in[major_horz3_sel];
    end

endmodule