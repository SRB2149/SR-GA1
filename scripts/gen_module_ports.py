#!/usr/bin/env python3
"""
gen_module_ports.py

Parses a SystemVerilog module's parameter and port list and generates:
  - localparam placeholders for any parameters (so the module can be
    instantiated -- ports/arrays that depend on parameters need concrete
    values to elaborate)
  - matching `logic` signal declarations for every port (packed and
    unpacked dimensions preserved)
  - a fully-wired module instantiation using named port connections

This exists because create_unit_test.pl only emits an empty instantiation
(`my_module my_my_module();`) -- it does not parse ports at all.

Usage:
    python3 gen_module_ports.py path/to/module.sv
    python3 gen_module_ports.py path/to/module.sv --inst-name my_dut

Limitations (this is a pragmatic regex/stack parser, not a full SV parser):
  - Only handles ANSI-style port lists (`module foo (input logic a, ...)`)
  - Interface ports and `.name` implicit-conn ports are not specially handled
  - Default parameter values ARE preserved when present; parameters without
    defaults get a `/* TODO: set value */` placeholder of 1
"""

import re
import sys
import argparse


def strip_comments(text: str) -> str:
    text = re.sub(r'/\*.*?\*/', '', text, flags=re.DOTALL)
    text = re.sub(r'//.*', '', text)
    return text


def find_matching_paren(text: str, open_idx: int) -> int:
    """Given the index of an opening '(', return index of its matching ')'."""
    depth = 0
    for i in range(open_idx, len(text)):
        if text[i] == '(':
            depth += 1
        elif text[i] == ')':
            depth -= 1
            if depth == 0:
                return i
    raise ValueError("Unbalanced parentheses in module header")


def split_top_level(s: str, sep: str = ',') -> list:
    """Split on `sep` but only at paren/bracket depth 0."""
    parts = []
    depth = 0
    current = []
    for ch in s:
        if ch in '([{':
            depth += 1
        elif ch in ')]}':
            depth -= 1
        if ch == sep and depth == 0:
            parts.append(''.join(current))
            current = []
        else:
            current.append(ch)
    if current:
        parts.append(''.join(current))
    return [p.strip() for p in parts if p.strip()]


def parse_module_header(text: str):
    text = strip_comments(text)

    m = re.search(r'\bmodule\s+(\w+)', text)
    if not m:
        raise ValueError("No 'module <name>' found in file")
    mod_name = m.group(1)
    pos = m.end()

    # Optional parameter port list: #( ... )
    params = []
    rest = text[pos:]
    hash_match = re.match(r'\s*#\s*\(', rest)
    if hash_match:
        open_idx = pos + hash_match.end() - 1
        close_idx = find_matching_paren(text, open_idx)
        param_body = text[open_idx + 1:close_idx]
        for entry in split_top_level(param_body):
            # entry looks like "W" or "W = 8" or "parameter int W = 8"
            entry = re.sub(r'^\s*parameter\s+', '', entry)
            if '=' in entry:
                name_part, default = entry.split('=', 1)
                default = default.strip()
            else:
                name_part, default = entry, None
            # name is the last identifier-looking token
            tokens = name_part.strip().split()
            pname = tokens[-1] if tokens else name_part.strip()
            params.append((pname, default))
        pos = close_idx + 1

    # Port list: ( ... )
    rest = text[pos:]
    paren_match = re.match(r'\s*\(', rest)
    if not paren_match:
        raise ValueError("Could not find port list '(' after module/parameters")
    open_idx = pos + paren_match.end() - 1
    close_idx = find_matching_paren(text, open_idx)
    port_body = text[open_idx + 1:close_idx]

    ports = []
    last_direction = 'input'
    last_type = 'logic'
    for entry in split_top_level(port_body):
        entry = entry.strip()
        if not entry:
            continue

        dir_match = re.match(r'^(input|output|inout)\b', entry)
        if dir_match:
            direction = dir_match.group(1)
            entry = entry[dir_match.end():].strip()
        else:
            direction = last_direction

        type_match = re.match(r'^(logic|wire|reg|bit|byte|int|integer)\b', entry)
        if type_match:
            base_type = type_match.group(1)
            entry = entry[type_match.end():].strip()
        else:
            base_type = last_type

        # signed/unsigned qualifier (rare, kept if present)
        signed_match = re.match(r'^(signed|unsigned)\b', entry)
        signed = ''
        if signed_match:
            signed = signed_match.group(1) + ' '
            entry = entry[signed_match.end():].strip()

        # packed dimension(s), e.g. [W-1:0], possibly multiple
        packed_dims = ''
        while True:
            dim_match = re.match(r'^\[[^\]]*\]\s*', entry)
            if not dim_match:
                break
            packed_dims += dim_match.group(0).strip() + ' '
            entry = entry[dim_match.end():].strip()

        # remaining should be: name [unpacked dims] [= default]
        if '=' in entry:
            name_and_dims, _default = entry.split('=', 1)
        else:
            name_and_dims = entry

        name_match = re.match(r'^(\w+)\s*(.*)$', name_and_dims.strip())
        if not name_match:
            continue  # skip anything we can't confidently parse
        pname = name_match.group(1)
        unpacked_dims = name_match.group(2).strip()

        ports.append({
            'direction': direction,
            'type': base_type,
            'signed': signed,
            'packed': packed_dims.strip(),
            'name': pname,
            'unpacked': unpacked_dims,
        })

        last_direction = direction
        last_type = base_type

    return mod_name, params, ports


def generate(mod_name, params, ports, inst_name=None):
    if inst_name is None:
        inst_name = f"my_{mod_name}"

    lines = []

    if params:
        lines.append("  //===================================")
        lines.append("  // Parameters (fill in real values)")
        lines.append("  //===================================")
        for pname, default in params:
            if default is not None:
                lines.append(f"  localparam {pname} = {default};")
            else:
                lines.append(f"  localparam {pname} = 1;  /* TODO: set value */")
        lines.append("")

    if ports:
        lines.append("  //===================================")
        lines.append("  // Signals wired to the UUT ports")
        lines.append("  //===================================")
        for p in ports:
            packed = f"{p['packed']} " if p['packed'] else ""
            unpacked = f" {p['unpacked']}" if p['unpacked'] else ""
            lines.append(f"  logic {p['signed']}{packed}{p['name']}{unpacked};")
        lines.append("")

    lines.append("  //===================================")
    lines.append("  // This is the UUT that we're")
    lines.append("  // running the Unit Tests on")
    lines.append("  //===================================")

    if params:
        lines.append(f"  {mod_name} #(")
        for i, (pname, _default) in enumerate(params):
            comma = ',' if i < len(params) - 1 else ''
            lines.append(f"    .{pname}({pname}){comma}")
        inst_open = f"  ) {inst_name} ("
    else:
        inst_open = f"  {mod_name} {inst_name} ("

    lines.append(inst_open)
    for i, p in enumerate(ports):
        comma = ',' if i < len(ports) - 1 else ''
        lines.append(f"    .{p['name']}({p['name']}){comma}")
    lines.append("  );")

    return '\n'.join(lines)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('sv_file', help='Path to the .sv file containing the module')
    ap.add_argument('--inst-name', help='Instance name (default: my_<module>)')
    args = ap.parse_args()

    with open(args.sv_file, 'r') as f:
        text = f.read()

    mod_name, params, ports = parse_module_header(text)
    print(f"// Auto-generated for module '{mod_name}'", file=sys.stderr)
    print(generate(mod_name, params, ports, args.inst_name))


if __name__ == '__main__':
    main()