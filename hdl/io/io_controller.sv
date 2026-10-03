// The FPGA IO controller
//
// FPGA Inputs | Connectivity          | Fabric Inputs
// ------------+-----------------------+------------------
//     15      | Fixed: chip_inputs[9] |                3-
//     14      | Fixed: chip_inputs[8] | Horizontal bus 2-
//     13      | MUX16: FPGA input mux |   to CLB 0:3   1-
//     12      | MUX16: FPGA input mux |                0-
// - - - - - - + - - - - - - - - - - - + - - - - - - - - -
//     11      | Fixed: ddio [1]       |                3-
//     10      | Fixed: ddio [0]       | Horizontal bus 2-
//      9      | MUX16: FPGA input mux |   to CLB 0:2   1-
//      8      | MUX16: FPGA input mux |                0-
// - - - - - - + - - - - - - - - - - - + - - - - - - - - -
//      7      | Fixed: chip_inputs[7] |                3-
//      6      | Fixed: chip_inputs[6] | Horizontal bus 2-
//      5      | MUX16: FPGA input mux |   to CLB 0:1   1-
//      4      | MUX16: FPGA input mux |                0-
// - - - - - - + - - - - - - - - - - - + - - - - - - - - - 
//      3      | Fixed: chip_inputs[3] |                3-
//      2      | Fixed: chip_inputs[2] | Horizontal bus 2-
//      1      | MUX16: FPGA input mux |   to CLB 0:0   1-
//      0      | MUX16: FPGA input mux |                0-
// ------------+-----------------------+------------------
//
// FPGA Outputs | Connectivity
// -------------+-----------------------------
//  ddio_dir[1] | MUX16: FPGA output mux
//  ddio_dir[0] | MUX16: FPGA output mux
//  ddio_out[1] | MUX16: FPGA output mux
//  ddio_out[0] | MUX16: FPGA output mux
// - - - - - - -|- - - - - - - - - - - - - - - 
//      9       | Fixed: from_fabric_buses[15]
//      8       | Fixed: from_fabric_buses[14]
//      7       | Fixed: from_fabric_buses[11]
//      6       | Fixed: from_fabric_buses[10]
//      5       | Fixed: from_fabric_buses[7]
//      4       | Fixed: from_fabric_buses[6]
//      3       | MUX16: FPGA output mux
//      2       | MUX16: FPGA output mux
//      1       | MUX16: FPGA output mux
//      0       | MUX16: FPGA output mux
// -------------+-----------------------------
//
// Notes:
// DDIO directions:
//  - dir = 1 -> output (FPGA-side input tracks output)
//  - dir = 0 -> input
// This means DDIO inputs can act as configurable loopback nets if needed.
// Discard the signals internally if not.
//
// Configure by shifting data into the configuration register block.
// Index | Function
//   0   | input_mux_sel0a[0]
//   1   | input_mux_sel0a[1]
//   2   | input_mux_sel0a[2]
//   3   | input_mux_sel0a[3]
// - - - + - - - - - - - - - - - - - -
//   4   | input_mux_sel0b[0]
//   5   | input_mux_sel0b[1]
//   6   | input_mux_sel0b[2]
//   7   | input_mux_sel0b[3]
// - - - + - - - - - - - - - - - - - -
//   8   | input_mux_sel1a[0]
//   9   | input_mux_sel1a[1]
//   10  | input_mux_sel1a[2]
//   11  | input_mux_sel1a[3]
// - - - + - - - - - - - - - - - - - -
//   12  | input_mux_sel1b[0]
//   13  | input_mux_sel1b[1]
//   14  | input_mux_sel1b[2]
//   15  | input_mux_sel1b[3]
// - - - + - - - - - - - - - - - - - -
//   16  | input_mux_sel2a[0]
//   17  | input_mux_sel2a[1]
//   18  | input_mux_sel2a[2]
//   19  | input_mux_sel2a[3]
// - - - + - - - - - - - - - - - - - -
//   20  | input_mux_sel2b[0]
//   21  | input_mux_sel2b[1]
//   22  | input_mux_sel2b[2]
//   23  | input_mux_sel2b[3]
//   24  | input_mux_sel3a[0]
// - - - + - - - - - - - - - - - - - -
//   25  | input_mux_sel3a[1]
//   26  | input_mux_sel3a[2]
//   27  | input_mux_sel3a[3]
// - - - + - - - - - - - - - - - - - -
//   28  | input_mux_sel3b[0]
//   29  | input_mux_sel3b[1]
//   30  | input_mux_sel3b[2]
//   31  | input_mux_sel3b[3]
// - - - + - - - - - - - - - - - - - -
//   32  | output_mux_sel0[0]
//   33  | output_mux_sel0[1]
//   34  | output_mux_sel0[2]
//   35  | output_mux_sel0[3]
// - - - + - - - - - - - - - - - - - -
//   36  | output_mux_sel1[0]
//   37  | output_mux_sel1[1]
//   38  | output_mux_sel1[2]
//   39  | output_mux_sel1[3]
// - - - + - - - - - - - - - - - - - -
//   40  | output_mux_sel2[0]
//   41  | output_mux_sel2[1]
//   42  | output_mux_sel2[2]
//   43  | output_mux_sel2[3]
// - - - + - - - - - - - - - - - - - -
//   44  | output_mux_sel3[0]
//   45  | output_mux_sel3[1]
//   46  | output_mux_sel3[2]
//   47  | output_mux_sel3[3]
// - - - + - - - - - - - - - - - - - -
//   48  | output_mux_sel_ddio0[0]
//   49  | output_mux_sel_ddio0[1]
//   50  | output_mux_sel_ddio0[2]
//   51  | output_mux_sel_ddio0[3]
// - - - + - - - - - - - - - - - - - -
//   52  | output_mux_sel_ddio1[0]
//   53  | output_mux_sel_ddio1[1]
//   54  | output_mux_sel_ddio1[2]
//   55  | output_mux_sel_ddio1[3]
// - - - + - - - - - - - - - - - - - -
//   56  | output_mux_sel_ddio_dir0[0]
//   57  | output_mux_sel_ddio_dir0[1]
//   58  | output_mux_sel_ddio_dir0[2]
//   59  | output_mux_sel_ddio_dir0[3]
// - - - + - - - - - - - - - - - - - -
//   60  | output_mux_sel_ddio_dir1[0]
//   61  | output_mux_sel_ddio_dir1[1]
//   62  | output_mux_sel_ddio_dir1[2]
//   63  | output_mux_sel_ddio_dir1[3]

module IO_Controller (
    // Programming interface
    input   logic       shift_clk,
    input   logic       shift_data_in,
    output  logic       shift_data_out,
    input   logic       freeze_fabric, // Prevents oscillation through loop-back paths as well
                                       // as ensuring DDIO is input only during programming
    // Chip IO
    input   logic [9:0] chip_inputs,
    output  logic [9:0] chip_outputs,
    
    // Chip Dual-Direction IO
    input   logic [1:0] ddio_in,
    output  logic [1:0] ddio_dir,
    output  logic [1:0] ddio_out,
    
    // Fabric IO
    input   logic [15:0] from_fabric_buses,
    output  logic [15:0] to_fabric_buses
);
    
    // Internal connections
    logic [15:0] input_mux_in;
    logic [9:0]  pregate_chip_outputs;
    logic [3:0]  input_mux_sel0a, input_mux_sel1a, input_mux_sel2a, input_mux_sel3a;
    logic [3:0]  input_mux_sel0b, input_mux_sel1b, input_mux_sel2b, input_mux_sel3b;
    logic [3:0]  output_mux_sel0, output_mux_sel1, output_mux_sel2, output_mux_sel3;
    logic [3:0]  output_mux_sel_ddio0, output_mux_sel_ddio1;
    logic [3:0]  output_mux_sel_ddio_dir0, output_mux_sel_ddio_dir1;
    logic [1:0]  ddio_in_pre_mux;
    logic [1:0]  pregate_ddio_out;
    logic [1:0]  pregate_ddio_dir;
    
    // Register block
    logic [63:0] reg_data;
    Shift_Reg_No_Reset #(
        .DEPTH(64)
    ) reg_block_u (
        .shift_clk(shift_clk),
        .data_in(shift_data_in),
        .data(reg_data),
        .data_out(shift_data_out)
    );
    
    always_comb
    begin : Inputs
        ddio_in_pre_mux = {
            ddio_dir[1] ? ddio_out[1] : ddio_in[1],
            ddio_dir[0] ? ddio_out[0] : ddio_in[0]
        };
        
        // Chip input multiplexer
        // 0  -  9: all chip inputs
        // 10 - 11: all ddio (reads the ddio output if in output mode, reads input otherwise)
        // 12 - 15: first 4 chip outputs
        
        input_mux_in = {
            chip_outputs[3:0],
            ddio_in_pre_mux,
            chip_inputs
        };
        
        input_mux_sel0a = reg_data[3:0];
        input_mux_sel0b = reg_data[7:4];
        
        input_mux_sel1a = reg_data[11:8];
        input_mux_sel1b = reg_data[15:12];
        
        input_mux_sel2a = reg_data[19:16];
        input_mux_sel2b = reg_data[23:20];
        
        input_mux_sel3a = reg_data[27:24];
        input_mux_sel3b = reg_data[31:28];
        
        to_fabric_buses = {
            chip_inputs[9],
            chip_inputs[8],
            input_mux_in[input_mux_sel3b],
            input_mux_in[input_mux_sel3a],
            
            ddio_in_pre_mux[1],
            ddio_in_pre_mux[0],
            input_mux_in[input_mux_sel2b],
            input_mux_in[input_mux_sel2a],
            
            chip_inputs[7],
            chip_inputs[6],
            input_mux_in[input_mux_sel1b],
            input_mux_in[input_mux_sel1a],
            
            chip_inputs[3],
            chip_inputs[2],
            input_mux_in[input_mux_sel0b],
            input_mux_in[input_mux_sel0a]
        };
    end
    
    always_comb
    begin : Outputs
        output_mux_sel0 = reg_data[35:32];
        output_mux_sel1 = reg_data[39:36];
        output_mux_sel2 = reg_data[43:40];
        output_mux_sel3 = reg_data[47:44];
        
        output_mux_sel_ddio0 = reg_data[51:48];
        output_mux_sel_ddio1 = reg_data[55:52];
        
        output_mux_sel_ddio_dir0 = reg_data[59:56];
        output_mux_sel_ddio_dir1 = reg_data[63:60];
        
        pregate_ddio_out = {
            from_fabric_buses[output_mux_sel_ddio1],
            from_fabric_buses[output_mux_sel_ddio0]
        };
        
        pregate_ddio_dir = {
            from_fabric_buses[output_mux_sel_ddio_dir1],
            from_fabric_buses[output_mux_sel_ddio_dir0]
        };
        
        pregate_chip_outputs = {
            from_fabric_buses[15],
            from_fabric_buses[14],
            from_fabric_buses[11],
            from_fabric_buses[10],
            from_fabric_buses[7],
            from_fabric_buses[6],
            
            from_fabric_buses[output_mux_sel3],
            from_fabric_buses[output_mux_sel2],
            from_fabric_buses[output_mux_sel1],
            from_fabric_buses[output_mux_sel0]
        };
        
        // Freeze outputs when freeze_fabric high
        ddio_out = freeze_fabric ? '0 : pregate_ddio_out; 
        ddio_dir = freeze_fabric ? '0 : pregate_ddio_dir; // Set DDIO direction to input when programming
        chip_outputs = freeze_fabric ? '0 : pregate_chip_outputs; 
    end

endmodule