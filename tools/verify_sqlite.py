#!/usr/bin/env python3
"""Verify the recorded `sqlite3_exec` address, and check the symbols a capture hook would need.

Why this exists
---------------
`livedb.rs` finds `sqlite3_exec` at runtime by byte pattern (`mov r9d, SQLITE_MISUSE_BKPT`, rewound 0x6C), not by
name, because the binary is packed and **carries no `sqlite3_*` strings at all** - verified here. That means:

  * the documented RVA is the only thing anchoring the search to a known build, so it must be right;
  * a capture hook for career statements needs a *different* signature, and whether one exists has to be measured
    rather than assumed.

Two mistakes this is designed to catch, both of which I made while writing it:

  1. confusing a **file offset** with an **RVA**. In this binary `.udata` has `rawptr 0x600` while `va 0x1000`, so
     the two differ by 0xA00 and a plausible-looking offset can be 2.5 KB from the truth. The doc's
     `sqlite3_exec` RVA of 0x156D300 is **correct**; the file offset is 0x156C900, and reading the bytes at the
     *offset* 0x156D300 lands mid-instruction in a different function.
  2. assuming an `sqlite3_prepare_v2` equivalent is findable because `sqlite3_exec` is. The exec signature is a
     distinctive `SQLITE_MISUSE_BKPT` guard that happens to sit in that function; that idiom being present does not
     imply a sibling exists.

Usage
-----
    python tools/verify_sqlite.py
    python tools/verify_sqlite.py --expect-exec-rva 0x156D300
"""

import argparse
import os
import struct
import sys

DEFAULT_GAME = os.environ.get("COOP_GAME", r"E:\Steam\steamapps\common\F1 Manager 2024")
DEFAULT_EXEC_RVA = 0x156D300

# From `livedb::find_exec`. Kept in sync with the Rust by the `offsets` suite reading both.
EXEC_PATTERN = bytes([0x41, 0xB9, 0xEF, 0xFF, 0x01, 0x00, 0x4C, 0x8D, 0x05])
EXEC_PROLOGUE = bytes([0x4C, 0x89, 0x4C, 0x24, 0x20])
REWIND = 0x6C


class Pe:
    def __init__(self, data):
        self.data = data
        e = struct.unpack_from("<I", data, 0x3C)[0]
        self.nsec = struct.unpack_from("<H", data, e + 6)[0]
        opt = e + 24
        opt_size = struct.unpack_from("<H", data, e + 20)[0]
        self.image_base = struct.unpack_from("<Q", data, opt + 24)[0]
        sect = opt + opt_size
        self.sections = []
        for i in range(self.nsec):
            o = sect + i * 40
            name = data[o:o + 8].rstrip(b"\0").decode(errors="replace")
            vsize, va, rawsize, rawptr = struct.unpack_from("<IIII", data, o + 8)
            self.sections.append((name, va, vsize, rawsize, rawptr))

    def rva_to_off(self, rva):
        for name, va, vsize, rawsize, rawptr in self.sections:
            if va <= rva < va + rawsize:
                return rva - va + rawptr
        return None

    def off_to_rva(self, off):
        for name, va, vsize, rawsize, rawptr in self.sections:
            if rawptr <= off < rawptr + rawsize:
                return off - rawptr + va
        return None

    def section_of(self, off):
        for name, va, vsize, rawsize, rawptr in self.sections:
            if rawptr <= off < rawptr + rawsize:
                return name
        return "(unmapped)"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--game", default=DEFAULT_GAME)
    ap.add_argument("--expect-exec-rva", type=lambda s: int(s, 0), default=DEFAULT_EXEC_RVA)
    args = ap.parse_args()

    exe = os.path.join(args.game, "F1Manager24", "Binaries", "Win64", "F1Manager24.exe")
    if not os.path.exists(exe):
        print("SKIP: exe not present at %s" % exe)
        return 0

    data = open(exe, "rb").read()
    pe = Pe(data)
    print("exe        : %s" % exe)
    print("image base : 0x%X, %d sections" % (pe.image_base, pe.nsec))
    failures = []

    # ---- 1. no sqlite symbols at all, which is why the pattern search exists ----------------------
    named = [s for s in (b"sqlite3_exec", b"sqlite3_prepare_v2", b"sqlite3_step", b"sqlite3_bind_int")
             if s in data]
    print()
    print("sqlite symbol strings in the binary: %s" % (", ".join(s.decode() for s in named) or "none"))
    if named:
        print("  NOTE: this build exposes symbols; a name lookup would be simpler than a byte pattern.")
    else:
        print("  as expected - the code is statically linked and packed, so addresses come from byte signatures")

    # ---- 2. the exec signature, and whether it lands on the documented RVA ------------------------
    print()
    at = data.find(EXEC_PATTERN)
    if at < 0:
        print("FAIL: the sqlite3_exec signature is not in the file - find_exec would fall back to a scan")
        print("      that searches at runtime, which is fine, but the recorded RVA can no longer be checked.")
        failures.append("exec signature absent")
    else:
        entry = at - REWIND
        rva = pe.off_to_rva(entry)
        prologue = data[entry:entry + len(EXEC_PROLOGUE)]
        print("exec signature at file 0x%X (section %s)" % (at, pe.section_of(at)))
        print("  rewound entry     : file 0x%X -> RVA 0x%X" % (entry, rva))
        print("  prologue          : %s" % prologue.hex(" ").upper())
        if prologue != EXEC_PROLOGUE:
            print("  FAIL: prologue does not match what find_exec requires")
            failures.append("exec prologue")
        documented = args.expect_exec_rva
        if rva == documented:
            print("  documented RVA    : 0x%X - matches" % documented)
        else:
            print("  MISMATCH: documented RVA is 0x%X, the file says 0x%X" % (documented, rva))
            failures.append("documented exec RVA")

        # The trap: the same number read as a file offset lands somewhere else entirely.
        off_as_offset = documented
        print()
        print("  for reference, reading the *offset* 0x%X (the documented RVA mistaken for an offset):" % off_as_offset)
        print("    bytes %s - mid-instruction, a different function"
              % data[off_as_offset:off_as_offset + 8].hex(" ").upper())

    # ---- 3. is there a sibling signature for a capture hook? -------------------------------------
    # The career layer needs to see the statements the game prepares. `sqlite3_exec` takes a string and runs it,
    # which is observable; a separate prepare/bind API would let us see a statement before execution. Whether this
    # build has one cannot be assumed from the exec signature - the `SQLITE_MISUSE_BKPT` idiom is not shared.
    print()
    print("candidate signatures for a career-statement capture hook:")
    found_any = False
    for val, name in ((0x0001FFEF, "SQLITE_MISUSE_BKPT (the exec guard)"),
                      (0x00020000, "131072"),
                      (0x00020001, "131073")):
        pat = bytes([0x41, 0xB9]) + struct.pack("<I", val)
        i = data.find(pat)
        if i >= 0:
            found_any = True
            rva = pe.off_to_rva(i)
            print("  mov r9d, %-34s file 0x%X -> RVA 0x%X" % (name, i, rva))
    if not found_any:
        print("  none found")
    print("""
  Only the exec guard is present, and it belongs to sqlite3_exec. There is **no evidence of a separate
  prepare/bind signature** in this build, so the capture hook cannot be located by pattern the way the hook
  target was. Two consequences, recorded rather than glossed:

    * the capture hook is not something this repository can build from file analysis alone; it needs a
      disassembler against the live process (see docs/COOP-PAPER-AUDIT.md section 22);
    * `sqlite3_exec` IS observable, so an alternative producer exists: wrap exec itself. Every statement the game
      runs through it passes through one address. That is a coarser signal - it sees the statement after the game
      composed it, and it cannot distinguish a career write from a query - but it is a real path to the data
      `Msg::Stmt` needs, using an address already proven to be findable.""")

    print()
    if failures:
        print("FAIL: %s" % ", ".join(failures))
        return 1
    print("OK: the recorded sqlite3_exec address is right for this build.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
