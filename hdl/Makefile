# Makefile for generating and simulating SVUnit testbenches
#
# Directory layout this expects:
#   parent/
#     hdl/       <- this Makefile lives here; run `make` targets from here
#     scripts/   <- gen_module_ports.py, patch_unit_test.py, fix_svunit_paths.py
#     tb/        <- build/sim OUTPUT only (logs, work library, waveforms) --
#                    mirrors hdl/'s subdirs, but does NOT contain the .sv
#                    testbench files themselves (see below)
#
# The generated <name>_unit_test.sv file itself lives right next to the DUT
# it tests, in hdl/, rather than in tb/ -- easier to find, and keeps the
# test alongside the module when browsing/editing. Everything build- and
# simulation-related (compiled work library, logs, waveforms, the SVUnit
# filelist) still lands under tb/, mirroring hdl/'s directory structure.
# This works because buildSVUnit's own -o flag lets us search for test files
# from one directory (hdl/) while writing all output to a different one
# (tb/) -- see the comments on `sim`/`sim_all` below.
#
# Usage:
#   make new_tb DUT=path/to/my_module.sv
#   make sim DUT=path/to/my_module.sv [WAVES=1]
#   make sim_all [WAVES=1]
#   make wave DUT=path/to/my_module.sv        (ModelSim GUI)
#   make wave_all
#   make wave_vcd DUT=path/to/my_module.sv    (Surfer, lighter/faster)
#   make wave_vcd_all
#
# Example:
#   make new_tb DUT=rtl/blocks/counter.sv
#   -> generates rtl/blocks/counter_unit_test.sv (next to counter.sv itself)
#   -> `make sim DUT=rtl/blocks/counter.sv` then builds/runs it, with all
#      output under ../tb/rtl/blocks/
#
# SVUnit location: rather than hardcoding a path (which would leak your local
# filesystem layout if this Makefile is committed to a public repo), this
# expects SVUNIT_INSTALL to be exported in your shell -- e.g. add to your
# ~/.bashrc (NOT to this file):
#   export SVUNIT_INSTALL=/path/to/svunit
#
# With that set, targets do the equivalent of what you'd do by hand: cd into
# SVUNIT_INSTALL, source its Setup.bsh (which sets up PATH), cd back, then
# run SVUnit's scripts by name (relying on PATH, not a hardcoded script
# path). This all happens in a single `bash -c` invocation because Make runs
# every recipe line in its own separate shell -- sourcing Setup.bsh on one
# line would have no effect on the next line's shell.

# Simulator to use for `make sim` / `make sim_all` (matches runSVUnit's -s flag:
# questa, modelsim, riviera, ius, xcelium, vcs, dsim, verilator, xsim)
SIM ?= modelsim

# Pass WAVES=1 to `make sim`/`make sim_all` to record a waveform (run.wlf) you
# can open afterward with `make wave`/`make wave_all`. Off by default since
# logging every signal slows compilation/sim down a bit and the .wlf can get
# large on bigger designs.
WAVES ?= 0

# Surfer executable, used by `make wave_vcd`/`make wave_vcd_all` -- a much
# lighter/faster alternative to launching ModelSim's own GUI just to look at
# a waveform. Download: https://surfer-project.org
SURFER ?= surfer

# Project-wide filelist (e.g. shared packages/interfaces some DUTs depend on
# but don't `include directly) -- compiled alongside every testbench in
# `sim`/`sim_all`. Defaults to sr-ga1.f sitting next to this Makefile;
# override with `make sim ... PROJECT_FILELIST=/path/to/other.f` if needed.
# If the file doesn't exist, sim/sim_all just skip it with a warning rather
# than failing, since not every checkout may have it.
PROJECT_FILELIST ?= $(MAKEFILE_DIR)all_files.f

.PHONY: new_tb sim sim_all wave wave_all wave_vcd wave_vcd_all

# Directory this Makefile lives in (works regardless of cwd when `make` is run)
MAKEFILE_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

# Top of the generated build/sim OUTPUT tree (parallel to hdl/), used by sim_all.
# Absolute forms are used when passing to buildSVUnit's -o flag, since we cd
# into hdl/ (a different directory) before invoking it.
TB_ROOT     := $(MAKEFILE_DIR)../tb
TB_ROOT_ABS := $(abspath $(TB_ROOT))

# Python interpreter (override with `make new_tb PYTHON=...` if `python3`
# isn't on your PATH -- some Windows Python installs only provide `python`)
PYTHON ?= python3

# Port-wiring helper script, resolved relative to this Makefile (../scripts/)
PATCH_SCRIPT := $(MAKEFILE_DIR)../scripts/patch_unit_test.py

# Path-fixing helper script for `sim`/`sim_all` (see comment on that target)
FIX_PATHS_SCRIPT := $(MAKEFILE_DIR)../scripts/fix_svunit_paths.py

# Resolves relative paths in PROJECT_FILELIST to absolute ones (see comment
# on `sim` below for why this is necessary)
RESOLVE_FILELIST_SCRIPT := $(MAKEFILE_DIR)../scripts/resolve_filelist_paths.py

# Directory component of the DUT path (includes trailing slash, "./" if none)
DUT_DIR      := $(dir $(DUT))
# DUT filename without directory
DUT_FILE     := $(notdir $(DUT))
# DUT filename without extension
DUT_BASENAME := $(basename $(DUT_FILE))
# Name of the generated testbench, living alongside the DUT in hdl/
UT_FILE      := $(DUT_BASENAME)_unit_test.sv
# Build/sim OUTPUT directory for this DUT, mirroring its directory under ../tb/
OUT_DIR      := ../tb/$(DUT_DIR)
OUT_DIR_ABS  := $(abspath $(OUT_DIR))

##########################################################################
# new_tb: generate a fully wired testbench for a module
#
#   make new_tb DUT=path/to/my_module.sv
#
# Writes <name>_unit_test.sv directly alongside the DUT (same directory),
# then auto-wires the UUT's ports/parameters and fixes the `include path
# using the scripts in ../scripts/.
##########################################################################
new_tb:
ifndef DUT
	$(error DUT is not set. Usage: make new_tb DUT=path/to/module.sv)
endif
ifeq ("$(wildcard $(DUT))","")
	$(error DUT file '$(DUT)' does not exist)
endif
ifndef SVUNIT_INSTALL
	$(error SVUNIT_INSTALL is not set. Export it in your shell profile (e.g. ~/.bashrc), not in this Makefile: export SVUNIT_INSTALL=/path/to/svunit)
endif
ifneq ("$(wildcard $(DUT_DIR)$(UT_FILE))","")
ifneq ($(FORCE),1)
	$(error $(DUT_DIR)$(UT_FILE) already exists -- refusing to overwrite it (you may have hand-edited tests in there). Pass FORCE=1 to overwrite anyway: make new_tb DUT=$(DUT) FORCE=1)
endif
endif
	@bash -c '\
		cd "$(SVUNIT_INSTALL)" && source Setup.bsh && cd - >/dev/null && \
		create_unit_test.pl -overwrite -out "$(DUT_DIR)$(UT_FILE)" $(DUT) \
	'
	@if [ -f "$(DUT_DIR)$(UT_FILE)" ]; then \
		echo "Created $(DUT_DIR)$(UT_FILE)"; \
		$(PYTHON) "$(PATCH_SCRIPT)" "$(DUT)" "$(DUT_DIR)$(UT_FILE)"; \
	else \
		echo "Error: expected output '$(DUT_DIR)$(UT_FILE)' was not created by create_unit_test.pl"; \
		exit 1; \
	fi

##########################################################################
# sim: simulate a single module's testbench
#
#   make sim DUT=path/to/my_module.sv
#
# Searches for test files starting from the DUT's own directory in hdl/
# (via buildSVUnit's default cwd-based discovery), but writes all build/sim
# output to the mirrored location under ../tb/ (via buildSVUnit's -o flag --
# search location and output location are independent). Requires the
# testbench to already exist -- run `make new_tb` first.
#
# For modelsim/questa specifically: rather than letting runSVUnit do
# "build test suite" and "compile+simulate" as one un-interruptible step,
# this splits them so the generated .svunit.f filelist can be patched in
# between. That's needed because SVUnit's own helper scripts only run
# correctly under Git Bash's bundled Perl (native Windows Perl can't invoke
# their extension-less shebang scripts like `buildSVUnit`) -- but that same
# Perl's Cwd::getcwd() always returns POSIX-style paths (/c/...), which
# ModelSim's native vlog.exe can't read. fix_svunit_paths.py converts them
# to Windows form (C:/...) after the filelist is built, before compiling.
#
# Pass WAVES=1 to log every signal to run.wlf, then view it with `make wave`.
##########################################################################
sim:
ifndef DUT
	$(error DUT is not set. Usage: make sim DUT=path/to/module.sv)
endif
ifndef SVUNIT_INSTALL
	$(error SVUNIT_INSTALL is not set. Export it in your shell profile (e.g. ~/.bashrc): export SVUNIT_INSTALL=/path/to/svunit)
endif
ifeq ("$(wildcard $(DUT_DIR)$(UT_FILE))","")
	$(error No testbench found at $(DUT_DIR)$(UT_FILE) -- run 'make new_tb DUT=$(DUT)' first)
endif
	@rm -rf "$(OUT_DIR)"
	@mkdir -p "$(OUT_DIR)"
	@bash -c '\
		cd "$(SVUNIT_INSTALL)" && source Setup.bsh && cd - >/dev/null && \
		cd "$(DUT_DIR)" && \
		if [ -f "$(PROJECT_FILELIST)" ]; then \
			$(PYTHON) "$(RESOLVE_FILELIST_SCRIPT)" "$(PROJECT_FILELIST)" "$(OUT_DIR_ABS)/project.resolved.f"; \
			EXTRA_F="-f $(OUT_DIR_ABS)/project.resolved.f"; \
		else \
			EXTRA_F=""; \
			echo "Note: PROJECT_FILELIST ($(PROJECT_FILELIST)) not found, skipping it"; \
		fi; \
		if [ "$(SIM)" = "modelsim" ] || [ "$(SIM)" = "questa" ]; then \
			buildSVUnit -o "$(OUT_DIR_ABS)" && \
			$(PYTHON) "$(FIX_PATHS_SCRIPT)" "$(OUT_DIR_ABS)/.svunit.f" && \
			cd "$(OUT_DIR_ABS)" && \
			vlib work && vlog -l compile.log -f .svunit.f $$EXTRA_F && \
			if [ "$(WAVES)" = "1" ]; then \
				vsim -c -lib work -wlf run.wlf -do "log -r /*; run -all; quit" -l run.log testrunner; \
			else \
				vsim -c -lib work -do "run -all; quit" -l run.log testrunner; \
			fi; \
		else \
			runSVUnit -s $(SIM) -o "$(OUT_DIR_ABS)" $$EXTRA_F; \
		fi \
	'

##########################################################################
# sim_all: build a fresh test suite covering every module under hdl/ and
#          simulate all of them together
#
#   make sim_all
#
# Searches all of hdl/ recursively for *_unit_test.sv files, and writes all
# build/sim output under ../tb/. See the comment on `sim` above for why
# modelsim/questa get a separate build+fix-paths+simulate flow rather than a
# single runSVUnit call, and for what WAVES=1 does.
##########################################################################
sim_all:
ifndef SVUNIT_INSTALL
	$(error SVUNIT_INSTALL is not set. Export it in your shell profile (e.g. ~/.bashrc): export SVUNIT_INSTALL=/path/to/svunit)
endif
	@rm -rf "$(TB_ROOT)"
	@mkdir -p "$(TB_ROOT)"
	@bash -c '\
		cd "$(SVUNIT_INSTALL)" && source Setup.bsh && cd - >/dev/null && \
		cd "$(MAKEFILE_DIR)" && \
		if [ -f "$(PROJECT_FILELIST)" ]; then \
			$(PYTHON) "$(RESOLVE_FILELIST_SCRIPT)" "$(PROJECT_FILELIST)" "$(TB_ROOT_ABS)/project.resolved.f"; \
			EXTRA_F="-f $(TB_ROOT_ABS)/project.resolved.f"; \
		else \
			EXTRA_F=""; \
			echo "Note: PROJECT_FILELIST ($(PROJECT_FILELIST)) not found, skipping it"; \
		fi; \
		if [ "$(SIM)" = "modelsim" ] || [ "$(SIM)" = "questa" ]; then \
			buildSVUnit -o "$(TB_ROOT_ABS)" && \
			$(PYTHON) "$(FIX_PATHS_SCRIPT)" "$(TB_ROOT_ABS)/.svunit.f" && \
			cd "$(TB_ROOT_ABS)" && \
			vlib work && vlog -l compile.log -f .svunit.f $$EXTRA_F && \
			if [ "$(WAVES)" = "1" ]; then \
				vsim -c -lib work -wlf run.wlf -do "log -r /*; run -all; quit" -l run.log testrunner; \
			else \
				vsim -c -lib work -do "run -all; quit" -l run.log testrunner; \
			fi; \
		else \
			runSVUnit -s $(SIM) -o "$(TB_ROOT_ABS)" $$EXTRA_F; \
		fi \
	'

##########################################################################
# wave / wave_all: open the waveform recorded by `make sim WAVES=1` /
#                   `make sim_all WAVES=1` in ModelSim's GUI viewer
#
#   make wave DUT=path/to/my_module.sv
#   make wave_all
#
# Opens run.wlf in a new (non-batch) vsim session and auto-adds every logged
# signal to the Wave pane, so you don't have to manually click through the
# signal tree each time.
##########################################################################
wave:
ifndef DUT
	$(error DUT is not set. Usage: make wave DUT=path/to/module.sv)
endif
ifndef SVUNIT_INSTALL
	$(error SVUNIT_INSTALL is not set. Export it in your shell profile (e.g. ~/.bashrc): export SVUNIT_INSTALL=/path/to/svunit)
endif
ifeq ("$(wildcard $(OUT_DIR)run.wlf)","")
	$(error No waveform found at $(OUT_DIR)run.wlf -- run 'make sim DUT=$(DUT) WAVES=1' first)
endif
	@bash -c '\
		cd "$(SVUNIT_INSTALL)" && source Setup.bsh && cd - >/dev/null && \
		cd "$(OUT_DIR)" && vsim -view run.wlf -do "add wave -r /*" \
	'

wave_all:
ifndef SVUNIT_INSTALL
	$(error SVUNIT_INSTALL is not set. Export it in your shell profile (e.g. ~/.bashrc): export SVUNIT_INSTALL=/path/to/svunit)
endif
ifeq ("$(wildcard $(TB_ROOT)/run.wlf)","")
	$(error No waveform found at $(TB_ROOT)/run.wlf -- run 'make sim_all WAVES=1' first)
endif
	@bash -c '\
		cd "$(SVUNIT_INSTALL)" && source Setup.bsh && cd - >/dev/null && \
		cd "$(TB_ROOT)" && vsim -view run.wlf -do "add wave -r /*" \
	'

##########################################################################
# wave_vcd / wave_vcd_all: convert the recorded waveform to VCD and open it
#                           in Surfer instead of ModelSim's (slow) GUI
#
#   make wave_vcd DUT=path/to/my_module.sv
#   make wave_vcd_all
#
# Uses ModelSim's own wlf2vcd command-line converter (fast, no GUI launch)
# to produce run.vcd, then opens it with Surfer. Install Surfer from
# https://surfer-project.org -- or override SURFER= if it's not on PATH
# under the name `surfer`.
##########################################################################
wave_vcd:
ifndef DUT
	$(error DUT is not set. Usage: make wave_vcd DUT=path/to/module.sv)
endif
ifeq ("$(wildcard $(OUT_DIR)run.wlf)","")
	$(error No waveform found at $(OUT_DIR)run.wlf -- run 'make sim DUT=$(DUT) WAVES=1' first)
endif
	@bash -c '\
		cd "$(OUT_DIR)" && \
		wlf2vcd -o run.vcd run.wlf && \
		$(SURFER) run.vcd \
	'

wave_vcd_all:
ifeq ("$(wildcard $(TB_ROOT)/run.wlf)","")
	$(error No waveform found at $(TB_ROOT)/run.wlf -- run 'make sim_all WAVES=1' first)
endif
	@bash -c '\
		cd "$(TB_ROOT)" && \
		wlf2vcd -o run.vcd run.wlf && \
		$(SURFER) run.vcd \
	'