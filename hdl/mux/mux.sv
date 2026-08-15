module MUX #(
    SEL_W
) (
    input   logic [2**SEL_W-1:0]    in,
    input   logic [SEL_W-1:0]       sel,
    output  logic                   out
);

    assign out = in[sel];

endmodule