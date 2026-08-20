module Clock_Bank # (
    parameter CLK_NUM
) (
    // Programming interface
    input   logic       shift_clk,
    input   logic       shift_data_in,
    output  logic       shift_data_out,
    
    // Fabric Connections
    input   logic [3:0] buses [CLK_NUM],
    output  logic       clks [CLK_NUM]
);

    logic shift_data [CLK_NUM+1];
    logic prev_clk   [CLK_NUM];
    
    genvar i;
    generate
        for (i = 0; i < CLK_NUM; i++)
        begin : gen_block
            Clock_Selector clk_sel_u (
                .shift_clk(shift_clk),
                .shift_data_in(shift_data[i]),
                .shift_data_out(shift_data[i+1]),
                
                // Fabric Connections
                .bus(buses[i]),
                .prev_clk(prev_clk[i]),
                .clk(clks[i])
            );
            
            always_comb
            begin : clk_routing
                if (i == 0)
                begin
                    prev_clk[i] = clks[CLK_NUM-1];
                end
                else
                begin
                    prev_clk[i] = clks[i-1];
                end
            end
        end
    endgenerate
    
    always_comb
    begin : config_data_routing
        shift_data[0] = shift_data_in;
        shift_data_out = shift_data[CLK_NUM];
    end
    
endmodule