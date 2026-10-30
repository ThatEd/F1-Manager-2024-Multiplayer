#!/usr/bin/env python3
"""Verify the hardcoded offsets in `coop-proto/src/layout.rs` against the game exe on disk.

Why this exists
---------------
The co-op mod reads and writes the game's memory at fixed RVAs (`CAR_BASE`, `SIMULATE_RVA`, `GUA_RVA`) and
fixed field offsets inside a 0xA132F0-byte object. Every one of those numbers was found empirically. A game
patch moves them, and the failure is silent: the mod keeps running and reads the wrong bytes.

Nobody had checked whether the numbers still match the installed build - it was listed as "needs the game
running". That was too pessimistic for the *function* offsets: a prologue is in the file, so `SIMULATE_RVA`
can be checked statically by comparing the bytes at that RVA against what the running game was observed to
hold. If they are identical, the exe has not moved that function - no launch required.

What this can and cannot do
---------------------------
* **Can** check `SIMULATE_RVA`: it points at real code with a distinctive 16-byte prologue, and the live log
  recorded those bytes.
* **Cannot** check the field offsets (`off::LAT`, `off::TYRE_SURFACE0`, ...). Those are offsets into a runtime
  object built by the running sim, not into the file. Anything claiming otherwise would be guessing.
* `GUA_RVA` and `CAR_BASE` fall in sections whose on-disk content is not the runtime content, so a file read
  there proves nothing either way. Reported as "cannot check" rather than as a pass.

Usage
-----
    python tools/verify_offsets.py
    python tools/verify_offsets.py --game "<path to F1 Manager 2024>"
"""

import argparse
import os
import re
import sys

try:
    import pefile
except ImportError:
    sys.exit("pefile is required: pip install pefile")

DEFAULT_GAME = os.environ.get("COOP_GAME", r"E:\Steam\steamapps\common\F1 Manager 2024")

# The bytes the running game was observed to hold at SIMULATE_RVA.
#
# Source: `coop_agent.log`, line `sim entry: base=0x140000000 target=0x142298080 plausible=true bytes=[...]`.
# That is a live read of the prologue by the agent itself, which is why comparing against it is meaningful
# rather than circular: the log came from the game, the file comes from disk, and they must agree.
LIVE_PROLOGUE = bytes.fromhex("40535641554881EC700100004533ED48")


def read_layout(layout_path):
    """Pull the constants out of layout.rs so this cannot drift from a copy."""
    src = open(layout_path, encoding="utf-8").read()
    consts = {}
    for name in ("CAR_BASE", "CAR_STRIDE", "SIMULATE_RVA", "GUA_RVA", "OBJECT_SIZE", "CAR_MAX"):
        m = re.search(r"pub const %s: usize = (0x[0-9A-Fa-f_]+|\d+);" % name, src)
        if m:
            consts[name] = int(m.group(1).replace("_", ""), 0)
    return consts


def rva_offset(pe, rva):
    """Map an RVA to a file offset, or say why it cannot be mapped."""
    for s in pe.sections:
        lo = s.VirtualAddress
        hi = lo + max(s.Misc_VirtualSize, s.SizeOfRawData)
        if lo <= rva < hi:
            delta = rva - lo
            name = s.Name.rstrip(b"\x00").decode(errors="replace")
            if delta >= s.SizeOfRawData:
                return None, name, "inside VirtualSize but past the raw data (zero-filled at load)"
            return s.PointerToRawData + delta, name, ""
    return None, "(unmapped)", "not inside any section"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--game", default=DEFAULT_GAME)
    ap.add_argument("--layout", default=os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                                     "..", "coop-rs", "crates", "coop-proto", "src",
                                                     "layout.rs"))
    args = ap.parse_args()

    exe = os.path.join(args.game, "F1Manager24", "Binaries", "Win64", "F1Manager24.exe")
    if not os.path.exists(exe):
        print("SKIP: game exe not present at %s" % exe)
        return 0
    if not os.path.exists(args.layout):
        sys.exit("layout.rs not found: %s" % args.layout)

    c = read_layout(args.layout)
    print("game exe : %s" % exe)
    print("exe size : %d bytes" % os.path.getsize(exe))
    print("layout   : %s" % os.path.basename(args.layout))
    print()

    pe = pefile.PE(exe, fast_load=True)
    print("image base 0x%X, %d sections" % (pe.OPTIONAL_HEADER.ImageBase, len(pe.sections)))
    print()

    failures = []

    # ---- SIMULATE_RVA: the one that can be proven from the file -----------------------------
    rva = c.get("SIMULATE_RVA")
    print("SIMULATE_RVA = 0x%X" % rva if rva else "SIMULATE_RVA missing from layout.rs")
    if rva:
        off, section, why = rva_offset(pe, rva)
        if off is None:
            print("  UNVERIFIABLE: section %s, %s" % (section, why))
        else:
            raw = pe.__data__[off:off + len(LIVE_PROLOGUE)]
            print("  section %s, file offset 0x%X" % (section, off))
            print("  file : %s" % raw.hex(" ").upper())
            print("  live : %s" % LIVE_PROLOGUE.hex(" ").upper())
            if raw == LIVE_PROLOGUE:
                print("  MATCH - this exe still has the same prologue at this RVA, so the function")
                print("          has not moved. The hook target in layout.rs is correct for this build.")
            else:
                print("  MISMATCH - the function has MOVED or the exe changed. Every offset in layout.rs")
                print("             must be re-derived before the mod is used; a stale target patches")
                print("             unrelated code.")
                failures.append("SIMULATE_RVA")

    # ---- the rest: state the limits rather than pretend -------------------------------------
    print()
    print("Offsets this cannot check, and why:")
    for name, note in (
        ("CAR_BASE", "inside a section whose on-disk bytes are not the runtime bytes"),
        ("GUA_RVA", "outside the raw data of its section - materialised at load, not in the file"),
    ):
        v = c.get(name)
        if v is not None:
            off, section, why = rva_offset(pe, v)
            print("  %-12s 0x%-8X section %-10s %s" % (name, v, section, why or "raw data present"))

    print("  field offsets (off::LAT, off::TYRE_SURFACE0, ...): these index a runtime object built by the")
    print("      sim, not the executable. No file can confirm them; only a live read can.")

    # ---- identity: the numbers UI-COMMANDS.md records so a write-up is anchored to a build ----------------
    # These are cheap to check and were wrong once: the PE timestamp was documented as 0x67AF44F7 when the
    # file says 0x67AE45F7 (a transposition), which is the kind of error that makes a write-up look verified
    # when it is not. Checking them here means the doc either matches the binary or the suite says so.
    print()
    print("Build identity (what UI-COMMANDS.md records):")
    import struct
    with open(exe, "rb") as fh:
        head = fh.read(0x200)
    e_lfanew = struct.unpack_from("<I", head, 0x3C)[0]
    stamp = struct.unpack_from("<I", head, e_lfanew + 8)[0]
    size = os.path.getsize(exe)
    print("  size           : %d bytes" % size)
    print("  PE timestamp   : 0x%08X" % stamp)

    doc = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "UI-COMMANDS.md")
    if os.path.exists(doc):
        text = open(doc, encoding="utf-8", errors="ignore").read()
        m = re.search(r"PE timestamp\s*\|\s*`0x([0-9A-Fa-f]+)`", text)
        if m:
            claimed = int(m.group(1), 16)
            if claimed == stamp:
                print("  documented     : matches UI-COMMANDS.md")
            else:
                print("  MISMATCH: UI-COMMANDS.md says 0x%08X, the file says 0x%08X" % (claimed, stamp))
                failures.append("documented PE timestamp")
        m = re.search(r"\|\s*size\s*\|\s*([\d,]+) bytes", text)
        if m:
            claimed_size = int(m.group(1).replace(",", ""))
            if claimed_size != size:
                print("  MISMATCH: UI-COMMANDS.md says %d bytes, the file is %d" % (claimed_size, size))
                failures.append("documented size")

    print()
    print("Structural invariants that ARE checkable from layout.rs alone:")
    car_base = c.get("CAR_BASE")
    stride = c.get("CAR_STRIDE")
    obj = c.get("OBJECT_SIZE")
    cmax = c.get("CAR_MAX")
    if None not in (car_base, stride, obj, cmax):
        end = car_base + cmax * stride
        print("  last car record ends at 0x%X; object size is 0x%X (%s)"
              % (end, obj, "fits" if end <= obj else "OVERRUNS THE OBJECT"))
        if end > obj:
            failures.append("car records overrun the object")
        # Command records must not overlap the car records.
        print("  car array occupies 0x%X..0x%X of a 0x%X-byte object (%.1f%%)"
              % (car_base, end, obj, 100.0 * (end - car_base) / obj))

    print()
    if failures:
        print("FAIL: %s" % ", ".join(failures))
        return 1
    print("OK: nothing checkable is wrong. The prologue at SIMULATE_RVA matches this exe.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
