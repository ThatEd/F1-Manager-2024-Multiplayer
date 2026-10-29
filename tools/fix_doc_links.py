#!/usr/bin/env python3
"""Repoint the docs' cross-references after the bundle was given its own layout.

The write-ups were written in a flat working folder that also held the decompiled UI export. Moving them
into `docs/` (and renaming the headless README) broke two kinds of link:

1. **Intra-bundle links** that used to resolve and now do not, because the file moved one level down.
   These are repaired: a reader following them should land on the write-up.

2. **Absolute paths into the export** (`E:\\FModel\\...`) or into the author's private workspace. These
   can never resolve for anyone else - the export is deliberately not shipped - so they are turned into
   inline code spans. That keeps the citation (which is the valuable part: it names the file and line that
   proves a claim) while removing a link that could only ever 404.

Run from the bundle root. Idempotent: running it twice changes nothing the second time.
"""
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
# The script lives in tools/, so the bundle root is one level up from it, not its own directory.
ROOT = os.path.dirname(HERE)
DOCS = os.path.join(ROOT, "docs")

# Absolute-path links, which become code spans rather than links.
ABS = re.compile(r"\[([^\]]+)\]\((?:[A-Za-z]:[\\/][^)]*)\)")


def code_span(m):
    text = m.group(1)
    # Strip an anchor if the author added one; the code span keeps the meaning.
    return "`%s`" % text


# Rewrites that must apply to link targets only, so prose is left alone.
TARGET_REWRITES = [
    # private workspace root -> the docs folder they now live in
    (re.compile(r"^[A-Za-z]:[\\/][^)]*[\\/]local[\\/]f1-manager-2024[\\/]([A-Za-z0-9\-]+\.md)$"), r"\1"),
    (re.compile(r"^[A-Za-z]:[\\/][^)]*[\\/]local[\\/]f1-manager-2024[\\/]pak[\\/]"), r"../pak/"),
    # a doc that moved into docs/ now points at its siblings, not two levels up
    (re.compile(r"^\.\./\.\./([A-Z][A-Z0-9\-]+\.md)$"), r"\1"),
    # the headless README was renamed
    (re.compile(r"^(?:\.\./)?tools/headless/README\.md$"), r"HEADLESS-HARNESS.md"),
    # NOTES.md stayed at the bundle root
    (re.compile(r"^(?:\.\./)?NOTES\.md$"), r"../NOTES.md"),
]


def fix_target(target):
    for rx, rep in TARGET_REWRITES:
        new = rx.sub(rep, target)
        if new != target:
            return new
    return target


def read_text(path):
    """Read a doc, tolerating the encodings these files accumulated before they were one folder.

    One write-up was saved as cp1252 (a `§` became byte 0xA7), and a strict UTF-8 read aborts the whole
    run on it. Falling back keeps the tool usable instead of requiring the caller to know which file it is.
    """
    with open(path, "rb") as fh:
        raw = fh.read()
    for enc in ("utf-8", "cp1252", "latin-1"):
        try:
            return raw.decode(enc)
        except UnicodeDecodeError:
            continue
    return raw.decode("utf-8", "replace")


def fix_file(path):
    text = read_text(path)
    original = text

    # 1. Repair relative targets.
    def on_link(m):
        label, target = m.group(1), m.group(2)
        if target.startswith(("http://", "https://", "#", "mailto:")):
            return m.group(0)
        new = fix_target(target)
        return "[%s](%s)" % (label, new)

    text = re.sub(r"\[([^\]]+)\]\(([^)#\s]+)\)", on_link, text)

    # 2. Absolute paths -> code spans, after the relative fixes so they are not re-linked.
    text = ABS.sub(code_span, text)

    if text != original:
        with open(path, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(text)
        return True
    return False


def main():
    targets = [os.path.join(DOCS, n) for n in sorted(os.listdir(DOCS)) if n.endswith(".md")]
    root_readme = os.path.join(ROOT, "README.md")
    if os.path.isfile(root_readme):
        targets.append(root_readme)
    changed = [p for p in targets if fix_file(p)]
    for p in changed:
        print("fixed " + os.path.relpath(p, ROOT))
    print("%d of %d files rewritten" % (len(changed), len(targets)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
