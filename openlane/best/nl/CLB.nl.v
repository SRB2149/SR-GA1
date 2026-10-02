module CLB (carry_in,
    carry_out,
    clk,
    clk_out,
    reset,
    reset_out,
    shift_clk,
    shift_clk_out,
    shift_data_in,
    shift_data_out,
    horz_bus_in,
    horz_bus_out,
    vert_bus_in,
    vert_bus_out);
 input carry_in;
 output carry_out;
 input clk;
 output clk_out;
 input reset;
 output reset_out;
 input shift_clk;
 output shift_clk_out;
 input shift_data_in;
 output shift_data_out;
 input [3:0] horz_bus_in;
 output [3:0] horz_bus_out;
 input [3:0] vert_bus_in;
 output [3:0] vert_bus_out;

 wire _00_;
 wire _01_;
 wire _02_;
 wire _03_;
 wire _04_;
 wire _05_;
 wire _06_;
 wire _07_;
 wire _08_;
 wire _09_;
 wire _10_;
 wire _11_;
 wire _12_;
 wire _13_;
 wire _14_;
 wire _15_;
 wire _16_;
 wire _17_;
 wire _18_;
 wire _19_;
 wire _20_;
 wire _21_;
 wire _22_;
 wire _23_;
 wire _24_;
 wire input_mux_a_sel;
 wire input_mux_b_sel;
 wire \input_mux_c_sel[0] ;
 wire \input_mux_c_sel[1] ;
 wire \lut[0] ;
 wire \lut[1] ;
 wire \lut[2] ;
 wire \lut[3] ;
 wire \lut[4] ;
 wire \lut[5] ;
 wire \lut[6] ;
 wire \lut[7] ;
 wire \major_horz2_sel[0] ;
 wire \major_horz2_sel[1] ;
 wire \major_horz2_sel[2] ;
 wire \major_horz3_sel[0] ;
 wire \major_horz3_sel[1] ;
 wire \major_horz3_sel[2] ;
 wire \minor_horz_sel[0] ;
 wire \minor_horz_sel[1] ;
 wire \minor_vert_sel[0] ;
 wire \minor_vert_sel[1] ;
 wire \minor_vert_sel[2] ;
 wire \minor_vert_sel[3] ;
 wire operation_ff;
 wire net1;
 wire net2;
 wire net3;
 wire net4;
 wire net5;
 wire net6;
 wire net7;
 wire net8;
 wire net9;
 wire net10;
 wire net11;
 wire net12;
 wire net13;
 wire net14;
 wire net15;
 wire net16;
 wire net17;
 wire net18;
 wire net19;
 wire net20;
 wire net21;
 wire net22;
 wire net23;
 wire net24;
 wire net25;
 wire net26;
 wire net27;
 wire net28;
 wire clknet_0_clk;
 wire clknet_1_0__leaf_clk;
 wire clknet_1_1__leaf_clk;

 sky130_fd_sc_hd__inv_2 _25_ (.A(net5),
    .Y(_01_));
 sky130_fd_sc_hd__inv_2 _26_ (.A(net6),
    .Y(_02_));
 sky130_fd_sc_hd__inv_2 _27_ (.A(net4),
    .Y(_03_));
 sky130_fd_sc_hd__mux4_2 _28_ (.A0(net4),
    .A1(net9),
    .A2(net5),
    .A3(net1),
    .S0(\input_mux_c_sel[1] ),
    .S1(\input_mux_c_sel[0] ),
    .X(_04_));
 sky130_fd_sc_hd__mux2_1 _29_ (.A0(net3),
    .A1(net4),
    .S(input_mux_b_sel),
    .X(_05_));
 sky130_fd_sc_hd__mux2_1 _30_ (.A0(\lut[0] ),
    .A1(\lut[1] ),
    .S(_04_),
    .X(_06_));
 sky130_fd_sc_hd__and2b_1 _31_ (.A_N(_05_),
    .B(_06_),
    .X(_07_));
 sky130_fd_sc_hd__mux2_1 _32_ (.A0(\lut[2] ),
    .A1(\lut[3] ),
    .S(_04_),
    .X(_08_));
 sky130_fd_sc_hd__mux2_1 _33_ (.A0(net2),
    .A1(net3),
    .S(input_mux_a_sel),
    .X(_09_));
 sky130_fd_sc_hd__inv_2 _34_ (.A(_09_),
    .Y(_10_));
 sky130_fd_sc_hd__a21o_1 _35_ (.A1(_05_),
    .A2(_08_),
    .B1(_09_),
    .X(_11_));
 sky130_fd_sc_hd__mux2_1 _36_ (.A0(\lut[4] ),
    .A1(\lut[5] ),
    .S(_04_),
    .X(_12_));
 sky130_fd_sc_hd__mux2_1 _37_ (.A0(\lut[6] ),
    .A1(\lut[7] ),
    .S(_04_),
    .X(_13_));
 sky130_fd_sc_hd__mux2_1 _38_ (.A0(_12_),
    .A1(_13_),
    .S(_05_),
    .X(_14_));
 sky130_fd_sc_hd__o22a_1 _39_ (.A1(_07_),
    .A2(_11_),
    .B1(_14_),
    .B2(_10_),
    .X(_15_));
 sky130_fd_sc_hd__mux2_1 _40_ (.A0(_15_),
    .A1(net2),
    .S(\minor_horz_sel[0] ),
    .X(net15));
 sky130_fd_sc_hd__mux2_1 _41_ (.A0(_15_),
    .A1(net3),
    .S(\minor_horz_sel[1] ),
    .X(net16));
 sky130_fd_sc_hd__mux2_1 _42_ (.A0(_15_),
    .A1(net11),
    .S(\minor_vert_sel[0] ),
    .X(net22));
 sky130_fd_sc_hd__mux2_1 _43_ (.A0(operation_ff),
    .A1(net12),
    .S(\minor_vert_sel[1] ),
    .X(net23));
 sky130_fd_sc_hd__mux2_1 _44_ (.A0(_15_),
    .A1(net9),
    .S(\minor_vert_sel[2] ),
    .X(net24));
 sky130_fd_sc_hd__mux2_1 _45_ (.A0(operation_ff),
    .A1(net10),
    .S(\minor_vert_sel[3] ),
    .X(net25));
 sky130_fd_sc_hd__mux4_1 _46_ (.A0(_15_),
    .A1(net12),
    .A2(operation_ff),
    .A3(net5),
    .S0(\major_horz2_sel[1] ),
    .S1(\major_horz2_sel[0] ),
    .X(_16_));
 sky130_fd_sc_hd__a21oi_1 _47_ (.A1(net11),
    .A2(\major_horz2_sel[1] ),
    .B1(\major_horz2_sel[0] ),
    .Y(_17_));
 sky130_fd_sc_hd__a31oi_1 _48_ (.A1(_03_),
    .A2(\major_horz2_sel[0] ),
    .A3(\major_horz2_sel[1] ),
    .B1(_17_),
    .Y(_18_));
 sky130_fd_sc_hd__mux2_1 _49_ (.A0(_18_),
    .A1(_16_),
    .S(\major_horz2_sel[2] ),
    .X(net17));
 sky130_fd_sc_hd__mux4_1 _50_ (.A0(_15_),
    .A1(operation_ff),
    .A2(net11),
    .A3(net4),
    .S0(\major_horz3_sel[0] ),
    .S1(\major_horz3_sel[1] ),
    .X(_19_));
 sky130_fd_sc_hd__a21oi_1 _51_ (.A1(net12),
    .A2(\major_horz3_sel[1] ),
    .B1(\major_horz3_sel[0] ),
    .Y(_20_));
 sky130_fd_sc_hd__a31oi_1 _52_ (.A1(_01_),
    .A2(\major_horz3_sel[0] ),
    .A3(\major_horz3_sel[1] ),
    .B1(_20_),
    .Y(_21_));
 sky130_fd_sc_hd__mux2_1 _53_ (.A0(_21_),
    .A1(_19_),
    .S(\major_horz3_sel[2] ),
    .X(net18));
 sky130_fd_sc_hd__o21a_1 _54_ (.A1(_05_),
    .A2(_09_),
    .B1(_04_),
    .X(_22_));
 sky130_fd_sc_hd__a21o_1 _55_ (.A1(_05_),
    .A2(_09_),
    .B1(_22_),
    .X(net13));
 sky130_fd_sc_hd__nand2_1 _56_ (.A(net5),
    .B(_15_),
    .Y(_23_));
 sky130_fd_sc_hd__a21oi_1 _57_ (.A1(_01_),
    .A2(operation_ff),
    .B1(net6),
    .Y(_24_));
 sky130_fd_sc_hd__o2bb2a_1 _58_ (.A1_N(_23_),
    .A2_N(_24_),
    .B1(_02_),
    .B2(net21),
    .X(_00_));
 sky130_fd_sc_hd__dfxtp_1 _59_ (.CLK(clknet_1_0__leaf_clk),
    .D(_00_),
    .Q(operation_ff));
 sky130_fd_sc_hd__dfxtp_1 _60_ (.CLK(net27),
    .D(net8),
    .Q(input_mux_a_sel));
 sky130_fd_sc_hd__dfxtp_1 _61_ (.CLK(net27),
    .D(input_mux_a_sel),
    .Q(input_mux_b_sel));
 sky130_fd_sc_hd__dfxtp_1 _62_ (.CLK(net26),
    .D(input_mux_b_sel),
    .Q(\input_mux_c_sel[0] ));
 sky130_fd_sc_hd__dfxtp_1 _63_ (.CLK(net26),
    .D(\input_mux_c_sel[0] ),
    .Q(\input_mux_c_sel[1] ));
 sky130_fd_sc_hd__dfxtp_1 _64_ (.CLK(net27),
    .D(\input_mux_c_sel[1] ),
    .Q(\lut[0] ));
 sky130_fd_sc_hd__dfxtp_1 _65_ (.CLK(net27),
    .D(\lut[0] ),
    .Q(\lut[1] ));
 sky130_fd_sc_hd__dfxtp_1 _66_ (.CLK(net27),
    .D(\lut[1] ),
    .Q(\lut[2] ));
 sky130_fd_sc_hd__dfxtp_1 _67_ (.CLK(net27),
    .D(\lut[2] ),
    .Q(\lut[3] ));
 sky130_fd_sc_hd__dfxtp_1 _68_ (.CLK(net27),
    .D(\lut[3] ),
    .Q(\lut[4] ));
 sky130_fd_sc_hd__dfxtp_1 _69_ (.CLK(net27),
    .D(\lut[4] ),
    .Q(\lut[5] ));
 sky130_fd_sc_hd__dfxtp_1 _70_ (.CLK(net27),
    .D(\lut[5] ),
    .Q(\lut[6] ));
 sky130_fd_sc_hd__dfxtp_1 _71_ (.CLK(net27),
    .D(\lut[6] ),
    .Q(\lut[7] ));
 sky130_fd_sc_hd__dfxtp_1 _72_ (.CLK(net26),
    .D(\lut[7] ),
    .Q(\minor_horz_sel[0] ));
 sky130_fd_sc_hd__dfxtp_1 _73_ (.CLK(net26),
    .D(\minor_horz_sel[0] ),
    .Q(\minor_horz_sel[1] ));
 sky130_fd_sc_hd__dfxtp_1 _74_ (.CLK(net26),
    .D(\minor_horz_sel[1] ),
    .Q(\minor_vert_sel[0] ));
 sky130_fd_sc_hd__dfxtp_1 _75_ (.CLK(net26),
    .D(\minor_vert_sel[0] ),
    .Q(\minor_vert_sel[1] ));
 sky130_fd_sc_hd__dfxtp_1 _76_ (.CLK(net26),
    .D(\minor_vert_sel[1] ),
    .Q(\minor_vert_sel[2] ));
 sky130_fd_sc_hd__dfxtp_1 _77_ (.CLK(net26),
    .D(\minor_vert_sel[2] ),
    .Q(\minor_vert_sel[3] ));
 sky130_fd_sc_hd__dfxtp_1 _78_ (.CLK(net26),
    .D(\minor_vert_sel[3] ),
    .Q(\major_horz2_sel[0] ));
 sky130_fd_sc_hd__dfxtp_1 _79_ (.CLK(net26),
    .D(\major_horz2_sel[0] ),
    .Q(\major_horz2_sel[1] ));
 sky130_fd_sc_hd__dfxtp_1 _80_ (.CLK(net28),
    .D(\major_horz2_sel[1] ),
    .Q(\major_horz2_sel[2] ));
 sky130_fd_sc_hd__dfxtp_1 _81_ (.CLK(net28),
    .D(\major_horz2_sel[2] ),
    .Q(\major_horz3_sel[0] ));
 sky130_fd_sc_hd__dfxtp_1 _82_ (.CLK(net28),
    .D(\major_horz3_sel[0] ),
    .Q(\major_horz3_sel[1] ));
 sky130_fd_sc_hd__dfxtp_1 _83_ (.CLK(net28),
    .D(\major_horz3_sel[1] ),
    .Q(\major_horz3_sel[2] ));
 sky130_fd_sc_hd__dfxtp_1 _84_ (.CLK(net28),
    .D(\major_horz3_sel[2] ),
    .Q(net21));
 sky130_fd_sc_hd__buf_2 _85_ (.A(clknet_1_1__leaf_clk),
    .X(net14));
 sky130_fd_sc_hd__clkbuf_1 _86_ (.A(net6),
    .X(net19));
 sky130_fd_sc_hd__clkbuf_1 _87_ (.A(net28),
    .X(net20));
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_0_Right_0 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_1_Right_1 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_2_Right_2 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_3_Right_3 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_4_Right_4 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_5_Right_5 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_6_Right_6 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_7_Right_7 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_8_Right_8 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_9_Right_9 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_10_Right_10 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_11_Right_11 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_12_Right_12 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_13_Right_13 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_14_Right_14 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_15_Right_15 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_0_Left_16 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_1_Left_17 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_2_Left_18 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_3_Left_19 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_4_Left_20 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_5_Left_21 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_6_Left_22 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_7_Left_23 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_8_Left_24 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_9_Left_25 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_10_Left_26 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_11_Left_27 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_12_Left_28 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_13_Left_29 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_14_Left_30 ();
 sky130_fd_sc_hd__decap_3 PHY_EDGE_ROW_15_Left_31 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_0_32 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_0_33 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_1_34 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_2_35 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_3_36 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_4_37 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_5_38 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_6_39 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_7_40 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_8_41 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_9_42 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_10_43 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_11_44 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_12_45 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_13_46 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_14_47 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_15_48 ();
 sky130_fd_sc_hd__tapvpwrvgnd_1 TAP_TAPCELL_ROW_15_49 ();
 sky130_fd_sc_hd__clkbuf_1 input1 (.A(carry_in),
    .X(net1));
 sky130_fd_sc_hd__clkbuf_1 input2 (.A(horz_bus_in[0]),
    .X(net2));
 sky130_fd_sc_hd__buf_1 input3 (.A(horz_bus_in[1]),
    .X(net3));
 sky130_fd_sc_hd__buf_1 input4 (.A(horz_bus_in[2]),
    .X(net4));
 sky130_fd_sc_hd__buf_1 input5 (.A(horz_bus_in[3]),
    .X(net5));
 sky130_fd_sc_hd__buf_1 input6 (.A(reset),
    .X(net6));
 sky130_fd_sc_hd__clkbuf_1 input7 (.A(shift_clk),
    .X(net7));
 sky130_fd_sc_hd__clkbuf_1 input8 (.A(shift_data_in),
    .X(net8));
 sky130_fd_sc_hd__clkbuf_1 input9 (.A(vert_bus_in[0]),
    .X(net9));
 sky130_fd_sc_hd__clkbuf_1 input10 (.A(vert_bus_in[1]),
    .X(net10));
 sky130_fd_sc_hd__buf_1 input11 (.A(vert_bus_in[2]),
    .X(net11));
 sky130_fd_sc_hd__buf_1 input12 (.A(vert_bus_in[3]),
    .X(net12));
 sky130_fd_sc_hd__buf_2 output13 (.A(net13),
    .X(carry_out));
 sky130_fd_sc_hd__buf_1 output14 (.A(net14),
    .X(clk_out));
 sky130_fd_sc_hd__buf_2 output15 (.A(net15),
    .X(horz_bus_out[0]));
 sky130_fd_sc_hd__buf_2 output16 (.A(net16),
    .X(horz_bus_out[1]));
 sky130_fd_sc_hd__buf_2 output17 (.A(net17),
    .X(horz_bus_out[2]));
 sky130_fd_sc_hd__buf_2 output18 (.A(net18),
    .X(horz_bus_out[3]));
 sky130_fd_sc_hd__buf_2 output19 (.A(net19),
    .X(reset_out));
 sky130_fd_sc_hd__buf_2 output20 (.A(net20),
    .X(shift_clk_out));
 sky130_fd_sc_hd__buf_2 output21 (.A(net21),
    .X(shift_data_out));
 sky130_fd_sc_hd__buf_2 output22 (.A(net22),
    .X(vert_bus_out[0]));
 sky130_fd_sc_hd__buf_2 output23 (.A(net23),
    .X(vert_bus_out[1]));
 sky130_fd_sc_hd__buf_2 output24 (.A(net24),
    .X(vert_bus_out[2]));
 sky130_fd_sc_hd__buf_2 output25 (.A(net25),
    .X(vert_bus_out[3]));
 sky130_fd_sc_hd__clkbuf_2 fanout26 (.A(net28),
    .X(net26));
 sky130_fd_sc_hd__clkbuf_2 fanout27 (.A(net28),
    .X(net27));
 sky130_fd_sc_hd__clkbuf_2 fanout28 (.A(net7),
    .X(net28));
 sky130_fd_sc_hd__clkbuf_16 clkbuf_0_clk (.A(clk),
    .X(clknet_0_clk));
 sky130_fd_sc_hd__clkbuf_16 clkbuf_1_0__f_clk (.A(clknet_0_clk),
    .X(clknet_1_0__leaf_clk));
 sky130_fd_sc_hd__clkbuf_16 clkbuf_1_1__f_clk (.A(clknet_0_clk),
    .X(clknet_1_1__leaf_clk));
 sky130_fd_sc_hd__clkbuf_4 clkload0 (.A(clknet_1_1__leaf_clk));
 sky130_fd_sc_hd__decap_3 FILLER_0_32 ();
 sky130_fd_sc_hd__fill_2 FILLER_0_38 ();
 sky130_fd_sc_hd__fill_2 FILLER_0_60 ();
 sky130_ef_sc_hd__decap_12 FILLER_1_3 ();
 sky130_fd_sc_hd__decap_4 FILLER_1_15 ();
 sky130_fd_sc_hd__fill_1 FILLER_1_19 ();
 sky130_fd_sc_hd__decap_4 FILLER_1_52 ();
 sky130_fd_sc_hd__decap_6 FILLER_1_57 ();
 sky130_fd_sc_hd__decap_8 FILLER_2_3 ();
 sky130_fd_sc_hd__fill_1 FILLER_2_11 ();
 sky130_fd_sc_hd__decap_4 FILLER_2_29 ();
 sky130_fd_sc_hd__fill_1 FILLER_2_33 ();
 sky130_fd_sc_hd__fill_1 FILLER_3_6 ();
 sky130_fd_sc_hd__fill_1 FILLER_3_23 ();
 sky130_fd_sc_hd__fill_1 FILLER_3_55 ();
 sky130_fd_sc_hd__fill_2 FILLER_4_29 ();
 sky130_fd_sc_hd__fill_1 FILLER_4_61 ();
 sky130_fd_sc_hd__decap_8 FILLER_5_19 ();
 sky130_fd_sc_hd__fill_2 FILLER_5_43 ();
 sky130_fd_sc_hd__fill_1 FILLER_5_50 ();
 sky130_fd_sc_hd__fill_1 FILLER_5_55 ();
 sky130_fd_sc_hd__fill_2 FILLER_5_60 ();
 sky130_fd_sc_hd__decap_4 FILLER_6_6 ();
 sky130_fd_sc_hd__fill_1 FILLER_6_38 ();
 sky130_fd_sc_hd__decap_3 FILLER_6_63 ();
 sky130_ef_sc_hd__decap_12 FILLER_7_19 ();
 sky130_fd_sc_hd__decap_3 FILLER_7_31 ();
 sky130_fd_sc_hd__decap_4 FILLER_8_7 ();
 sky130_fd_sc_hd__fill_1 FILLER_8_11 ();
 sky130_fd_sc_hd__fill_1 FILLER_8_27 ();
 sky130_fd_sc_hd__decap_4 FILLER_9_6 ();
 sky130_fd_sc_hd__fill_1 FILLER_9_10 ();
 sky130_fd_sc_hd__fill_1 FILLER_9_20 ();
 sky130_fd_sc_hd__fill_1 FILLER_9_39 ();
 sky130_fd_sc_hd__fill_1 FILLER_9_57 ();
 sky130_fd_sc_hd__fill_1 FILLER_10_19 ();
 sky130_fd_sc_hd__fill_1 FILLER_10_27 ();
 sky130_fd_sc_hd__fill_1 FILLER_10_41 ();
 sky130_fd_sc_hd__decap_4 FILLER_11_19 ();
 sky130_fd_sc_hd__fill_2 FILLER_11_29 ();
 sky130_fd_sc_hd__fill_1 FILLER_11_65 ();
 sky130_fd_sc_hd__fill_1 FILLER_12_29 ();
 sky130_fd_sc_hd__fill_2 FILLER_13_10 ();
 sky130_fd_sc_hd__decap_6 FILLER_13_28 ();
 sky130_fd_sc_hd__fill_1 FILLER_14_3 ();
 sky130_fd_sc_hd__fill_1 FILLER_14_45 ();
 sky130_fd_sc_hd__fill_1 FILLER_15_3 ();
 sky130_fd_sc_hd__fill_1 FILLER_15_39 ();
endmodule
