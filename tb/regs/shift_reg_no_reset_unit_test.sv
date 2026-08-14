`include "svunit_defines.svh"
`include "../../hdl/regs/shift_reg_no_reset.sv"

module Shift_Reg_No_Reset_unit_test;
    import svunit_pkg::svunit_testcase;

    string name = "Shift_Reg_No_Reset_ut";
    svunit_testcase svunit_ut;


    //===================================
    // Parameters (fill in real values)
    //===================================
    localparam DEPTH = 4;

    //===================================
    // Signals wired to the UUT ports
    //===================================
    logic shift_clk;
    logic data_in;
    logic [DEPTH-1:0] data;
    logic data_out;
    
    `SVUNIT_CLK_GEN(shift_clk, 5ns)

    //===================================
    // This is the UUT that we're
    // running the Unit Tests on
    //===================================
    Shift_Reg_No_Reset #(
        .DEPTH(DEPTH)
    ) my_Shift_Reg_No_Reset (
        .shift_clk(shift_clk),
        .data_in(data_in),
        .data(data),
        .data_out(data_out)
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
        data_in = 0;

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
    // Helper: shift a known bit into the
    // register on each clock edge
    //===================================
    task automatic shift_in(logic bit_val);
        data_in = bit_val;
        @(posedge shift_clk);
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
    `SVUNIT_TESTS_BEGIN

    //-----------------------------------
    // Since there's no reset, the
    // register's contents are unknown
    // until DEPTH bits have been shifted
    // in -- this fully flushes any
    // initial garbage and checks the
    // known pattern lands correctly.
    //-----------------------------------
    `SVTEST(test_shifts_in_known_pattern)
        // Drive oldest-to-newest: 1, 0, 1, 1
        shift_in(1'b1);
        shift_in(1'b0);
        shift_in(1'b1);
        shift_in(1'b1);
        #1;
        `FAIL_UNLESS_EQUAL(data, 4'b1011)
    `SVTEST_END

    //-----------------------------------
    // data_out should track the oldest
    // bit still in the register, i.e.
    // the input from DEPTH cycles ago
    //-----------------------------------
    `SVTEST(test_data_out_matches_oldest_bit)
        shift_in(1'b1);   // this bit will be data_out after 3 more shifts
        shift_in(1'b0);
        shift_in(1'b0);
        shift_in(1'b0);
        #1;
        `FAIL_UNLESS_EQUAL(data_out, 1'b1)
    `SVTEST_END

    //-----------------------------------
    // Sanity check: a single new bit
    // should always appear at data[0]
    // immediately after one clock edge
    //-----------------------------------
    `SVTEST(test_new_bit_enters_at_lsb)
        shift_in(1'b1);
        shift_in(1'b1);
        shift_in(1'b1);
        shift_in(1'b0);   // most recent bit
        #1;
        `FAIL_UNLESS_EQUAL(data[0], 1'b0)
    `SVTEST_END

    `SVUNIT_TESTS_END

endmodule
