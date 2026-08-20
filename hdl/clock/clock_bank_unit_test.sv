`include "svunit_defines.svh"
`include "clock_bank.sv"
//`define DEBUG

module Clock_Bank_unit_test;
    import svunit_pkg::svunit_testcase;

    string name = "Clock_Bank_ut";
    svunit_testcase svunit_ut;


    //===================================
    // Parameters (fill in real values)
    //===================================
    localparam CLK_NUM = 7;

    //===================================
    // Signals wired to the UUT ports
    //===================================
    logic shift_clk;
    logic shift_data_in;
    logic shift_data_out;
    logic [3:0] buses [CLK_NUM];
    logic clks [CLK_NUM];

    logic shift_clk_pre_enable;
    logic shift_clk_enable;
    
    `SVUNIT_CLK_GEN(shift_clk_pre_enable, 5ns)
    
    assign shift_clk = shift_clk_enable && shift_clk_pre_enable;
    
    logic [2:0] reg_data_probe [CLK_NUM-1:0];

    genvar gi;
    generate
      for (gi = 0; gi < CLK_NUM; gi++) begin : probe_block
        assign reg_data_probe[gi] = my_Clock_Bank.gen_block[gi].clk_sel_u.reg_data;
      end
    endgenerate

    //===================================
    // This is the UUT that we're
    // running the Unit Tests on
    //===================================
    Clock_Bank #(
        .CLK_NUM(CLK_NUM)
    ) my_Clock_Bank (
        .shift_clk(shift_clk),
        .shift_data_in(shift_data_in),
        .shift_data_out(shift_data_out),
        .buses(buses),
        .clks(clks)
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
        buses = '{4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0};
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
    
    task automatic configure(logic[20:0] config_val);
        shift_clk_enable = '1;
        
        for (int i = 0; i < 21; i++)
        begin
            shift_in(config_val[20-i]);
        end
        
        #1ns;
        
        shift_clk_enable = '0;
    endtask
    
    `SVUNIT_TESTS_BEGIN

    `SVTEST(test_config)
        configure(21'b000_001_010_011_100_101_110);
        
        for (int i = 0; i < 7; i++)
        begin
            `ifdef DEBUG
                $display("%b", reg_data_probe[6-i]);
            `endif
            `FAIL_UNLESS_EQUAL(3'(i), reg_data_probe[6-i])
        end
        
        #15ns;
    `SVTEST_END
    
    `SVTEST(test_clk_passthrough)
        configure(21'b100_011_000_000_000_000_000);
        
        #1ns;
        
        buses[5][3] = '1;
        
        #1ns;
        
        `FAIL_UNLESS_EQUAL('1, clks[5])
        `FAIL_UNLESS_EQUAL('1, clks[6])
        
        buses[5][3] = '0;
        
        #1ns;
        
        `FAIL_UNLESS_EQUAL('0, clks[5])
        `FAIL_UNLESS_EQUAL('0, clks[6])
    `SVTEST_END

    `SVUNIT_TESTS_END

endmodule
