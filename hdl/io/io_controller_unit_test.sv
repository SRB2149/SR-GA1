`include "svunit_defines.svh"
`include "io_controller.sv"

module IO_Controller_unit_test;
    import svunit_pkg::svunit_testcase;

    string name = "IO_Controller_ut";
    svunit_testcase svunit_ut;


    //===================================
    // Signals wired to the UUT ports
    //===================================
    logic [9:0] chip_inputs;
    logic [9:0] chip_outputs;
    logic [1:0] ddio_in;
    logic [1:0] ddio_dir;
    logic [1:0] ddio_out;
    logic [3:0] from_fabric_buses [4];
    logic [3:0] to_fabric_buses [4];
    
    logic [9:0] out_temp;
    logic [1:0] dirs;
    logic [3:0] data_temp;

    //===================================
    // This is the UUT that we're
    // running the Unit Tests on
    //===================================
    IO_Controller my_IO_Controller (
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
        from_fabric_buses[0] = '0;
        from_fabric_buses[1] = '0;
        from_fabric_buses[2] = '0;
        from_fabric_buses[3] = '0;
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

    `SVTEST(test_inputs)
        for (int i=0; i<2**10; i++)
        begin
            chip_inputs = i;
            
            #1ns;
            
            `FAIL_UNLESS_EQUAL(chip_inputs[3:0], to_fabric_buses[0])
            `FAIL_UNLESS_EQUAL(chip_inputs[7:4], to_fabric_buses[1])
            `FAIL_UNLESS_EQUAL(chip_inputs[9:8], to_fabric_buses[2][1:0])
            `FAIL_UNLESS_EQUAL(4'b1010, to_fabric_buses[3])
        end
        
        #1ns;
    `SVTEST_END
    
    `SVTEST(test_outputs)
        for (int i=0; i<2**10; i++)
        begin
            out_temp = 10'(i);
            from_fabric_buses[0] = out_temp[3:0];
            from_fabric_buses[1] = out_temp[7:4];
            from_fabric_buses[2][1:0] = out_temp[9:8];
            
            #1ns;
            
            `FAIL_UNLESS_EQUAL(chip_outputs, out_temp)
        end
        
        #1ns;
    `SVTEST_END
    
    `SVTEST(test_dual_dir)
        for (int i=0; i<4; i++)
        begin
            dirs = 2'(i);
            from_fabric_buses[3][1:0] = dirs;
            
            for (int data=0; data<16; data++)
            begin
                data_temp = 4'(data);
                ddio_in = data_temp[1:0];
                from_fabric_buses[2][3:2] = data_temp[3:2];
                
                #1ns;
                
                `FAIL_UNLESS_EQUAL(dirs[0] ? '0 : ddio_in[0], to_fabric_buses[2][2])
                `FAIL_UNLESS_EQUAL(dirs[1] ? '0 : ddio_in[1], to_fabric_buses[2][3])
                
                `FAIL_UNLESS_EQUAL(from_fabric_buses[2][2], ddio_out[0])
                `FAIL_UNLESS_EQUAL(from_fabric_buses[2][3], ddio_out[1])
            end
        end
        
        #1ns;
    `SVTEST_END

    `SVUNIT_TESTS_END

endmodule
