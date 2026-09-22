module IO_Controller (
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
    
    always_comb
    begin : Inputs
        to_fabric_buses[3:0] = chip_inputs[3:0];
        to_fabric_buses[7:4] = chip_inputs[7:4];
        to_fabric_buses[9:8] = chip_inputs[9:8];
        to_fabric_buses[11:10] = {
            ddio_dir[1] ? '0 : ddio_in[1],
            ddio_dir[0] ? '0 : ddio_in[0]
        };
        to_fabric_buses[15:12] = 4'b1010;
    end
    
    always_comb
    begin : Outputs
        chip_outputs[3:0] = from_fabric_buses[3:0];
        chip_outputs[7:4] = from_fabric_buses[7:4];
        chip_outputs[9:8] = from_fabric_buses[9:8];
        ddio_out          = from_fabric_buses[11:10];
        ddio_dir          = from_fabric_buses[13:12];
    end

endmodule