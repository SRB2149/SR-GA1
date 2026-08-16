// A small configurable logic block.
// Configure by shifting data into the configuration register block.
// Index | Function
//   0   | input_mux_a_sel
//   1   | input_mux_b_sel
//   2   | input_mux_c_sel
//   3   | operation_select[0]
//   4   | operation_select[1]
//   5   | operation_select[2]
//   6   | minor_horz_sel[0]
//   7   | minor_horz_sel[1]
//   8   | minor_horz_sel[2]
//   9   | minor_vert_sel[0]
//   10  | minor_vert_sel[1]
//   11  | minor_vert_sel[2]
//   12  | major_horz_sel[0]
//   13  | major_horz_sel[1]
//   14  | major_horz_sel[2]
//   15  | major_vert_sel[0]
//   16  | major_vert_sel[1]
//   17  | major_vert_sel[2]
//   18  | op_ff_reset_val
//
// Layout (pins not necessarily in exact shown positions, just done to show which edge each is on)
//                  v v v v
//                  | | | |
//                  b b b b
//                  u u u u   r
//                  s s s s   e
//                  | | | | c s
//                  o o o o l e
//                  u u u u k t
//                  t t t t | |
//                  - - - - o o
//                  0 1 2 3 u u
//                  - - - - t t
//                     
//                 _|_|_|_|_|_|_
// h_bus_in[0]   -|             |- h_bus_out[0]
// h_bus_in[1]   -|             |- h_bus_out[1]
// h_bus_in[2]   -|    SR-GA1   |- h_bus_out[2]
// h_bus_in[3]   -|    CLB      |- h_bus_out[3]
// shift_clk     -|             |- shift_clk_out
// shift_data_in -|_ _ _ _ _ _ _|- shift_data_out
//                  | | | | | |
//
//                  v v v v c r
//                  | | | | l e
//                  b b b b k s
//                  u u u u   e
//                  s s s s   t
//                  | | | |
//                  i i i i
//                  n n n n
//                  - - - -
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
    input   logic [3:0] horz_bus_in,
    input   logic [3:0] vert_bus_in,
    output  logic       clk_out,
    output  logic       reset_out,
    output  logic [3:0] horz_bus_out,
    output  logic [3:0] vert_bus_out
);

    // Configuration register block
    logic [18:0] reg_data;
    Shift_Reg_No_Reset #(
        .DEPTH(19)
    ) reg_block_u (
        .shift_clk(shift_clk),
        .data_in(shift_data_in),
        .data(reg_data),
        .data_out(shift_data_out)
    );
    
    // Input signals
    logic input_mux_a_sel, input_mux_b_sel, input_mux_c_sel;
    logic input_a, input_b, input_c;
    
    assign input_mux_a_sel = reg_data[0];
    assign input_mux_b_sel = reg_data[1];
    assign input_mux_c_sel = reg_data[2];
    
    // Logic/Arithmetic signals
    logic [7:0] logic_mux_in;
    logic [2:0] operation_select;
    logic [1:0] full_sum_result;
    logic       and_result, or_result, xor_result, nand_result; 
    logic       nor_result, xnor_result, mux_result, carry, partial_sum_result;
    logic       operation_result;
    
    assign operation_select = reg_data[5:3];
    
    // Output signals
    logic [7:0] output_mux_in;
    logic [2:0] minor_horz_sel, minor_vert_sel, major_horz_sel, major_vert_sel;
    
    assign minor_horz_sel = reg_data[8:6];
    assign minor_vert_sel = reg_data[11:9];
    assign major_horz_sel = reg_data[14:12];
    assign major_vert_sel = reg_data[17:15];
    assign shift_clk_out = shift_clk;   // Shift clock passthrough into deeper cells
    assign clk_out = clk;               // Clock passthrough into deeper cells
    assign reset_out = reset;           // Reset passthrough into deeper cells
    
    // Register
    logic operation_ff, op_ff_reset_val;
    assign op_ff_reset_val = reg_data[18];
    
    // Input multiplexers
    always_comb
    begin : input_muxes
        input_a = input_mux_a_sel ? horz_bus_in[1] : horz_bus_in[0];
        input_b = input_mux_b_sel ? horz_bus_in[2] : horz_bus_in[1];
        input_c = input_mux_c_sel ? horz_bus_in[3] : horz_bus_in[2];
    end
    
    //Logic/Arithmetic operations
    always_comb
    begin : arith_logic_core
        nand_result = !(input_a & input_b & input_c);
        nor_result = !(input_a | input_b | input_c);
        xnor_result = !(input_a ^ input_b ^ input_c);
        and_result = !nand_result;
        or_result = !nor_result;
        xor_result = !xnor_result;
        full_sum_result = input_a + input_b + input_c;
        partial_sum_result = full_sum_result[0];
        carry = full_sum_result[1];
        mux_result = input_c ? input_a : input_b;
        
        logic_mux_in = {        // INDEX
            mux_result,         //   7
            partial_sum_result, //   6
            xnor_result,        //   5
            nor_result,         //   4
            nand_result,        //   3
            xor_result,         //   2         
            or_result,          //   1
            and_result          //   0
        };
    end
    
    MUX #(
        .SEL_W(3)
    ) logic_mux_u (
        .in(logic_mux_in),
        .sel(operation_select),
        .out(operation_result)
    );
    
    // Register (synchronous reset)
    always_ff @ (posedge clk)
    begin
        if (reset)
        begin
            operation_ff <= op_ff_reset_val;
        end
        else
        begin
            if (horz_bus_in[0])
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
        horz_bus_out[2] = minor_horz_sel[2] ? horz_bus_in[2] : operation_result;
        
        vert_bus_out[0] = minor_vert_sel[0] ? vert_bus_in[0] : operation_result;
        vert_bus_out[1] = minor_vert_sel[1] ? vert_bus_in[1] : operation_result;
        vert_bus_out[2] = minor_vert_sel[2] ? vert_bus_in[2] : operation_result;
    end
    
    // Major output multiplexers
    always_comb
    begin : major_output_muxes
        output_mux_in = {       // INDEX
            1'b0,               //   7
            carry,              //   6
            operation_ff,       //   5
            operation_result,   //   4
            horz_bus_in[3],     //   3
            vert_bus_in[3],     //   2         
            1'b1,               //   1
            1'b0                //   0
        };
    end
    
    MUX #(
        .SEL_W(3)
    ) major_output_mux_horz_u (
        .in(output_mux_in),
        .sel(major_horz_sel),
        .out(horz_bus_out[3])
    );
    
    MUX #(
        .SEL_W(3)
    ) major_output_mux_vert_u (
        .in(output_mux_in),
        .sel(major_vert_sel),
        .out(vert_bus_out[3])
    );

endmodule