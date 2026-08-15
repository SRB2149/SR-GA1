`include "svunit_defines.svh"
`include "clb.sv"

module CLB_unit_test;
    import svunit_pkg::svunit_testcase;

    string name = "CLB_ut";
    svunit_testcase svunit_ut;


    //===================================
    // Signals wired to the UUT ports
    //===================================
    logic shift_clk;
    logic shift_clk_out;
    logic shift_clk_pre_enable;
    logic shift_clk_enable;
    logic shift_data_in;
    logic shift_data_out;
    logic clk;
    logic clk_out;
    logic reset;
    logic reset_out;
    logic [3:0] horz_bus_in;
    logic [3:0] vert_bus_in;
    logic [3:0] horz_bus_out;
    logic [3:0] vert_bus_out;
    
    `SVUNIT_CLK_GEN(shift_clk_pre_enable, 5ns)
    `SVUNIT_CLK_GEN(clk, 5ns)
    
    assign shift_clk = shift_clk_enable && shift_clk_pre_enable;

    //===================================
    // This is the UUT that we're
    // running the Unit Tests on
    //===================================
    CLB my_CLB (
        .shift_clk(shift_clk),
        .shift_clk_out(shift_clk_out),
        .shift_data_in(shift_data_in),
        .shift_data_out(shift_data_out),
        .clk(clk),
        .clk_out(clk_out),
        .reset(reset),
        .reset_out(reset_out),
        .horz_bus_in(horz_bus_in),
        .vert_bus_in(vert_bus_in),
        .horz_bus_out(horz_bus_out),
        .vert_bus_out(vert_bus_out)
    );




    //===================================
    // Build
    //===================================
    function void build();
        svunit_ut = new(name);
    endfunction


    //===================================
    // Setup for running the Unit Tests
    //===================================
    task setup();
        svunit_ut.setup();
        horz_bus_in = '0;
        vert_bus_in = '0;
    endtask


    //===================================
    // Here we deconstruct anything we 
    // need after running the Unit Tests
    //===================================
    task teardown();
        svunit_ut.teardown();
        /* Place Teardown Code Here */

    endtask


    //===================================
    // All tests are defined between the
    // SVUNIT_TESTS_BEGIN/END macros
    //
    // Each individual test must be
    // defined between `SVTEST(_NAME_)
    // `SVTEST_END
    //
    // i.e.
    //   `SVTEST(mytest)
    //     <test code>
    //   `SVTEST_END
    //===================================

    task automatic shift_in(logic bit_val);
        shift_data_in = bit_val;
        @(posedge shift_clk);
    endtask
    
    task automatic configure(logic[18:0] config_val);
        shift_clk_enable = '1;
        
        for (int i = 0; i < 19; i++)
        begin
            shift_in(config_val[18-i]);
        end
        
        shift_clk_enable = '0;
    endtask
    
    task automatic configure_all(
        logic       input_mux_a_sel, 
        logic       input_mux_b_sel, 
        logic       input_mux_c_sel,
        logic [2:0] operation_select,
        logic [2:0] minor_horz_sel,
        logic [2:0] minor_vert_sel,
        logic [2:0] major_horz_sel,
        logic [2:0] major_vert_sel,
        logic       op_ff_reset_val
    );
        configure({
            op_ff_reset_val,
            major_vert_sel,
            major_horz_sel,
            minor_vert_sel,
            minor_horz_sel,
            operation_select,
            input_mux_c_sel,
            input_mux_b_sel,
            input_mux_a_sel
        });
    endtask

    `SVUNIT_TESTS_BEGIN

    `SVTEST(test_configure)
        configure(19'h7F0F0);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.reg_data, 19'h7F0F0)
        `FAIL_UNLESS_EQUAL(shift_data_out, 1'b1)
        #5ns;
    `SVTEST_END

    `SVTEST(test_input_a)
        configure_all(1'b0, 1'b0, 1'b0, 3'b000, 3'b000, 3'b000, 3'b000, 3'b000, 1'b0);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_a, 1'b0)
        horz_bus_in = 4'b0001;
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_a, 1'b1)
        configure_all(1'b1, 1'b0, 1'b0, 3'b000, 3'b000, 3'b000, 3'b000, 3'b000, 1'b0);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_a, 1'b0)
        horz_bus_in = 4'b0010;
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_a, 1'b1)
    `SVTEST_END

    `SVTEST(test_input_b)
        configure_all(1'b0, 1'b0, 1'b0, 3'b000, 3'b000, 3'b000, 3'b000, 3'b000, 1'b0);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_b, 1'b0)
        horz_bus_in = 4'b0010;
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_b, 1'b1)
        configure_all(1'b0, 1'b1, 1'b0, 3'b000, 3'b000, 3'b000, 3'b000, 3'b000, 1'b0);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_b, 1'b0)
        horz_bus_in = 4'b0100;
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_b, 1'b1)
    `SVTEST_END
    
    `SVTEST(test_input_c)
        configure_all(1'b0, 1'b0, 1'b0, 3'b000, 3'b000, 3'b000, 3'b000, 3'b000, 1'b0);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_c, 1'b0)
        horz_bus_in = 4'b0100;
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_c, 1'b1)
        configure_all(1'b0, 1'b0, 1'b1, 3'b000, 3'b000, 3'b000, 3'b000, 3'b000, 1'b0);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_c, 1'b0)
        horz_bus_in = 4'b1000;
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_c, 1'b1)
    `SVTEST_END

    `SVUNIT_TESTS_END

endmodule
