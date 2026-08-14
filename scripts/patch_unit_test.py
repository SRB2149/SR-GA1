#!/usr/bin/env python3
"""
patch_unit_test.py

Patches a create_unit_test.pl-generated SVUnit testbench file, replacing the
bare, unwired UUT instantiation (e.g. `MUX my_MUX();`) with a fully wired
version generated from the DUT's actual port list. Relies on gen_module_ports.py
living in the same directory.

Usage:
    python3 patch_unit_test.py path/to/dut.sv path/to/dut_unit_test.sv
"""

import re
import sys
import os

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gen_module_ports import parse_module_header, generate

# Matches the placeholder block create_unit_test.pl always emits for modules:
#   //===================================
#   // This is the UUT that we're
#   // running the Unit Tests on
#   //===================================
#   <mod> my_<mod>();
UUT_BLOCK_RE = re.compile(
    r'  //={10,}\n'
    r"  // This is the UUT that we're ?\n"
    r'  // running the Unit Tests on\n'
    r'  //={10,}\n'
    r'  \S+\s+my_\S+\(\);\n'
)


def fix_include_path(ut_text, dut_path, ut_path):
    """
    create_unit_test.pl always emits `include "<basename>.sv"` -- a bare
    filename with no directory. That only resolves if the DUT source sits in
    the same directory as the generated testbench. Since testbenches get
    moved into a separate tb/ tree (mirroring hdl/'s layout, not sitting next
    to the DUT), rewrite it to a relative path from the testbench's actual
    location back to the real DUT file, so the include resolves regardless
    of what directory the simulator is invoked from.
    """
    dut_basename = os.path.basename(dut_path)
    ut_dir = os.path.dirname(os.path.abspath(ut_path))
    dut_abs = os.path.abspath(dut_path)
    rel_path = os.path.relpath(dut_abs, start=ut_dir).replace(os.sep, '/')

    pattern = re.compile(r'`include\s+"' + re.escape(dut_basename) + r'"')
    new_text, n = pattern.subn(lambda m: f'`include "{rel_path}"', ut_text, count=1)
    return new_text, n


def main():
    if len(sys.argv) != 3:
        print("Usage: patch_unit_test.py <dut.sv> <unit_test.sv>", file=sys.stderr)
        sys.exit(1)

    dut_path, ut_path = sys.argv[1], sys.argv[2]

    with open(dut_path) as f:
        dut_text = f.read()
    mod_name, params, ports = parse_module_header(dut_text)

    replacement = generate(mod_name, params, ports) + "\n\n\n"

    with open(ut_path) as f:
        ut_text = f.read()

    # Use a function replacement (not a plain string) so any backslashes or
    # regex-special sequences in the generated SV code aren't misread as
    # backreferences by re.sub.
    new_text, n = UUT_BLOCK_RE.subn(lambda m: replacement, ut_text, count=1)

    if n == 0:
        print(f"Warning: could not find the placeholder UUT block in {ut_path}; "
              f"file left unchanged.", file=sys.stderr)
        sys.exit(1)

    new_text, inc_n = fix_include_path(new_text, dut_path, ut_path)
    if inc_n == 0:
        print(f"Warning: could not find the `include for '{os.path.basename(dut_path)}' "
              f"in {ut_path}; include path left unchanged (simulation may fail to find the DUT).",
              file=sys.stderr)

    with open(ut_path, 'w') as f:
        f.write(new_text)

    print(f"Patched {ut_path} with wired-up port connections for '{mod_name}'", file=sys.stderr)


if __name__ == '__main__':
    main()