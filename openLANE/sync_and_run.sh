#!/usr/bin/env bash
#
# sync_and_run.sh
#
# Syncs every HDL file referenced in a Verilog filelist (.f) from a
# Windows-side source tree into your WSL OpenLane project directory,
# then runs the OpenLane flow.
#
# Only files listed in the .f file (plus any +incdir+/-I directories
# it references) are copied -- if you add a new source file to the
# project, add it to the .f file too, or it won't be picked up here.

set -euo pipefail

# ---------------------------------------------------------------------------
# CONFIGURATION - edit these lines for your project
# ---------------------------------------------------------------------------
WIN_ROOT="/mnt/c/Users/stanr/Documents/SRB-CC/SR-GA1"   # Windows-side project root
WSL_ROOT="$HOME/openLANE/SR-GA1"                         # WSL-side OpenLane project root
FILELIST="hdl/clb.f"                                  # filelist path, relative to WIN_ROOT
CONFIG="CLB/config.json"                                      # OpenLane config, relative to WSL_ROOT
PDK_ROOT="$HOME/.volare"
TESTBENCH_PATTERN="_unit_test.sv"                         # suffix that marks a file as non-synthesizable
# ---------------------------------------------------------------------------

if ! command -v rsync >/dev/null 2>&1; then
    echo "ERROR: rsync is not installed. Run: sudo apt install rsync" >&2
    exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
    echo "ERROR: jq is not installed. Run: sudo apt install jq" >&2
    exit 1
fi

filelist_path="$WIN_ROOT/$FILELIST"
filelist_dir="$(dirname "$filelist_path")"

if [[ ! -f "$filelist_path" ]]; then
    echo "ERROR: filelist not found at $filelist_path" >&2
    exit 1
fi

LAST_SYNCED_REL=""

sync_one() {
    # $1 = absolute source path on the Windows side (file or dir)
    local src="$1"
    local rel
    rel="$(realpath --relative-to="$WIN_ROOT" "$src")"
    local dest="$WSL_ROOT/$rel"

    if [[ -d "$src" ]]; then
        mkdir -p "$dest"
        rsync -a --update "$src"/ "$dest"/
    else
        mkdir -p "$(dirname "$dest")"
        rsync -a --update "$src" "$dest"
    fi
    echo "  synced: $rel"
    LAST_SYNCED_REL="$rel"
}

echo "Syncing HDL sources listed in $FILELIST ..."
copied=0
verilog_src=()   # relative paths of real (non-testbench) source files, for VERILOG_FILES

while IFS= read -r raw_line || [[ -n "$raw_line" ]]; do
    line="${raw_line%$'\r'}"                      # strip stray CR from Windows-edited files
    line="$(echo "$line" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"

    [[ -z "$line" ]] && continue                  # skip blank lines
    [[ "$line" == \#* || "$line" == //* ]] && continue   # skip comments

    # +incdir+<path> or -I<path> style entries -> sync the whole directory
    if [[ "$line" == +incdir+* || "$line" == -I* ]]; then
        dir_entry="${line#+incdir+}"
        dir_entry="${dir_entry#-I}"
        dir_entry="$(echo "$dir_entry" | sed 's/^[[:space:]]*//')"
        [[ "$dir_entry" != /* ]] && dir_entry="$filelist_dir/$dir_entry"
        if [[ -d "$dir_entry" ]]; then
            sync_one "$(realpath "$dir_entry")"
        else
            echo "  WARNING: incdir not found, skipping: $dir_entry" >&2
        fi
        continue
    fi

    [[ "$line" == +* ]] && continue                # skip +define+ and other plusargs

    # otherwise: treat as a source file, resolved relative to the filelist's own directory
    file="$line"
    [[ "$file" != /* ]] && file="$filelist_dir/$file"

    if [[ ! -f "$file" ]]; then
        echo "  WARNING: listed file not found, skipping: $file" >&2
        continue
    fi

    sync_one "$(realpath "$file")"
    copied=$((copied + 1))

    # Track it as a real source file unless it's a testbench, and unless
    # it's a header/include-only file (those belong in VERILOG_INCLUDE_DIRS,
    # not VERILOG_FILES).
    case "$LAST_SYNCED_REL" in
        *"$TESTBENCH_PATTERN") ;;                 # skip: testbench, don't add
        *.vh|*.svh) ;;                             # skip: header/include file
        *.v|*.sv) verilog_src+=("$WSL_ROOT/$LAST_SYNCED_REL") ;;
    esac
done < "$filelist_path"

# keep a synced copy of the filelist itself too
sync_one "$(realpath "$filelist_path")"

echo "Done. $copied HDL file(s) synced (${#verilog_src[@]} added to VERILOG_FILES, testbenches excluded)."

echo "Updating VERILOG_FILES in $CONFIG ..."
files_json="$(printf '%s\n' "${verilog_src[@]}" | jq -R -s -c 'split("\n") | map(select(length > 0))')"
tmp_config="$(mktemp)"
jq --argjson files "$files_json" '.VERILOG_FILES = $files' "$WSL_ROOT/$CONFIG" > "$tmp_config"
mv "$tmp_config" "$WSL_ROOT/$CONFIG"

echo ""
echo "Running OpenLane flow..."
cd "$WSL_ROOT"
python3 -m openlane --dockerized --pdk-root "$PDK_ROOT" "$CONFIG"
