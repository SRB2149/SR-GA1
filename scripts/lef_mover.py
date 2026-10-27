#!/usr/bin/env python3
"""
lef_mover.py

Resizes a LEF macro's bounding box and/or translates everything inside it.

  -r W*H      set the macro SIZE to W BY H (also accepts WxH or W,H)
  -x DX       shift all geometry (pins and obstructions) by DX in x
  -y DY       shift all geometry by DY in y

After applying the changes, the geometry extents are checked against the
bounding box. If anything sticks out past the top or right edge, a warning is
printed and the SIZE is automatically enlarged so the box is big enough. If
anything ends up below/left of the box origin, a warning is printed (resizing
cannot fix that -- translate it back with -x/-y).

Geometry handled: RECT, POLYGON, PATH and VIA statements (including MASK and
ITERATE forms) inside PORT and OBS blocks. Numbers keep their original layout
and decimal precision.

Usage:
    python3 lef_mover.py in.lef -r 40*50 -o out.lef
    python3 lef_mover.py in.lef -x 2.5 -y -1
    python3 lef_mover.py in.lef -x 3 --macro CLB          (only touch CLB)
    python3 lef_mover.py in.lef                           (just check fit)

The input file is edited in place unless -o is given. Warnings go to stderr.
"""

import re
import sys
import argparse
from decimal import Decimal, InvalidOperation

NUM_RE = re.compile(r'^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$')
GEOM_KEYWORDS = ('RECT', 'POLYGON', 'PATH', 'VIA')


def is_num(tok: str) -> bool:
    return bool(NUM_RE.match(tok))


def decimals_of(tok: str) -> int:
    return len(tok.split('.', 1)[1]) if '.' in tok else 0


def fmt(value: Decimal, places: int) -> str:
    if places == 0:
        return str(value.quantize(Decimal(1)))
    return str(value.quantize(Decimal(1).scaleb(-places)))


def parse_size(text: str):
    parts = re.split(r'\s*[xX*,]\s*', text.strip())
    if len(parts) != 2:
        raise argparse.ArgumentTypeError(f"bad size '{text}', expected W*H (e.g. 40*50)")
    try:
        w, h = Decimal(parts[0]), Decimal(parts[1])
    except InvalidOperation:
        raise argparse.ArgumentTypeError(f"bad size '{text}', expected W*H (e.g. 40*50)")
    if w <= 0 or h <= 0:
        raise argparse.ArgumentTypeError(f"size must be positive, got '{text}'")
    return w, h


def parse_decimal(text: str) -> Decimal:
    try:
        return Decimal(text)
    except InvalidOperation:
        raise argparse.ArgumentTypeError(f"not a number: '{text}'")


class BBox:
    def __init__(self):
        self.xmin = self.ymin = self.xmax = self.ymax = None

    def add(self, x0, y0, x1, y1):
        x0, x1 = min(x0, x1), max(x0, x1)
        y0, y1 = min(y0, y1), max(y0, y1)
        if self.xmin is None:
            self.xmin, self.ymin, self.xmax, self.ymax = x0, y0, x1, y1
        else:
            self.xmin = min(self.xmin, x0)
            self.ymin = min(self.ymin, y0)
            self.xmax = max(self.xmax, x1)
            self.ymax = max(self.ymax, y1)

    def empty(self):
        return self.xmin is None


def transform_geom(stmt: str, dx: Decimal, dy: Decimal, path_width: Decimal, bbox: BBox) -> str:
    """Shift the coordinates of one RECT/POLYGON/PATH/VIA statement (text up to
    and including ';', possibly spanning lines) and record its extents."""
    toks = list(re.finditer(r'[^\s;]+|;', stmt))
    kind = toks[0].group().upper()
    i = 1

    if i < len(toks) and toks[i].group().upper() == 'MASK':
        i += 2
    iterate = False
    if i < len(toks) and toks[i].group().upper() == 'ITERATE':
        iterate = True
        i += 1

    max_coords = {'RECT': 4, 'VIA': 2}.get(kind)
    coord_idx = []
    while i < len(toks) and is_num(toks[i].group()):
        if max_coords is not None and len(coord_idx) == max_coords:
            break
        coord_idx.append(i)
        i += 1
    if len(coord_idx) % 2:
        raise ValueError(f"odd number of coordinates in: {stmt.strip()}")

    rep = None
    if iterate:
        rest = [t.group().upper() for t in toks[i:]]
        if 'DO' in rest and 'STEP' in rest:
            d = rest.index('DO')
            s = rest.index('STEP')
            nx, ny = int(rest[d + 1]), int(rest[d + 3])
            sx, sy = Decimal(rest[s + 1]), Decimal(rest[s + 2])
            rep = (nx, ny, sx, sy)

    new_vals = {}
    pts = []
    for n in range(0, len(coord_idx), 2):
        xi, yi = coord_idx[n], coord_idx[n + 1]
        xt, yt = toks[xi].group(), toks[yi].group()
        places_x = max(decimals_of(xt), -dx.as_tuple().exponent if dx else 0)
        places_y = max(decimals_of(yt), -dy.as_tuple().exponent if dy else 0)
        x, y = Decimal(xt) + dx, Decimal(yt) + dy
        new_vals[xi] = fmt(x, places_x)
        new_vals[yi] = fmt(y, places_y)
        pts.append((x, y))

    if pts:
        xs = [p[0] for p in pts]
        ys = [p[1] for p in pts]
        x0, y0, x1, y1 = min(xs), min(ys), max(xs), max(ys)
        if kind == 'PATH':
            half = path_width / 2
            x0, y0, x1, y1 = x0 - half, y0 - half, x1 + half, y1 + half
        if rep:
            nx, ny, sx, sy = rep
            ex, ey = (nx - 1) * sx, (ny - 1) * sy
            x0, x1 = min(x0, x0 + ex), max(x1, x1 + ex)
            y0, y1 = min(y0, y0 + ey), max(y1, y1 + ey)
        bbox.add(x0, y0, x1, y1)

    if not dx and not dy:
        return stmt
    out = []
    last = 0
    for idx, t in enumerate(toks):
        if idx in new_vals:
            out.append(stmt[last:t.start()])
            out.append(new_vals[idx])
            last = t.end()
    out.append(stmt[last:])
    return ''.join(out)


def warn(msg: str):
    print(f"WARNING: {msg}", file=sys.stderr)


def process(lines, args):
    out = []
    macro = None
    active = False
    size_line = None
    size_tokens = None
    origin = (Decimal(0), Decimal(0))
    bbox = BBox()
    path_width = Decimal(0)
    seen_macros = []

    i = 0
    while i < len(lines):
        line = lines[i]
        toks = line.split()
        head = toks[0].upper() if toks else ''

        if macro is None:
            if head == 'MACRO' and len(toks) > 1:
                macro = toks[1]
                seen_macros.append(macro)
                active = args.macro is None or macro in args.macro
                size_line = None
                size_tokens = None
                origin = (Decimal(0), Decimal(0))
                bbox = BBox()
                path_width = Decimal(0)
            out.append(line)
            i += 1
            continue

        if head == 'END' and len(toks) > 1 and toks[1] == macro:
            if active:
                finish_macro(out, macro, size_line, size_tokens, origin, bbox, args)
            macro = None
            out.append(line)
            i += 1
            continue

        if not active:
            out.append(line)
            i += 1
            continue

        if head == 'SIZE':
            size_line = len(out)
            size_tokens = toks
            out.append(line)
        elif head == 'ORIGIN' and len(toks) >= 3:
            origin = (Decimal(toks[1]), Decimal(toks[2]))
            out.append(line)
        elif head == 'LAYER':
            path_width = Decimal(0)
            out.append(line)
        elif head == 'WIDTH' and len(toks) >= 2 and is_num(toks[1]):
            path_width = Decimal(toks[1])
            out.append(line)
        elif head in GEOM_KEYWORDS:
            stmt = line
            while ';' not in stmt and i + 1 < len(lines):
                i += 1
                stmt += lines[i]
            out.append(transform_geom(stmt, args.x, args.y, path_width, bbox))
        else:
            out.append(line)
        i += 1

    if macro is not None:
        raise ValueError(f"MACRO {macro} has no matching END")
    if args.macro:
        for m in args.macro:
            if m not in seen_macros:
                warn(f"macro '{m}' not found in file")
    if not seen_macros:
        warn("no MACRO found in file")
    return out


def finish_macro(out, macro, size_line, size_tokens, origin, bbox, args):
    if size_line is None:
        raise ValueError(f"MACRO {macro} has no SIZE statement")

    old_w, old_h = Decimal(size_tokens[1]), Decimal(size_tokens[3])
    places_w, places_h = decimals_of(size_tokens[1]), decimals_of(size_tokens[3])
    w, h = old_w, old_h
    if args.resize:
        w, h = args.resize
        places_w = max(places_w, -w.as_tuple().exponent)
        places_h = max(places_h, -h.as_tuple().exponent)

    box_x0, box_y0 = -origin[0], -origin[1]

    if not bbox.empty():
        need_w = bbox.xmax - box_x0
        need_h = bbox.ymax - box_y0
        if need_w > w or need_h > h:
            new_w, new_h = max(w, need_w), max(h, need_h)
            old_size = f"{fmt(w, places_w)} BY {fmt(h, places_h)}"
            places_w = max(places_w, -new_w.as_tuple().exponent)
            places_h = max(places_h, -new_h.as_tuple().exponent)
            warn(f"{macro}: contents extend to ({bbox.xmax}, {bbox.ymax}) which does not fit "
                 f"in {old_size}; automatically resized to "
                 f"{fmt(new_w, places_w)} BY {fmt(new_h, places_h)} so the box is big enough")
            w, h = new_w, new_h
        if bbox.xmin < box_x0 or bbox.ymin < box_y0:
            warn(f"{macro}: contents extend to ({bbox.xmin}, {bbox.ymin}), below/left of the box "
                 f"origin ({box_x0}, {box_y0}); resizing cannot fix this, shift it back with -x/-y")

    if (w, h) != (old_w, old_h):
        line = out[size_line]
        indent = line[:len(line) - len(line.lstrip())]
        nl = '\n' if line.endswith('\n') else ''
        out[size_line] = f"{indent}SIZE {fmt(w, places_w)} BY {fmt(h, places_h)} ;{nl}"

    if not args.quiet:
        ext = "empty" if bbox.empty() else f"({bbox.xmin}, {bbox.ymin}) - ({bbox.xmax}, {bbox.ymax})"
        print(f"{macro}: SIZE {fmt(old_w, places_w)} BY {fmt(old_h, places_h)} -> "
              f"{fmt(w, places_w)} BY {fmt(h, places_h)}, shift ({args.x}, {args.y}), "
              f"contents {ext}", file=sys.stderr)


def main():
    ap = argparse.ArgumentParser(description="Resize and/or translate the contents of LEF macros.")
    ap.add_argument('lef', help="input LEF file")
    ap.add_argument('-r', '--resize', type=parse_size, metavar='W*H',
                    help="new macro size, e.g. 40*50 (also WxH or W,H)")
    ap.add_argument('-x', type=parse_decimal, default=Decimal(0), metavar='DX',
                    help="translate contents in x by DX microns")
    ap.add_argument('-y', type=parse_decimal, default=Decimal(0), metavar='DY',
                    help="translate contents in y by DY microns")
    ap.add_argument('-m', '--macro', action='append',
                    help="only modify this macro (repeatable); default is all macros")
    ap.add_argument('-o', '--output', help="output LEF file (default: overwrite the input file)")
    ap.add_argument('-q', '--quiet', action='store_true', help="suppress the per-macro summary")
    args = ap.parse_args()

    with open(args.lef, newline='') as f:
        lines = f.readlines()

    try:
        result = process(lines, args)
    except (ValueError, InvalidOperation, IndexError) as e:
        print(f"ERROR: {e}", file=sys.stderr)
        sys.exit(1)

    with open(args.output or args.lef, 'w', newline='') as f:
        f.write(''.join(result))


if __name__ == '__main__':
    main()
