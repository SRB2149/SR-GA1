module dff1 (
    input  logic clk,
    input  logic rst,
    input  logic d,
    output logic q
);
    always_ff @(posedge clk)
        if (rst) q <= 1'b0;
        else     q <= d;
endmodule
