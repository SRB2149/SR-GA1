module shift4 (
    input  logic clk,
    input  logic rst,
    input  logic din,
    output logic dout
);
    logic [3:0] sr;

    always_ff @(posedge clk)
        if (rst) sr <= 4'b0000;
        else     sr <= {sr[2:0], din};

    assign dout = sr[3];
endmodule
