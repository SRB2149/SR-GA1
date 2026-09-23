module count4 (
    input  logic clk,
    input  logic rst,
    output logic [3:0] count
);
    always_ff @(posedge clk)
        if (rst) count <= 4'b0000;
        else     count <= count + 4'b0001;
endmodule
