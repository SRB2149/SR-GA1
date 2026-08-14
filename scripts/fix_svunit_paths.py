#!/usr/bin/env python3
"""
fix_svunit_paths.py

Converts POSIX/MSYS-style absolute paths (e.g. /c/Users/...) written into an
SVUnit-generated .svunit.f filelist into native Windows form (C:/Users/...),
so that native Windows simulator binaries (e.g. ModelSim's vlog.exe) can
read them.

Why this is needed: SVUnit's helper scripts (buildSVUnit, runSVUnit, etc.)
have no file extension and rely on their #!/usr/bin/env perl shebang line to
run -- that only works correctly when invoked through Git Bash's own bundled
(MSYS-aware) Perl. But that same Perl's Cwd::getcwd() always returns
POSIX-style paths regardless of platform, and those get written verbatim
into .svunit.f. A native Windows Perl (e.g. Strawberry Perl) would report
Windows-style paths instead, but then can't invoke the extension-less helper
scripts at all. This script lets you keep using Git Bash's Perl (so
everything actually runs) and just fixes up the resulting paths afterward.

Usage:
    python3 fix_svunit_paths.py path/to/.svunit.f
"""

import re
import sys

# Matches a POSIX/MSYS drive-mount path like "/c/" -- but only where it looks
# like the start of an absolute path (start of line, or right after a
# non-path character like '+' from "+incdir+"), not something that happens
# to contain "/x/" in the middle of an unrelated string.
DRIVE_RE = re.compile(r'(?<![:/\w])/([A-Za-z])/')


def convert(text: str) -> str:
    return DRIVE_RE.sub(lambda m: f"{m.group(1).upper()}:/", text)


def main():
    if len(sys.argv) != 2:
        print("Usage: fix_svunit_paths.py <file>", file=sys.stderr)
        sys.exit(1)

    path = sys.argv[1]
    with open(path) as f:
        text = f.read()

    new_text, n = DRIVE_RE.subn(lambda m: f"{m.group(1).upper()}:/", text)

    with open(path, 'w') as f:
        f.write(new_text)

    print(f"Converted {n} path(s) in {path} to Windows form", file=sys.stderr)


if __name__ == '__main__':
    main()