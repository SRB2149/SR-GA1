`include "svunit_defines.svh"
`include "io_controller.sv"

module IO_Controller_unit_test;
    import svunit_pkg::svunit_testcase;

    string name = "IO_Controller_ut";
    svunit_testcase svunit_ut;


    //===================================
    // Signals wired to the UUT ports
    //===================================
    logic shift_clk;
    logic shift_clk_pre_enable;
    logic shift_clk_enable;
    logic shift_data_in;
    logic shift_data_out;
    logic freeze_fabric;
    logic [9:0] chip_inputs;
    logic [9:0] chip_outputs;
    logic [1:0] ddio_in;
    logic [1:0] ddio_dir;
    logic [1:0] ddio_out;
    logic [15:0] from_fabric_buses;
    logic [15:0] to_fabric_buses;
    
    `SVUNIT_CLK_GEN(shift_clk_pre_enable, 5ns)
    
    assign shift_clk = shift_clk_enable && shift_clk_pre_enable;
    
    //TB Signals
    logic [1:0] dirs;
    logic [3:0] data_temp;
    logic [7:0] mux_data;
    logic [15:0] mux_src;
    logic exp_result;

    //===================================
    // This is the UUT that we're
    // running the Unit Tests on
    //===================================
    IO_Controller my_IO_Controller (
        .shift_clk(shift_clk),
        .shift_data_in(shift_data_in),
        .shift_data_out(shift_data_out),
        .freeze_fabric(freeze_fabric),
        .chip_inputs(chip_inputs),
        .chip_outputs(chip_outputs),
        .ddio_in(ddio_in),
        .ddio_dir(ddio_dir),
        .ddio_out(ddio_out),
        .from_fabric_buses(from_fabric_buses),
        .to_fabric_buses(to_fabric_buses)
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
        
        chip_inputs = '0;
        ddio_in = '0;
        from_fabric_buses = '0;
        freeze_fabric = '0;
        shift_clk_enable = '0;
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
    
    task automatic configure(logic[63:0] config_val);
        shift_clk_enable = '1;
        
        for (int i = 0; i < 64; i++)
        begin
            shift_in(config_val[63-i]);
        end
        
        shift_clk_enable = '0;
    endtask
    
    task automatic configure_all(
        logic [3:0] input_mux_sel0a,
        logic [3:0] input_mux_sel0b,
        logic [3:0] input_mux_sel1a,
        logic [3:0] input_mux_sel1b,
        logic [3:0] input_mux_sel2a,
        logic [3:0] input_mux_sel2b,
        logic [3:0] input_mux_sel3a,
        logic [3:0] input_mux_sel3b,
        logic [3:0] output_mux_sel0,
        logic [3:0] output_mux_sel1,
        logic [3:0] output_mux_sel2,
        logic [3:0] output_mux_sel3,
        logic [3:0] output_mux_sel_ddio0,
        logic [3:0] output_mux_sel_ddio1,
        logic [3:0] output_mux_sel_ddio_dir0,
        logic [3:0] output_mux_sel_ddio_dir1
    );
        configure({
            output_mux_sel_ddio_dir1,
            output_mux_sel_ddio_dir0,
            output_mux_sel_ddio1,
            output_mux_sel_ddio0,
            output_mux_sel3,
            output_mux_sel2,
            output_mux_sel1,
            output_mux_sel0,
            input_mux_sel3b,
            input_mux_sel3a,
            input_mux_sel2b,
            input_mux_sel2a,
            input_mux_sel1b,
            input_mux_sel1a,
            input_mux_sel0b,
            input_mux_sel0a
        });
    endtask
    
    // Drives the 16 input mux sources. Requires output_mux_sel0-3 = 0-3 so that
    // chip_outputs[3:0] = from_fabric_buses[3:0], and the ddio_dir muxes to select
    // a fabric bus held at 0 so the ddio inputs are not looped back.
    task automatic set_input_mux_sources(logic [15:0] mux_sources);
        chip_inputs = mux_sources[9:0];
        ddio_in = mux_sources[11:10];
        from_fabric_buses[3:0] = mux_sources[15:12];
    endtask

    `SVUNIT_TESTS_BEGIN

    `SVTEST(test_configure)
        configure(64'hF0E1D2C3B4A59687);
        #1ns;
        `FAIL_UNLESS_EQUAL(my_IO_Controller.reg_data, 64'hF0E1D2C3B4A59687)
        `FAIL_UNLESS_EQUAL(shift_data_out, 1'b1)
        #5ns;
    `SVTEST_END
    
    `SVTEST(test_fixed_inputs)
        //ddio_dir muxes select from_fabric_buses[0] (held at 0) so ddio pins are inputs
        configure(64'd0);
        
        for (int i=0; i<2**12; i++)
        begin
            {ddio_in, chip_inputs} = 12'(i);
            
            #1ns;
            
            `FAIL_UNLESS_EQUAL(chip_inputs[9:8], to_fabric_buses[15:14])
            `FAIL_UNLESS_EQUAL(ddio_in, to_fabric_buses[11:10])
            `FAIL_UNLESS_EQUAL(chip_inputs[7:6], to_fabric_buses[7:6])
            `FAIL_UNLESS_EQUAL(chip_inputs[3:2], to_fabric_buses[3:2])
        end
        
        #1ns;
    `SVTEST_END
    
    `SVTEST(test_input_muxes)
        for (int sel=0; sel<16; sel++)
        begin
            configure_all(
                4'(sel), 4'(sel), 4'(sel), 4'(sel), 4'(sel), 4'(sel), 4'(sel), 4'(sel),
                4'd0, 4'd1, 4'd2, 4'd3, 4'd4, 4'd4, 4'd4, 4'd4
            );
            
            //Walking one then walking zero on the selected source
            for (int pattern=0; pattern<2; pattern++)
            begin
                mux_src = pattern ? ~(16'b1 << sel) : (16'b1 << sel);
                set_input_mux_sources(mux_src);
                exp_result = mux_src[sel];
                
                #1ns;
                
                `FAIL_UNLESS_EQUAL(mux_src, my_IO_Controller.input_mux_in)
                `FAIL_UNLESS_EQUAL({2{exp_result}}, to_fabric_buses[1:0])
                `FAIL_UNLESS_EQUAL({2{exp_result}}, to_fabric_buses[5:4])
                `FAIL_UNLESS_EQUAL({2{exp_result}}, to_fabric_buses[9:8])
                `FAIL_UNLESS_EQUAL({2{exp_result}}, to_fabric_buses[13:12])
            end
        end
        
        #1ns;
    `SVTEST_END
    
    `SVTEST(test_input_mux_select_bits)
        //Each input mux selects a different chip input to check the configuration mapping
        configure_all(
            4'd0, 4'd1, 4'd2, 4'd3, 4'd4, 4'd5, 4'd6, 4'd7,
            4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0
        );
        
        for (int data=0; data<2**8; data++)
        begin
            mux_data = 8'(data);
            chip_inputs = {2'b00, mux_data};
            
            #1ns;
            
            `FAIL_UNLESS_EQUAL(mux_data[1:0], to_fabric_buses[1:0])
            `FAIL_UNLESS_EQUAL(mux_data[3:2], to_fabric_buses[5:4])
            `FAIL_UNLESS_EQUAL(mux_data[5:4], to_fabric_buses[9:8])
            `FAIL_UNLESS_EQUAL(mux_data[7:6], to_fabric_buses[13:12])
        end
        
        #1ns;
    `SVTEST_END
    
    `SVTEST(test_fixed_outputs)
        configure(64'd0);
        
        for (int i=0; i<2**16; i++)
        begin
            from_fabric_buses = 16'(i);
            
            #1ns;
            
            `FAIL_UNLESS_EQUAL(from_fabric_buses[15:14], chip_outputs[9:8])
            `FAIL_UNLESS_EQUAL(from_fabric_buses[11:10], chip_outputs[7:6])
            `FAIL_UNLESS_EQUAL(from_fabric_buses[7:6], chip_outputs[5:4])
        end
        
        #1ns;
    `SVTEST_END
    
    `SVTEST(test_output_muxes)
        for (int sel=0; sel<16; sel++)
        begin
            configure_all(
                4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0,
                4'(sel), 4'(sel), 4'(sel), 4'(sel), 4'(sel), 4'(sel), 4'(sel), 4'(sel)
            );
            
            //Walking one then walking zero on the selected fabric bus
            for (int pattern=0; pattern<2; pattern++)
            begin
                from_fabric_buses = pattern ? ~(16'b1 << sel) : (16'b1 << sel);
                exp_result = from_fabric_buses[sel];
                
                #1ns;
                
                `FAIL_UNLESS_EQUAL({4{exp_result}}, chip_outputs[3:0])
                `FAIL_UNLESS_EQUAL({2{exp_result}}, ddio_out)
                `FAIL_UNLESS_EQUAL({2{exp_result}}, ddio_dir)
            end
        end
        
        #1ns;
    `SVTEST_END
    
    `SVTEST(test_output_mux_select_bits)
        //Each output mux selects a different fabric bus to check the configuration mapping
        configure_all(
            4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0,
            4'd0, 4'd1, 4'd2, 4'd3, 4'd4, 4'd5, 4'd6, 4'd7
        );
        
        for (int data=0; data<2**8; data++)
        begin
            mux_data = 8'(data);
            from_fabric_buses = {8'h00, mux_data};
            
            #1ns;
            
            `FAIL_UNLESS_EQUAL(mux_data[3:0], chip_outputs[3:0])
            `FAIL_UNLESS_EQUAL(mux_data[5:4], ddio_out)
            `FAIL_UNLESS_EQUAL(mux_data[7:6], ddio_dir)
        end
        
        #1ns;
    `SVTEST_END
    
    `SVTEST(test_dual_dir)
        //ddio_out from from_fabric_buses[11:10], ddio_dir from from_fabric_buses[13:12]
        configure_all(
            4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0, 4'd0,
            4'd0, 4'd0, 4'd0, 4'd0, 4'd10, 4'd11, 4'd12, 4'd13
        );
        
        for (int i=0; i<4; i++)
        begin
            dirs = 2'(i);
            from_fabric_buses[13:12] = dirs;
            
            for (int data=0; data<16; data++)
            begin
                data_temp = 4'(data);
                ddio_in = data_temp[1:0];
                from_fabric_buses[11:10] = data_temp[3:2];
                
                #1ns;
                
                `FAIL_UNLESS_EQUAL(dirs, ddio_dir)
                
                //Outputs are looped back into the fabric when set as an output
                `FAIL_UNLESS_EQUAL(dirs[0] ? ddio_out[0] : ddio_in[0], to_fabric_buses[10])
                `FAIL_UNLESS_EQUAL(dirs[1] ? ddio_out[1] : ddio_in[1], to_fabric_buses[11])
                
                `FAIL_UNLESS_EQUAL(from_fabric_buses[10], ddio_out[0])
                `FAIL_UNLESS_EQUAL(from_fabric_buses[11], ddio_out[1])
            end
        end
        
        #1ns;
    `SVTEST_END
    
    `SVTEST(test_freeze)
        //Input muxes select the loopback sources (chip outputs and ddio) and two chip inputs
        configure_all(
            4'd12, 4'd13, 4'd14, 4'd15, 4'd10, 4'd11, 4'd0, 4'd1,
            4'd0, 4'd1, 4'd2, 4'd3, 4'd4, 4'd5, 4'd6, 4'd7
        );
        
        freeze_fabric = '1;
        
        for (int i=0; i<2**12; i++)
        begin
            {ddio_in, chip_inputs} = 12'(i);
            from_fabric_buses = ~16'(i);
            
            #1ns;
            
            //All outputs forced low and DDIO forced to input while programming
            `FAIL_UNLESS_EQUAL(10'd0, chip_outputs)
            `FAIL_UNLESS_EQUAL(2'b00, ddio_out)
            `FAIL_UNLESS_EQUAL(2'b00, ddio_dir)
            
            //Loopback paths are broken
            `FAIL_UNLESS_EQUAL(2'b00, to_fabric_buses[1:0])
            `FAIL_UNLESS_EQUAL(2'b00, to_fabric_buses[5:4])
            `FAIL_UNLESS_EQUAL(ddio_in, to_fabric_buses[9:8])
            `FAIL_UNLESS_EQUAL(ddio_in, to_fabric_buses[11:10])
            
            //Chip inputs still reach the fabric
            `FAIL_UNLESS_EQUAL(chip_inputs[1:0], to_fabric_buses[13:12])
            `FAIL_UNLESS_EQUAL(chip_inputs[9:8], to_fabric_buses[15:14])
        end
        
        //Releasing freeze reconnects the outputs and loopback paths
        ddio_in = '0;
        from_fabric_buses = '1;
        freeze_fabric = '0;
        
        #1ns;
        
        `FAIL_UNLESS_EQUAL(10'h3FF, chip_outputs)
        `FAIL_UNLESS_EQUAL(2'b11, ddio_out)
        `FAIL_UNLESS_EQUAL(2'b11, ddio_dir)
        `FAIL_UNLESS_EQUAL(2'b11, to_fabric_buses[1:0])
        `FAIL_UNLESS_EQUAL(2'b11, to_fabric_buses[5:4])
        `FAIL_UNLESS_EQUAL(2'b11, to_fabric_buses[9:8])
        `FAIL_UNLESS_EQUAL(2'b11, to_fabric_buses[11:10])
        
        #1ns;
    `SVTEST_END

    `SVUNIT_TESTS_END

endmodule