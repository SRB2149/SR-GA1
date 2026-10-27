module add5 (
    input  logic [4:0] a,
    output logic [5:0] sum
);
    // Five bits wide, so wider than a four-row column, with operands that
    // share pins: two independent 5-bit operands need more left-edge lanes
    // than the fabric has rows for.
    assign sum = a + {a[3:0], a[4]};
endmodule
