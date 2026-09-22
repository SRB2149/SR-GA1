#!/usr/bin/env python3
"""
png_to_gds.py - convert small black-and-white PNGs into GDS chip art.

Every pixel at or above the threshold becomes a metal rectangle. Runs of
horizontally adjacent pixels are merged into single rectangles, which keeps
the polygon count down and avoids thousands of abutting squares.

Examples
--------
    # 1 um per pixel on met1
    python3 png_to_gds.py logo.png -o logo.gds --pixel-size 1.0 --layer met1

    # several images into one GDS, each its own cell
    python3 png_to_gds.py a.png b.png -o art.gds --pixel-size 0.5

    # invert (dark pixels become metal) and preview without writing
    python3 png_to_gds.py logo.png -o logo.gds --invert --dry-run
"""

import argparse
import math
import sys
from pathlib import Path

try:
    import gdstk
except ImportError:
    sys.exit("Missing dependency: pip install gdstk")

try:
    from PIL import Image
except ImportError:
    sys.exit("Missing dependency: pip install pillow")


# sky130 drawing layers, as (layer, datatype).
SKY130_LAYERS = {
    "li1":  (67, 20),
    "met1": (68, 20),
    "met2": (69, 20),
    "met3": (70, 20),
    "met4": (71, 20),
    "met5": (72, 20),
}

# Minimum drawn width per layer in um, from the sky130 DRC rules. Used to warn
# when the requested pixel size would produce shapes that cannot pass DRC.
SKY130_MIN_WIDTH = {
    "li1":  0.17,
    "met1": 0.14,
    "met2": 0.14,
    "met3": 0.30,
    "met4": 0.30,
    "met5": 1.60,
}

# Minimum area per shape in um^2 (sky130 rules li1.5a, met1.6, met2.6, met3.6,
# met4.4a, met5.4). A shape smaller than this fails DRC even if its width is
# legal, which is what catches isolated single-pixel squares.
SKY130_MIN_AREA = {
    "li1":  0.0561,
    "met1": 0.083,
    "met2": 0.0676,
    "met3": 0.240,
    "met4": 0.240,
    "met5": 4.000,
}

# Minimum spacing between shapes in um (li1.3, met1.2, met2.2, met3.2, met4.2,
# met5.2). A one-pixel gap in the artwork must be at least this wide.
SKY130_MIN_SPACING = {
    "li1":  0.17,
    "met1": 0.14,
    "met2": 0.14,
    "met3": 0.30,
    "met4": 0.30,
    "met5": 1.60,
}


# Place-and-route boundary layer. A macro GDS needs a rectangle on this layer
# covering its footprint, or the flow cannot work out the macro's extent.
SKY130_PRBOUNDARY = (235, 4)


def parse_layer(spec):
    """Accept either a name like 'met1' or a raw 'layer/datatype' pair."""
    if spec in SKY130_LAYERS:
        return SKY130_LAYERS[spec], spec
    if "/" in spec:
        layer, datatype = spec.split("/", 1)
        return (int(layer), int(datatype)), None
    try:
        return (int(spec), 0), None
    except ValueError:
        raise argparse.ArgumentTypeError(
            f"Unrecognized layer '{spec}'. Use a name ({', '.join(SKY130_LAYERS)}), "
            "a number, or 'layer/datatype'."
        )


def load_mask(path, threshold, invert):
    """Return a 2D list of booleans: True where metal should be placed."""
    img = Image.open(path).convert("L")
    width, height = img.size
    pixels = img.load()

    mask = []
    for y in range(height):
        row = []
        for x in range(width):
            on = pixels[x, y] >= threshold
            row.append(not on if invert else on)
        mask.append(row)
    return mask, width, height


def crop_mask(mask, crop_mode):
    """
    Trim blank rows and columns from the edges of the mask.

    crop_mode: "none", "vertical" (blank rows only), "horizontal" (blank
    columns only), or "both".

    Returns (cropped_mask, width, height, trimmed) where trimmed describes how
    many rows/columns were removed from each side.
    """
    trimmed = {"top": 0, "bottom": 0, "left": 0, "right": 0}

    if crop_mode == "none" or not mask:
        return mask, (len(mask[0]) if mask else 0), len(mask), trimmed

    rows = list(mask)

    if crop_mode in ("vertical", "both"):
        first = 0
        while first < len(rows) and not any(rows[first]):
            first += 1
        if first == len(rows):
            # Nothing is set anywhere; leave the mask empty.
            return [], 0, 0, trimmed
        last = len(rows) - 1
        while last > first and not any(rows[last]):
            last -= 1
        trimmed["top"] = first
        trimmed["bottom"] = len(rows) - 1 - last
        rows = rows[first:last + 1]

    if crop_mode in ("horizontal", "both") and rows:
        width = len(rows[0])
        first_col = 0
        while first_col < width and not any(r[first_col] for r in rows):
            first_col += 1
        if first_col == width:
            return [], 0, 0, trimmed
        last_col = width - 1
        while last_col > first_col and not any(r[last_col] for r in rows):
            last_col -= 1
        trimmed["left"] = first_col
        trimmed["right"] = width - 1 - last_col
        rows = [r[first_col:last_col + 1] for r in rows]

    height = len(rows)
    width = len(rows[0]) if rows else 0
    return rows, width, height, trimmed


def runs_in_row(row):
    """Yield (start_x, length) for each run of True values."""
    start = None
    for x, on in enumerate(row):
        if on and start is None:
            start = x
        elif not on and start is not None:
            yield start, x - start
            start = None
    if start is not None:
        yield start, len(row) - start


def mask_rectangles(mask, height, pixel_size, merge_rows=True):
    """
    Yield (x0, y0, x1, y1) rectangles in um for the set pixels in the mask.

    Image rows run top to bottom, GDS/LEF y runs bottom to top, so rows are
    flipped here. This is the single source of geometry for both outputs.
    """
    for y, row in enumerate(mask):
        flipped_y = height - 1 - y
        spans = runs_in_row(row) if merge_rows else (
            (x, 1) for x, on in enumerate(row) if on
        )
        for start_x, length in spans:
            x0 = start_x * pixel_size
            y0 = flipped_y * pixel_size
            yield (x0, y0, x0 + length * pixel_size, y0 + pixel_size)


def find_diagonals(mask):
    """
    Locate corner-only contacts.

    Returns a list of (x, y, kind) where the contact point is the shared corner
    between pixel (x, y) and its diagonal neighbour. kind is "dr" for a
    down-right diagonal ((x,y) with (x+1,y+1)) or "dl" for down-left
    ((x+1,y) with (x,y+1)).
    """
    found = []
    for y in range(len(mask) - 1):
        row, below = mask[y], mask[y + 1]
        for x in range(len(row) - 1):
            if row[x] and below[x + 1] and not row[x + 1] and not below[x]:
                found.append((x, y, "dr"))
            if row[x + 1] and below[x] and not row[x] and not below[x + 1]:
                found.append((x, y, "dl"))
    return found


def fix_diagonals_fill(mask):
    """
    Close corner contacts by setting a whole neighbouring pixel.

    Simple and always legal, but it visibly thickens diagonal lines.
    Returns (new_mask, filled_count).
    """
    out = [list(row) for row in mask]
    contacts = find_diagonals(mask)

    for x, y, kind in contacts:
        if kind == "dr":
            out[y][x + 1] = True
        else:
            out[y][x] = True

    return out, len(contacts)


def diagonal_bridges(mask, height, pixel_size, bridge_size):
    """
    Build small squares that straddle each corner contact.

    A square of side bridge_size centred on the shared corner overlaps both
    diagonal pixels, turning a point contact into a proper edge overlap while
    changing the artwork far less than filling a whole pixel.

    Returns a list of (x0, y0, x1, y1) rectangles in um.
    """
    half = bridge_size / 2.0
    bridges = []

    for x, y, _kind in find_diagonals(mask):
        # The shared corner sits at the boundary between columns x and x+1,
        # and between rows y and y+1. Rows are flipped for GDS orientation.
        corner_x = (x + 1) * pixel_size
        corner_y = (height - 1 - y) * pixel_size
        bridges.append((corner_x - half, corner_y - half,
                        corner_x + half, corner_y + half))

    return bridges


def check_drc(rects, mask, pixel_size, layer_name, layer, datatype,
              diagonals_fixed=False, bridge_rects=None):
    """
    Report shapes that will fail sky130 minimum-area or spacing rules.

    Rectangles are merged first, because DRC sees merged geometry: a small
    bridge square that overlaps a pixel is part of one larger shape, not a
    separate undersized one.

    Returns a list of human-readable problem strings. This is a pre-check for
    the rules bitmap art actually trips, not a DRC signoff.
    """
    problems = []
    if layer_name is None:
        return problems

    min_area = SKY130_MIN_AREA.get(layer_name)
    min_space = SKY130_MIN_SPACING.get(layer_name)

    if min_area is not None and rects:
        polys = [gdstk.rectangle((x0, y0), (x1, y1), layer=layer,
                                 datatype=datatype)
                 for x0, y0, x1, y1 in rects]
        merged = gdstk.boolean(polys, polys, "or", layer=layer,
                               datatype=datatype)

        too_small = []
        for poly in merged:
            area = abs(poly.area())
            if area < min_area:
                pts = poly.points
                too_small.append((pts[:, 0].min(), pts[:, 1].min(), area))

        if too_small:
            worst = min(a for _, _, a in too_small)
            side_needed = math.sqrt(min_area)
            problems.append(
                f"{len(too_small)} merged shape(s) below the {layer_name} "
                f"minimum area of {min_area} um^2 (smallest is {worst:.4f} "
                f"um^2). An isolated pixel needs --pixel-size >= "
                f"{side_needed:.3f} um; you have {pixel_size} um."
            )
            for x0, y0, area in too_small[:3]:
                problems.append(f"    e.g. shape at ({x0:.3f}, {y0:.3f}), "
                                f"area {area:.4f} um^2")
            if len(too_small) > 3:
                problems.append(f"    ...and {len(too_small) - 3} more")

    # Minimum width. An individual rectangle narrower than the minimum creates
    # a narrow protrusion in the merged shape, even when the merged shape is
    # large overall. A bridge square overhangs by half its side, so the bridge
    # must be at least twice the minimum width for that overhang to be legal.
    min_width = SKY130_MIN_WIDTH.get(layer_name)
    if min_width is not None and rects:
        bridges = set(bridge_rects or ())
        narrow = []
        for r in rects:
            if r in bridges:
                # A bridge is measured by its diagonal neck, checked below,
                # not by its side length.
                continue
            x0, y0, x1, y1 = r
            w, h = x1 - x0, y1 - y0
            if w < min_width - 1e-9 or h < min_width - 1e-9:
                narrow.append((x0, y0, min(w, h)))
        if narrow:
            worst = min(d for _, _, d in narrow)
            problems.append(
                f"{len(narrow)} shape(s) narrower than the {layer_name} minimum "
                f"width of {min_width} um (narrowest is {worst:.3f} um)."
            )
            for x0, y0, d in narrow[:3]:
                problems.append(f"    e.g. shape at ({x0:.3f}, {y0:.3f}), "
                                f"{d:.3f} um across")
            if len(narrow) > 3:
                problems.append(f"    ...and {len(narrow) - 3} more")

        if bridges:
            side = next(iter(bridges))
            neck = (side[2] - side[0]) / math.sqrt(2)
            if neck < min_width - 1e-9:
                problems.append(
                    f"Bridge necks are {neck:.3f} um, below the {layer_name} "
                    f"minimum width of {min_width} um. Increase --bridge-size "
                    f"to at least {min_width * math.sqrt(2):.3f} um."
                )

    if min_space is not None and pixel_size < min_space:
        has_gap = False
        for row in mask:
            for x in range(len(row) - 2):
                if row[x] and not row[x + 1] and row[x + 2]:
                    has_gap = True
                    break
            if has_gap:
                break
        if has_gap:
            problems.append(
                f"One-pixel gaps in the artwork are {pixel_size} um wide, below "
                f"the {layer_name} minimum spacing of {min_space} um. "
                f"Use --pixel-size >= {min_space} um, or remove single-pixel "
                f"gaps from the image."
            )

    # Corner-only contacts: two pixels touching at a single point are separate
    # shapes with zero spacing there. Scaling the art up does not help, since
    # the contact stays a point at any size; the corner has to be closed.
    if not diagonals_fixed:
        diagonals = len(find_diagonals(mask))
        if diagonals:
            problems.append(
                f"{diagonals} corner-only diagonal contact(s). Diagonally "
                f"adjacent pixels touch at a single point, giving zero spacing, "
                f"which fails {layer_name} spacing at ANY pixel size. Use "
                f"--fix-diagonals bridge (or fill) to close the corners."
            )

    return problems


def write_lef(path, macros, layer_name, layer_num, datatype, obs_mode):
    """
    Write a LEF abstract view.

    macros: list of (name, width, height, rects) where rects are (x0,y0,x1,y1)
            tuples in um, relative to the macro origin.
    obs_mode: "shapes" writes every rectangle as an obstruction, "bbox" writes
              a single covering rectangle, "none" writes no OBS section.
    """
    # LEF needs a layer name, not a GDS number. Fall back to a sensible guess
    # when the caller used a raw layer/datatype pair.
    lef_layer = layer_name
    if lef_layer is None:
        for name, (num, dt) in SKY130_LAYERS.items():
            if (num, dt) == (layer_num, datatype):
                lef_layer = name
                break
    if lef_layer is None:
        lef_layer = f"met{layer_num}"

    lines = [
        "VERSION 5.7 ;",
        'BUSBITCHARS "[]" ;',
        'DIVIDERCHAR "/" ;',
        "UNITS",
        "  DATABASE MICRONS 1000 ;",
        "END UNITS",
        "",
    ]

    for name, width, height, rects in macros:
        lines.append(f"MACRO {name}")
        lines.append("  CLASS BLOCK ;")
        lines.append("  ORIGIN 0.000 0.000 ;")
        lines.append(f"  FOREIGN {name} 0.000 0.000 ;")
        lines.append(f"  SIZE {width:.3f} BY {height:.3f} ;")
        lines.append("  SYMMETRY X Y R90 ;")

        if obs_mode != "none":
            lines.append("  OBS")
            lines.append(f"    LAYER {lef_layer} ;")
            if obs_mode == "bbox":
                lines.append(f"      RECT 0.000 0.000 {width:.3f} {height:.3f} ;")
            else:
                for x0, y0, x1, y1 in rects:
                    lines.append(
                        f"      RECT {x0:.3f} {y0:.3f} {x1:.3f} {y1:.3f} ;"
                    )
            lines.append("  END")

        lines.append(f"END {name}")
        lines.append("")

    lines.append("END LIBRARY")
    lines.append("")

    Path(path).write_text("\n".join(lines))


def build_cell(name, mask, width, height, pixel_size, layer, datatype,
               origin_x=0.0, origin_y=0.0, merge_rows=True,
               prboundary=None, extra_rects=None):
    """Build a gdstk cell of rectangles, and return the rectangles used."""
    cell = gdstk.Cell(name)
    pixel_count = sum(sum(row) for row in mask)

    rects = list(mask_rectangles(mask, height, pixel_size, merge_rows))
    if extra_rects:
        rects.extend(extra_rects)

    for x0, y0, x1, y1 in rects:
        cell.add(gdstk.rectangle((origin_x + x0, origin_y + y0),
                                 (origin_x + x1, origin_y + y1),
                                 layer=layer, datatype=datatype))

    # The place-and-route boundary covers the whole macro footprint, so the
    # flow can determine its extent. Without it the macro is unusable.
    if prboundary is not None:
        pr_layer, pr_datatype = prboundary
        cell.add(gdstk.rectangle(
            (origin_x, origin_y),
            (origin_x + width * pixel_size, origin_y + height * pixel_size),
            layer=pr_layer, datatype=pr_datatype))

    return cell, rects, pixel_count


def main():
    parser = argparse.ArgumentParser(
        description="Convert black-and-white PNGs into GDS chip art.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__.split("Examples")[1] if "Examples" in __doc__ else None,
    )
    parser.add_argument("images", nargs="+", type=Path,
                        help="Input PNG file(s). Each becomes its own cell.")
    parser.add_argument("-o", "--output", type=Path, required=True,
                        help="Output GDS file.")
    parser.add_argument("--pixel-size", type=float, default=1.0,
                        help="Size of one pixel in um (default: 1.0).")
    parser.add_argument("--min-size", type=float, default=None,
                        help="Minimum allowed pixel size in um. If --pixel-size "
                             "is smaller, it is raised to this value. Defaults "
                             "to the chosen layer's minimum drawn width.")
    parser.add_argument("--threshold", type=int, default=128,
                        help="Grey level 0-255; pixels >= this become metal "
                             "(default: 128).")
    parser.add_argument("--invert", action="store_true",
                        help="Place metal on dark pixels instead of light ones.")
    parser.add_argument("--layer", type=parse_layer, default=parse_layer("met4"),
                        help="Target layer: a sky130 name (met1..met5, li1), a "
                             "number, or 'layer/datatype' (default: met4).")
    parser.add_argument("--origin", type=float, nargs=2, metavar=("X", "Y"),
                        default=(0.0, 0.0),
                        help="Lower-left corner of the artwork in um "
                             "(default: 0 0).")
    parser.add_argument("--spacing", type=float, default=0.0,
                        help="Gap in um between images when placing several "
                             "side by side in the top cell (default: 0).")
    parser.add_argument("--lef", type=Path, default=None,
                        help="Also write a LEF abstract view. Defaults to the "
                             "output path with a .lef extension; pass a path to "
                             "override, or omit --lef entirely to skip it.")
    parser.add_argument("--no-lef", action="store_true",
                        help="Do not write a LEF file.")
    parser.add_argument("--lef-obs", choices=["shapes", "bbox", "none"],
                        default="shapes",
                        help="What to put in the LEF OBS section: 'shapes' "
                             "blocks each rectangle (default), 'bbox' blocks the "
                             "whole bounding box, 'none' declares no "
                             "obstructions.")
    parser.add_argument("--crop", choices=["none", "vertical", "horizontal", "both"],
                        default="vertical",
                        help="Trim blank bands from the edges: 'vertical' drops "
                             "empty rows top and bottom (default), 'horizontal' "
                             "drops empty columns left and right, 'both' does "
                             "each, 'none' keeps the image as-is.")
    parser.add_argument("--no-merge", action="store_true",
                        help="Emit one rectangle per pixel instead of merging "
                             "horizontal runs. Larger files; rarely wanted.")
    parser.add_argument("--prboundary", default="235/4",
                        help="Layer/datatype for the place-and-route boundary "
                             "rectangle covering the macro footprint "
                             "(default: 235/4, the sky130 PR boundary). Pass "
                             "'none' to omit it, though the flow normally "
                             "requires it.")
    parser.add_argument("--name", nargs="+", default=None, metavar="NAME",
                        help="Cell/macro name(s), used in both the GDS and the "
                             "LEF. Give one name per image to set them "
                             "individually, or a single name to use as-is for "
                             "one image (or as a NAME_0, NAME_1... prefix for "
                             "several). Defaults to each file's stem.")
    parser.add_argument("--no-top-cell", action="store_true",
                        help="Do not create the wrapper top cell. Use this when "
                             "the GDS is a macro view: the flow requires exactly "
                             "one top-level cell, and the wrapper makes two. "
                             "Only valid with a single input image.")
    parser.add_argument("--top-cell", default="CHIP_ART",
                        help="Name of the top cell holding the images "
                             "(default: CHIP_ART).")
    parser.add_argument("--fix-diagonals", choices=["none", "bridge", "fill"],
                        default="none",
                        help="Close corner-only contacts, which otherwise fail "
                             "minimum-spacing DRC at any pixel size. 'bridge' "
                             "adds a small square straddling each corner, "
                             "changing the artwork least. 'fill' sets a whole "
                             "neighbouring pixel, which is blunter but always "
                             "works. Default: none.")
    parser.add_argument("--bridge-size", type=float, default=None,
                        help="Side length in um of the corner bridge squares. "
                             "Defaults to the layer's minimum width. Raised "
                             "automatically if it would be too small to overlap "
                             "both pixels legally.")
    parser.add_argument("--strict", action="store_true",
                        help="Exit with an error if the DRC pre-check finds "
                             "shapes that will fail minimum area or spacing, "
                             "instead of only warning.")
    parser.add_argument("--no-drc-check", action="store_true",
                        help="Skip the minimum area and spacing pre-check.")
    parser.add_argument("--dry-run", action="store_true",
                        help="Report what would be produced without writing.")

    args = parser.parse_args()

    (layer_num, datatype), layer_name = args.layer

    # Resolve the minimum pixel size.
    min_size = args.min_size
    if min_size is None and layer_name is not None:
        min_size = SKY130_MIN_WIDTH.get(layer_name)

    pixel_size = args.pixel_size
    if min_size is not None and pixel_size < min_size:
        print(f"Pixel size {pixel_size} um is below the minimum {min_size} um; "
              f"raising it to {min_size} um.")
        pixel_size = min_size

    # Resolve the PR boundary layer.
    if args.prboundary.lower() in ("none", "off", ""):
        prboundary = None
    else:
        try:
            if "/" in args.prboundary:
                pr_l, pr_d = args.prboundary.split("/", 1)
                prboundary = (int(pr_l), int(pr_d))
            else:
                prboundary = (int(args.prboundary), 0)
        except ValueError:
            sys.exit(f"Invalid --prboundary '{args.prboundary}'. "
                     "Use 'layer/datatype', a number, or 'none'.")

    # Resolve the corner-bridge size.
    #
    # A bridge centred on the shared vertex leaves a narrow neck: the tightest
    # cut across the merged shape runs diagonally through the bridge, between
    # the corners of the two empty quadrants, and measures (bridge/2)*sqrt(2).
    # So the smallest legal bridge is min_width * sqrt(2).
    #
    # Note the bridge also overhangs into each empty quadrant by bridge/2. At
    # this minimum size that overhang is narrower than min_width. If DRC
    # objects to the overhang rather than the neck, use 2 * min_width instead
    # (0.6 um on met4), which satisfies both readings.
    min_width = (SKY130_MIN_WIDTH.get(layer_name, 0.30) if layer_name else 0.30)
    min_bridge = math.ceil(min_width * math.sqrt(2) * 1000) / 1000

    bridge_size = args.bridge_size
    if bridge_size is None:
        bridge_size = min_bridge

    if args.fix_diagonals == "bridge":
        if bridge_size < min_bridge - 1e-9:
            neck = bridge_size / math.sqrt(2)
            print(f"Bridge size {bridge_size} um leaves a {neck:.3f} um neck, "
                  f"below the {layer_name} minimum width of {min_width} um; "
                  f"raising it to {min_bridge} um.")
            bridge_size = min_bridge

        print(f"Diagonal fix: bridge ({bridge_size} um squares, "
              f"{bridge_size / math.sqrt(2):.3f} um neck)")
    elif args.fix_diagonals == "fill":
        print("Diagonal fix: fill")

    if layer_name:
        print(f"Layer: {layer_name} ({layer_num}/{datatype})")
    else:
        print(f"Layer: {layer_num}/{datatype}")
    if prboundary:
        print(f"PR boundary: {prboundary[0]}/{prboundary[1]}")
    else:
        print("PR boundary: omitted")
    print(f"Pixel size: {pixel_size} um")
    print(f"Threshold: {args.threshold}{' (inverted)' if args.invert else ''}")
    print()

    # Work out the cell name for each image before building anything.
    if args.name is None:
        cell_names = [p.stem.upper().replace(" ", "_").replace("-", "_")
                      for p in args.images]
    elif len(args.name) == len(args.images):
        cell_names = list(args.name)
    elif len(args.name) == 1:
        base = args.name[0]
        if len(args.images) == 1:
            cell_names = [base]
        else:
            cell_names = [f"{base}_{i}" for i in range(len(args.images))]
    else:
        sys.exit(f"Got {len(args.name)} name(s) for {len(args.images)} image(s). "
                 "Pass one name per image, or a single name.")

    if args.no_top_cell and len(args.images) > 1:
        sys.exit("--no-top-cell needs a single input image; with several there "
                 "would be several top-level cells, which the flow rejects. "
                 "Run the script once per image instead.")

    seen = set()
    for name in cell_names:
        if name in seen:
            sys.exit(f"Duplicate cell name '{name}'. Names must be unique.")
        seen.add(name)
        if not args.no_top_cell and name == args.top_cell:
            sys.exit(f"Cell name '{name}' collides with the top cell name. "
                     "Use --top-cell to rename the top cell, or --no-top-cell "
                     "to omit the wrapper entirely.")

    lib = gdstk.Library()
    top = None if args.no_top_cell else gdstk.Cell(args.top_cell)

    cursor_x = args.origin[0]
    total_rects = 0
    lef_macros = []
    drc_problem_count = 0

    for path, cell_name in zip(args.images, cell_names):
        if not path.is_file():
            sys.exit(f"Input not found: {path}")

        mask, width, height = load_mask(path, args.threshold, args.invert)
        orig_w, orig_h = width, height

        mask, width, height, trimmed = crop_mask(mask, args.crop)

        bridges = []
        if args.fix_diagonals == "fill":
            mask, filled = fix_diagonals_fill(mask)
            if filled:
                print(f"  Filled {filled} diagonal corner(s).")
        elif args.fix_diagonals == "bridge":
            contacts = find_diagonals(mask)
            if contacts:
                bridges = diagonal_bridges(mask, height, pixel_size,
                                           bridge_size)
                print(f"  Bridged {len(contacts)} diagonal corner(s) with "
                      f"{bridge_size:.3f} um squares.")

        cell, rects, pixels_on = build_cell(
            cell_name, mask, width, height, pixel_size,
            layer_num, datatype,
            merge_rows=not args.no_merge,
            prboundary=prboundary,
            extra_rects=bridges,
        )
        rect_count = len(rects)

        art_w = width * pixel_size
        art_h = height * pixel_size

        print(f"{path.name}  ->  cell {cell_name}")
        if (width, height) != (orig_w, orig_h):
            trim_parts = [f"{v} {k}" for k, v in trimmed.items() if v]
            print(f"  {orig_w} x {orig_h} px, cropped to {width} x {height} px "
                  f"(trimmed {', '.join(trim_parts)})")
        else:
            print(f"  {width} x {height} px")
        print(f"  -> {art_w:.3f} x {art_h:.3f} um")
        print(f"  {pixels_on} pixels on, {rect_count} rectangle(s)")

        if pixels_on == 0:
            print("  WARNING: no pixels above the threshold; cell will be empty.")

        if not args.no_drc_check:
            problems = check_drc(rects, mask, pixel_size, layer_name,
                                 layer_num, datatype,
                                 diagonals_fixed=(args.fix_diagonals != "none"),
                                 bridge_rects=bridges)
            for p in problems:
                print(f"  DRC: {p}")
            if problems:
                drc_problem_count += 1

        lef_macros.append((cell_name, art_w, art_h, rects))

        lib.add(cell)
        if top is not None:
            top.add(gdstk.Reference(cell, (cursor_x, args.origin[1])))
        cursor_x += art_w + args.spacing
        total_rects += rect_count

    if top is not None:
        lib.add(top)

    total_w = cursor_x - args.spacing - args.origin[0] if args.images else 0
    print()
    print(f"Total: {total_rects} rectangle(s), overall extent "
          f"{total_w:.3f} um wide")

    if drc_problem_count:
        print()
        print(f"DRC pre-check flagged {drc_problem_count} image(s). These shapes "
              f"will fail sign-off DRC as drawn.")
        if args.strict:
            sys.exit("Aborting because --strict was given.")

    if args.dry_run:
        print("Dry run: no file written.")
        return

    lib.write_gds(args.output)
    print(f"Wrote {args.output}")

    if not args.no_lef:
        lef_path = args.lef if args.lef is not None else args.output.with_suffix(".lef")
        write_lef(lef_path, lef_macros, layer_name, layer_num, datatype,
                  args.lef_obs)
        print(f"Wrote {lef_path}")


if __name__ == "__main__":
    main()