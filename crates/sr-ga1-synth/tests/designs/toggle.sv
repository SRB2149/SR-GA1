module toggle (
    input  logic clk,
    input  logic rst,
    output logic q
);
    always_ff @(posedge clk)
        if (rst) q <= 1'b0;
        else     q <= ~q;
endmodule
