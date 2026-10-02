###############################################################################
# Created by write_sdc
###############################################################################
current_design CLB
###############################################################################
# Timing Constraints
###############################################################################
create_clock -name clk -period 100.0000 [get_ports {clk}]
set_clock_transition 0.1500 [get_clocks {clk}]
set_clock_uncertainty 0.2500 clk
set_propagated_clock [get_clocks {clk}]
set_input_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {carry_in}]
set_input_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {horz_bus_in[0]}]
set_input_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {horz_bus_in[1]}]
set_input_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {horz_bus_in[2]}]
set_input_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {horz_bus_in[3]}]
set_input_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {reset}]
set_input_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {shift_clk}]
set_input_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {shift_data_in}]
set_input_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {vert_bus_in[0]}]
set_input_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {vert_bus_in[1]}]
set_input_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {vert_bus_in[2]}]
set_input_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {vert_bus_in[3]}]
set_output_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {carry_out}]
set_output_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {clk_out}]
set_output_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {horz_bus_out[0]}]
set_output_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {horz_bus_out[1]}]
set_output_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {horz_bus_out[2]}]
set_output_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {horz_bus_out[3]}]
set_output_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {reset_out}]
set_output_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {shift_clk_out}]
set_output_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {shift_data_out}]
set_output_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {vert_bus_out[0]}]
set_output_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {vert_bus_out[1]}]
set_output_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {vert_bus_out[2]}]
set_output_delay 20.0000 -clock [get_clocks {clk}] -add_delay [get_ports {vert_bus_out[3]}]
###############################################################################
# Environment
###############################################################################
set_load -pin_load 0.0334 [get_ports {carry_out}]
set_load -pin_load 0.0334 [get_ports {clk_out}]
set_load -pin_load 0.0334 [get_ports {reset_out}]
set_load -pin_load 0.0334 [get_ports {shift_clk_out}]
set_load -pin_load 0.0334 [get_ports {shift_data_out}]
set_load -pin_load 0.0334 [get_ports {horz_bus_out[3]}]
set_load -pin_load 0.0334 [get_ports {horz_bus_out[2]}]
set_load -pin_load 0.0334 [get_ports {horz_bus_out[1]}]
set_load -pin_load 0.0334 [get_ports {horz_bus_out[0]}]
set_load -pin_load 0.0334 [get_ports {vert_bus_out[3]}]
set_load -pin_load 0.0334 [get_ports {vert_bus_out[2]}]
set_load -pin_load 0.0334 [get_ports {vert_bus_out[1]}]
set_load -pin_load 0.0334 [get_ports {vert_bus_out[0]}]
set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin {Y} -input_transition_rise 0.0000 -input_transition_fall 0.0000 [get_ports {carry_in}]
set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin {Y} -input_transition_rise 0.0000 -input_transition_fall 0.0000 [get_ports {clk}]
set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin {Y} -input_transition_rise 0.0000 -input_transition_fall 0.0000 [get_ports {reset}]
set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin {Y} -input_transition_rise 0.0000 -input_transition_fall 0.0000 [get_ports {shift_clk}]
set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin {Y} -input_transition_rise 0.0000 -input_transition_fall 0.0000 [get_ports {shift_data_in}]
set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin {Y} -input_transition_rise 0.0000 -input_transition_fall 0.0000 [get_ports {horz_bus_in[3]}]
set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin {Y} -input_transition_rise 0.0000 -input_transition_fall 0.0000 [get_ports {horz_bus_in[2]}]
set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin {Y} -input_transition_rise 0.0000 -input_transition_fall 0.0000 [get_ports {horz_bus_in[1]}]
set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin {Y} -input_transition_rise 0.0000 -input_transition_fall 0.0000 [get_ports {horz_bus_in[0]}]
set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin {Y} -input_transition_rise 0.0000 -input_transition_fall 0.0000 [get_ports {vert_bus_in[3]}]
set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin {Y} -input_transition_rise 0.0000 -input_transition_fall 0.0000 [get_ports {vert_bus_in[2]}]
set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin {Y} -input_transition_rise 0.0000 -input_transition_fall 0.0000 [get_ports {vert_bus_in[1]}]
set_driving_cell -lib_cell sky130_fd_sc_hd__inv_2 -pin {Y} -input_transition_rise 0.0000 -input_transition_fall 0.0000 [get_ports {vert_bus_in[0]}]
###############################################################################
# Design Rules
###############################################################################
set_max_transition 0.7500 [current_design]
set_max_capacitance 0.2000 [current_design]
set_max_fanout 10.0000 [current_design]
