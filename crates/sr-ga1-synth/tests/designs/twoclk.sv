module twoclk (
    input  logic clk_a,
    input  logic clk_b,
    input  logic rst,
    input  logic d,
    output logic q_a,
    output logic q_b
);
    always_ff @(posedge clk_a)
        if (rst) q_a <= 1'b0;
        else     q_a <= d;

    always_ff @(posedge clk_b)
        if (rst) q_b <= 1'b0;
        else     q_b <= ~d;
endmodule
