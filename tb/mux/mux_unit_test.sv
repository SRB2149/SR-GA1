`include "svunit_defines.svh"
`include "../../hdl/mux/mux.sv"

module MUX_unit_test;
    import svunit_pkg::svunit_testcase;

    string name = "MUX_ut";
    svunit_testcase svunit_ut;


    //===================================
    // Parameters (fill in real values)
    //===================================
    localparam W = 1;  /* TODO: set value */
    localparam SEL_W = 1;  /* TODO: set value */

    //===================================
    // Signals wired to the UUT ports
    //===================================
    logic [W-1:0] in [2**SEL_W-1:0];
    logic [SEL_W-1:0] sel;
    logic [W-1:0] out;

    //===================================
    // This is the UUT that we're
    // running the Unit Tests on
    //===================================
    MUX #(
        .W(W),
    .SEL_W(SEL_W)
    ) my_MUX (
        .in(in),
        .sel(sel),
        .out(out)
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
        /* Place Setup Code Here */

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
    `SVUNIT_TESTS_BEGIN

    //===================================
    // Give each input a distinct, known value before every test
    //===================================
    task automatic drive_known_inputs();
        for (int i = 0; i < 2**SEL_W; i++) begin
          in[i] = i + 1;   // e.g. in[0]=1, in[1]=2, in[2]=3, in[3]=4
        end
    endtask

    `SVTEST(test_selects_each_input)
        drive_known_inputs();
        for (int i = 0; i < 2**SEL_W; i++) begin
          sel = i;
          #1;  // let the combinational `assign` settle
          `FAIL_UNLESS_EQUAL(out, in[i])
        end
    `SVTEST_END

    `SVTEST(test_out_changes_when_sel_changes)
        drive_known_inputs();
        sel = 0;
        #1;
        `FAIL_UNLESS_EQUAL(out, in[0])

        sel = 1;
        #1;
        `FAIL_IF_EQUAL(out, in[0])
        `FAIL_UNLESS_EQUAL(out, in[1])
    `SVTEST_END

    `SVUNIT_TESTS_END

endmodule
