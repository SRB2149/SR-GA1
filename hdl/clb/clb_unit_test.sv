`include "svunit_defines.svh"
`include "clb.sv"
//`define DEBUG_OP
//`define DEBUG_FF

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
    
    //TB Signals
    logic [1:0] exp_sum;
    logic [1:0] data_2bits;
    logic exp_result;
    logic exp_carry;

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
        clk = '0;
        reset = '0;
        shift_data_in = '0;
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
        logic [1:0] minor_horz_sel,
        logic [3:0] minor_vert_sel,
        logic [2:0] major_horz2_sel,
        logic [2:0] major_horz3_sel,
        logic       op_ff_reset_val
    );
        configure({
            op_ff_reset_val,
            major_horz3_sel,
            major_horz2_sel,
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
        configure_all(1'b0, 1'b0, 1'b0, 3'b000, 2'b00, 4'b0000, 3'b000, 3'b000, 1'b0);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_a, 1'b0)
        horz_bus_in = 4'b0001;
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_a, 1'b1)
        configure_all(1'b1, 1'b0, 1'b0, 3'b000, 2'b00, 4'b0000, 3'b000, 3'b000, 1'b0);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_a, 1'b0)
        horz_bus_in = 4'b0010;
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_a, 1'b1)
    `SVTEST_END

    `SVTEST(test_input_b)
        configure_all(1'b0, 1'b0, 1'b0, 3'b000, 2'b00, 4'b0000, 3'b000, 3'b000, 1'b0);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_b, 1'b0)
        horz_bus_in = 4'b0010;
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_b, 1'b1)
        configure_all(1'b0, 1'b1, 1'b0, 3'b000, 2'b00, 4'b0000, 3'b000, 3'b000, 1'b0);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_b, 1'b0)
        horz_bus_in = 4'b0100;
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_b, 1'b1)
    `SVTEST_END
    
    `SVTEST(test_input_c)
        configure_all(1'b0, 1'b0, 1'b0, 3'b000, 2'b00, 4'b0000, 3'b000, 3'b000, 1'b0);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_c, 1'b0)
        horz_bus_in = 4'b0100;
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_c, 1'b1)
        configure_all(1'b0, 1'b0, 1'b1, 3'b000, 2'b00, 4'b0000, 3'b000, 3'b000, 1'b0);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_c, 1'b0)
        horz_bus_in = 4'b1000;
        #1ns;
        `FAIL_UNLESS_EQUAL(my_CLB.input_c, 1'b1)
    `SVTEST_END
    
    `SVTEST(test_clk_passthrough)
        configure(19'd0);
        `FAIL_UNLESS_EQUAL(clk, clk_out)
        clk = '1;
        #1ns;
        `FAIL_UNLESS_EQUAL(clk, clk_out)
        clk = '0;
        #1ns;
        `FAIL_UNLESS_EQUAL(clk, clk_out)
    `SVTEST_END
    
    `SVTEST(test_reset_passthrough)
        configure(19'd0);
        `FAIL_UNLESS_EQUAL(reset, reset_out)
        reset = '1;
        #1ns;
        `FAIL_UNLESS_EQUAL(reset, reset_out)
        reset = '0;
        #1ns;
        `FAIL_UNLESS_EQUAL(reset, reset_out)
    `SVTEST_END
    
    `SVTEST(test_shift_clk_passthrough)
        configure(19'd0);
        shift_clk_enable = '1;
        `FAIL_UNLESS_EQUAL(shift_clk, shift_clk_out)
        @(posedge shift_clk);
        `FAIL_UNLESS_EQUAL(shift_clk, shift_clk_out)
        @(negedge shift_clk);
        `FAIL_UNLESS_EQUAL(shift_clk, shift_clk_out)
        shift_clk_enable = '0;
    `SVTEST_END
    
    `SVTEST(test_operations)
        for (int op = 0; op < 8; op++)
        begin
            configure_all(1'b0, 1'b0, 1'b0, 3'(op), 2'b00, 4'b0000, 3'b000, 3'b000, 1'b0);
            
            for (int data = 0; data < 8; data++)
            begin
                horz_bus_in = {1'b0, 3'(data)};
                
                #1ns;
                
                exp_sum = horz_bus_in[2] + horz_bus_in[1] + horz_bus_in[0];
                exp_carry = exp_sum[1];
                
                case(op)
                    3'b000 : begin //AND3
                        exp_result = horz_bus_in[2] && horz_bus_in[1] && horz_bus_in[0];
                    end
                    
                    3'b001 : begin //OR3
                        exp_result = horz_bus_in[2] || horz_bus_in[1] || horz_bus_in[0];
                    end
                    
                    3'b010 : begin //XOR3
                        exp_result = horz_bus_in[2] ^^ horz_bus_in[1] ^^ horz_bus_in[0];
                    end
                    
                    3'b011 : begin //NAND3
                        exp_result = !(horz_bus_in[2] && horz_bus_in[1] && horz_bus_in[0]);
                    end
                    
                    3'b100 : begin //NOR3
                        exp_result = !(horz_bus_in[2] || horz_bus_in[1] || horz_bus_in[0]);
                    end
                    
                    3'b101 : begin //XNOR3
                        exp_result = !(horz_bus_in[2] ^^ horz_bus_in[1] ^^ horz_bus_in[0]);
                    end
                    
                    3'b110 : begin //SUM1
                        exp_result = exp_sum[0];
                    end
                    
                    3'b111 : begin //MUX2
                        exp_result = horz_bus_in[2] ? horz_bus_in[0] : horz_bus_in[1];
                    end
                endcase
                
                #1ns;
                `ifdef DEBUG_OP
                    $display("%b, %b, %b", my_CLB.input_a, my_CLB.input_b, my_CLB.input_c);
                    $display("%b, %d, %d, %b, %b", horz_bus_in, exp_sum, my_CLB.full_sum_result, exp_carry, my_CLB.carry);
                `endif
                `FAIL_UNLESS_EQUAL(exp_result, my_CLB.operation_result)
                `FAIL_UNLESS_EQUAL(exp_carry, my_CLB.carry)
            end
        end
    `SVTEST_END
    
    `SVTEST(test_operation_ff)
        for (int val = 0; val < 2; val++)
        begin
            configure_all(1'b1, 1'b1, 1'b1, 3'b111, 2'b00, 4'b0000, 3'b000, 3'b000, 1'(val));
            
            //Testing synchronous reset
            reset = '1;
        
            @(posedge clk);
        
            reset = '0;
            
            #1ns;
            
            `ifdef DEBUG_FF
                $display("RESET TEST VALUES: %b, %b, %b", 1'(val), my_CLB.op_ff_reset_val, my_CLB.operation_ff);
            `endif
            
            `FAIL_UNLESS_EQUAL(my_CLB.operation_ff, 1'(val))
            
            //Test setting via fabric after reset
            horz_bus_in = 4'b0001;
            
            @(posedge clk);
            
            #1ns;
            
            `FAIL_UNLESS_EQUAL(my_CLB.operation_ff, 1'b0)
            
            horz_bus_in = 4'b0101;
            
            @(posedge clk);
            
            #1ns;
            
            `FAIL_UNLESS_EQUAL(my_CLB.operation_ff, 1'b1)
        end
    `SVTEST_END
    
    `SVTEST(test_horz_minor_muxes)
        //Test MUX0
        configure_all(1'b1, 1'b1, 1'b1, 3'b001, 2'b01, 4'b0000, 3'b000, 3'b000, 1'b0);
        
        for (int data = 0; data < 4; data++)
        begin
            data_2bits = 2'(data);
            horz_bus_in[0] = data_2bits[1];
            horz_bus_in[1] = data_2bits[0];
            horz_bus_in[2] = data_2bits[0];
            horz_bus_in[3] = data_2bits[0];
            
            #1ns;
            
            `FAIL_UNLESS_EQUAL(horz_bus_out[0], data_2bits[1])
            `FAIL_UNLESS_EQUAL(horz_bus_out[1], data_2bits[0])
        end
        
        //Test MUX1
        configure_all(1'b0, 1'b1, 1'b1, 3'b001, 2'b10, 4'b0000, 3'b000, 3'b000, 1'b0);
        
        for (int data = 0; data < 4; data++)
        begin
            data_2bits = 2'(data);
            horz_bus_in[0] = data_2bits[0];
            horz_bus_in[1] = data_2bits[1];
            horz_bus_in[2] = data_2bits[0];
            horz_bus_in[3] = data_2bits[0];
            
            #1ns;
            
            `FAIL_UNLESS_EQUAL(horz_bus_out[0], data_2bits[0])
            `FAIL_UNLESS_EQUAL(horz_bus_out[1], data_2bits[1])
        end
    `SVTEST_END
    
    `SVTEST(test_vert_minor_muxes)
        for (logic [4:0] mux = 0; mux < 16; mux++)
        begin
            configure_all(1'b1, 1'b1, 1'b1, 3'b001, 2'b00, 4'(mux), 3'b000, 3'b000, 1'b0);
            
            horz_bus_in[0] = '1;
            
            for (logic [5:0] data = 0; data < 32; data++)
            begin
                vert_bus_in = data[3:0];
                horz_bus_in[1] = data[4];
                
                @(posedge clk);
                
                #1ns;
                
                `FAIL_UNLESS_EQUAL(vert_bus_out[0], mux[0] ? data[2] : data[4])
                `FAIL_UNLESS_EQUAL(vert_bus_out[1], mux[1] ? data[3] : data[4])
                `FAIL_UNLESS_EQUAL(vert_bus_out[2], mux[2] ? data[0] : data[4])
                `FAIL_UNLESS_EQUAL(vert_bus_out[3], mux[3] ? data[1] : data[4])
            end
        end
    `SVTEST_END
    
    `SVTEST(test_major_output_muxes)
        //horz_bus_out[2]
        for (int mux = 0; mux < 8; mux++)
        begin
            configure_all(1'b0, 1'b0, 1'b0, 3'b000, 2'b00, 4'b0000, 3'(mux), 3'(mux), 1'b0);
            
            for (logic [3:0] data = 0; data < 8; data++)
            begin
                vert_bus_in[2] = data[0];
                vert_bus_in[3] = data[1];
                horz_bus_in[3] = data[2];
                
                #1ns;
                
                case(mux)
                    3'b000 : begin //Constant 0
                        exp_result = '0;
                    end
                    
                    3'b001 : begin //Constant 1
                        exp_result = '1;
                    end
                    
                    3'b010 : begin //vert_bus_in[2]
                        exp_result = vert_bus_in[2];
                    end
                    
                    3'b011 : begin //vert_bus_in[3]
                        exp_result = vert_bus_in[3];
                    end
                    
                    3'b100 : begin //horz_bus_in[3]
                        exp_result = horz_bus_in[3];
                    end
                    
                    3'b101 : begin //Operation result
                        exp_result = my_CLB.operation_result;
                    end
                    
                    3'b110 : begin //Operation flip-flop
                        exp_result = my_CLB.operation_ff;
                    end
                    
                    3'b111 : begin //Carry
                        exp_result = my_CLB.carry;
                    end
                endcase
                
                #1ns;
                `FAIL_UNLESS_EQUAL(exp_result, horz_bus_out[2])
                `FAIL_UNLESS_EQUAL(exp_result, horz_bus_out[3])
            end
        end
    `SVTEST_END

    `SVUNIT_TESTS_END

endmodule
