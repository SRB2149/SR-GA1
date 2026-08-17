

module IO_Controller (
    // Chip IO
    input   logic [9:0] chip_inputs,
    output  logic [9:0] chip_outputs,
    
    // Chip Dual-Direction IO
    input   logic [1:0] ddio_in,
    output  logic [1:0] ddio_dir,
    output  logic [1:0] ddio_out,
    
    // Fabric IO
    input   logic [3:0] from_fabric_buses [4],
    output  logic [3:0] to_fabric_buses [4]
);
    
    always_comb
    begin : Inputs
        to_fabric_buses[0] = chip_inputs[3:0];
        to_fabric_buses[1] = chip_inputs[7:4];
        to_fabric_buses[2][1:0] = chip_inputs[9:8];
        to_fabric_buses[2][3:2] = {
            ddio_dir[1] ? '0 : ddio_in[1],
            ddio_dir[0] ? '0 : ddio_in[0]
        };
        to_fabric_buses[3] = 4'b1010;
    end
    
    always_comb
    begin : Outputs
        chip_outputs[3:0] = from_fabric_buses[0];
        chip_outputs[7:4] = from_fabric_buses[1];
        chip_outputs[9:8] = from_fabric_buses[2][1:0];
        ddio_out          = from_fabric_buses[2][3:2];
        ddio_dir          = from_fabric_buses[3][1:0];
    end

endmodule