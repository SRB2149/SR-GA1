#!/usr/bin/env python3
"""
resolve_filelist_paths.py

Rewrites relative file paths in a vlog/vsim -f filelist into absolute paths,
anchored to the directory containing the filelist itself. Already-absolute
paths (POSIX or Windows-drive-letter form) are left untouched.

Why this is needed: ModelSim resolves relative paths inside a -f filelist
against its OWN current working directory at invocation time, not against
the filelist's location. Our sim/sim_all targets run vlog from the tb/
output tree (required, since the SVUnit-generated filelist has its own
file referenced by a bare relative name that only resolves from there), so
a project filelist (e.g. sr-ga1.f) written with paths relative to hdl/
would silently fail to resolve. Pre-resolving to absolute paths here makes
it work regardless of vlog's actual cwd.

Handles:
  - Plain file paths, one per line (main case)
  - +incdir+<path> entries (path portion resolved, prefix preserved)
  - Comments (// ...) and blank lines passed through unchanged
  - Flags/switches (starting with -) and other +plusargs passed through
    unchanged, since they aren't file paths
  - Lines already using absolute paths (POSIX / or Windows C:/, C:\\) are
    left as-is

Usage:
    python3 resolve_filelist_paths.py <input.f> <output.f>
"""

import os
import re
import sys

ABS_PATH_RE = re.compile(r'^([A-Za-z]:[\\/]|/)')


def is_absolute(path: str) -> bool:
    return bool(ABS_PATH_RE.match(path))


def looks_like_path(token: str) -> bool:
    return '/' in token or '\\' in token or token.lower().endswith(('.sv', '.v', '.svh', '.vh', '.f'))


def resolve_token(token: str, base_dir: str) -> str:
    if is_absolute(token):
        return token
    return os.path.normpath(os.path.join(base_dir, token)).replace(os.sep, '/')


def resolve_line(line: str, base_dir: str) -> str:
    stripped = line.strip()
    if not stripped or stripped.startswith('//') or stripped.startswith('#'):
        return line

    tokens = stripped.split()
    new_tokens = []
    for tok in tokens:
        if tok.startswith('+incdir+'):
            path = tok[len('+incdir+'):]
            new_tokens.append('+incdir+' + resolve_token(path, base_dir))
        elif tok.startswith('-') or (tok.startswith('+') and not tok.startswith('+incdir+')):
            new_tokens.append(tok)
        elif looks_like_path(tok):
            new_tokens.append(resolve_token(tok, base_dir))
        else:
            new_tokens.append(tok)
    return ' '.join(new_tokens) + '\n'


def main():
    if len(sys.argv) != 3:
        print("Usage: resolve_filelist_paths.py <input.f> <output.f>", file=sys.stderr)
        sys.exit(1)

    in_path, out_path = sys.argv[1], sys.argv[2]
    base_dir = os.path.dirname(os.path.abspath(in_path))

    with open(in_path) as f:
        lines = f.readlines()

    resolved = [resolve_line(line, base_dir) for line in lines]

    with open(out_path, 'w') as f:
        f.writelines(resolved)

    print(f"Resolved {in_path} -> {out_path} (paths anchored to {base_dir})", file=sys.stderr)


if __name__ == '__main__':
    main()