module MUX #(
    W,
    SEL_W
) (
    input   logic [W-1:0]       in [2**SEL_W-1:0],
    input   logic [SEL_W-1:0]   sel,
    output  logic [W-1:0]       out
);

    assign out = in[sel];

endmodule