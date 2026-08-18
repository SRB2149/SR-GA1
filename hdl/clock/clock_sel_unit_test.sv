`include "svunit_defines.svh"
`include "clock_sel.sv"

module Clock_Selector_unit_test;
    import svunit_pkg::svunit_testcase;

    string name = "Clock_Selector_ut";
    svunit_testcase svunit_ut;


    //===================================
    // Signals wired to the UUT ports
    //===================================
    logic shift_clk;
    logic shift_clk_enable;
    logic shift_clk_pre_enable;
    logic shift_data_in;
    logic shift_data_out;
    logic [3:0] bus;
    logic prev_clk;
    logic clk;
    
    `SVUNIT_CLK_GEN(shift_clk_pre_enable, 5ns)
    
    assign shift_clk = shift_clk_enable && shift_clk_pre_enable;

    //===================================
    // This is the UUT that we're
    // running the Unit Tests on
    //===================================
    Clock_Selector my_Clock_Selector (
        .shift_clk(shift_clk),
        .shift_data_in(shift_data_in),
        .shift_data_out(shift_data_out),
        .bus(bus),
        .prev_clk(prev_clk),
        .clk(clk)
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
  
    task automatic shift_in(logic bit_val);
        shift_data_in = bit_val;
        @(posedge shift_clk);
    endtask
    
    task automatic configure(logic[2:0] config_val);
        shift_clk_enable = '1;
        
        for (int i = 0; i < 3; i++)
        begin
            shift_in(config_val[2-i]);
        end
        
        #1ns;
        
        shift_clk_enable = '0;
    endtask
    
    task automatic check(int sel);
        logic [4:0] inputs;
        
        for (int i=0; i < 32; i++)
        begin
            inputs = 5'(i);
            
            bus = inputs[3:0];
            prev_clk = inputs[4];
            
            #1ns;
            
            case (sel)
                0,1,2,3 : begin
                    `FAIL_UNLESS_EQUAL(clk, bus[sel])
                end
                
                4,5,6,7 : begin
                    `FAIL_UNLESS_EQUAL(clk, prev_clk)
                end
            endcase
        end
        
    endtask
    
    `SVUNIT_TESTS_BEGIN

    `SVTEST(test_input_selection)
        for (int i=0; i < 8; i++)
        begin
            configure(3'(i));
            
            check(i);
        end
    `SVTEST_END

    `SVUNIT_TESTS_END

endmodule
