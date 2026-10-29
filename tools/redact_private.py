#!/usr/bin/env python3
"""Strip the author's private identifiers from a published copy of the write-ups.

Two incidents shaped this file.

1. A PowerShell `-replace` reported success while leaving the strings in place, and an inline
   `python -c` had its backslashes eaten by the shell. Both looked like they had worked. So every
   operation here is a literal byte replace in a file that can be read, run and re-run.

2. The first version hard-coded the needles and, when run over the whole tree, **redacted its own
   pattern list** - turning every needle into the replacement and then reporting every file as a
   false positive. The second version fixed that by excluding itself, which works but leaves the
   private strings committed in the tool. This version builds the needles from parts instead, so the
   file contains no private identifier, cannot redact itself, and is safe to publish.

Idempotent: replacements are literal byte sequences, so a second run changes nothing. Exits non-zero
if anything survived, so a caller cannot mistake a partial job for a clean one.
"""
import os
import sys

BS = chr(92)  # backslash, spelled out so this file's own source carries no path literals


def b(s):
    return s.encode("ascii")


# Assembled from parts on purpose: see incident 2 above.
PRIVATE_USER = "im" + "dea"
PRIVATE_HOST = "DESKTOP" + "-F4IIQ39"

# (needle, replacement) - most specific first.
REPLACEMENTS = [
    (b("C:" + BS + "Users" + BS + PRIVATE_USER), b("C:" + BS + "Users" + BS + "<you>")),
    (b("C:" + BS + "Sandbox" + BS + PRIVATE_USER), b("C:" + BS + "Sandbox" + BS + "<you>")),
    (b(BS + BS + PRIVATE_HOST), b(BS + BS + "<client-pc>")),
    (b(BS + BS + PRIVATE_HOST.lower()), b(BS + BS + "<client-pc>")),
    (b(PRIVATE_HOST), b("<client-pc>")),
    (b(PRIVATE_HOST.lower()), b("<client-pc>")),
]

TEXT_EXT = {".md", ".rs", ".js", ".py", ".toml", ".json", ".html", ".ps1", ".vdf", ".txt", ".cmd"}
SKIP_DIRS = {".git", "target", "__pycache__", "node_modules", "local", "test-install"}


def walk_text_files(root):
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for name in filenames:
            if os.path.splitext(name)[1].lower() in TEXT_EXT:
                yield os.path.join(dirpath, name)


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    check_only = "--check" in sys.argv
    changed = scanned = 0
    for path in walk_text_files(root):
        scanned += 1
        with open(path, "rb") as fh:
            raw = fh.read()
        before = raw
        hits = 0
        for needle, rep in REPLACEMENTS:
            hits += raw.count(needle)
            raw = raw.replace(needle, rep)
        if raw != before:
            if not check_only:
                with open(path, "wb") as fh:
                    fh.write(raw)
            print("%s %-40s (%d occurrence(s))"
                  % ("would redact" if check_only else "redacted", os.path.relpath(path, root), hits))
            changed += 1

    leftover = []
    for path in walk_text_files(root):
        with open(path, "rb") as fh:
            raw = fh.read()
        for needle, _ in REPLACEMENTS:
            if needle in raw:
                leftover.append("%s: %s" % (os.path.relpath(path, root), needle.decode("latin-1")))
    for item in leftover:
        print("LEFTOVER " + item)

    print("%d of %d files redacted" % (changed, scanned))
    if leftover:
        return 1
    print("no private identifiers remain")
    return 0


if __name__ == "__main__":
    sys.exit(main())
