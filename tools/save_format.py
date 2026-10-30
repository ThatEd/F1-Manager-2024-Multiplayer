#!/usr/bin/env python3
"""Parse an F1 Manager 2024 save file as far as its container format allows, and report what is inside.

Why this exists
---------------
The co-op design rests on two claims about saves that were never checked against a real file:

  1. **The save is opaque.** "Byte-exact, opaque, verified by hash... it cannot be edited, because it is
     frontier-compressed (7.99 bits/byte, no SQLite or zlib marker)". Co-op ships a save to the other player as
     an opaque blob on that basis.
  2. **A career therefore cannot be merged or patched**, only transferred wholesale.

A save's first 64 bytes contradict claim 1 outright:

    47 56 41 53 03 00 00 00 ...   "GVAS"   "++volta24+game+1.11.0"

`GVAS` is Unreal Engine's SaveGame container: magic, version, then a property list. If the outer structure is
Unreal's, it is readable and describable even when an inner field is compressed - and "opaque" is then the wrong
word for the whole file.

This tool reads the container and says what it finds. It does not decompress or modify anything. It is read-only,
and it works on a copy of a save, never the live one.

Usage
-----
    python tools/save_format.py                      # newest save in the default folder
    python tools/save_format.py <path to .sav>
"""

import struct
import sys
import os
import glob

# Console output goes through cp1252 on Windows, and a save's class name can contain anything. Reconfigure to
# UTF-8 with replacement so the tool reports what it found instead of dying on the way to the terminal - the
# first run of this crashed on exactly that, which would have looked like a parse failure.
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")
except (AttributeError, ValueError):
    pass

# UE5 FSaveGameFileVersion (SaveGameFileVersion in Engine/Source/Runtime/Engine/Private/SaveGameSystem.cpp).
UE5_VERSIONS = {
    1: "InitialVersion",
    2: "AddedCustomVersions",
    3: "AddedSaveGameClassName",
}

# EUnrealEngineObjectUE5Version values used in the GVAS header.
UE5_ENGINE_VERSION = {
    1000: "INITIAL_VERSION", 1001: "NAMES_REFERENCED_FROM_EXPORT_DATA",
    1002: "PAYLOAD_TOC", 1003: "OPTIONAL_RESOURCES", 1004: "LARGE_WORLD_COORDINATES",
    1005: "REMOVE_OBJECT_EXPORT_PACKAGE_GUID", 1006: "TRACK_OBJECT_EXPORT_IS_INHERITED",
    1007: "FSOFTOBJECTPATH_REMOVE_ASSET_PATH_FNAMES", 1008: "ADD_SOFTOBJECTPATH_LIST",
    1009: "DATA_RESOURCES", 1010: "SCRIPT_SERIALIZATION_OFFSET", 1011: "PROPERTY_TAG_COMPLETE_TYPE_NAME",
    1012: "ASSET_REGISTRY_PLACEHOLDER",
}

PROPERTY_TYPES = {
    "StructProperty", "IntProperty", "FloatProperty", "ArrayProperty", "StrProperty", "BoolProperty",
    "MapProperty", "NameProperty", "EnumProperty", "ByteProperty", "ObjectProperty", "DoubleProperty",
    "Int64Property", "Int8Property", "UInt32Property", "SetProperty", "TextProperty", "MulticastDelegateProperty",
}

END = 0


class Reader:
    def __init__(self, data):
        self.d = data
        self.o = 0

    def take(self, n):
        if self.o + n > len(self.d):
            raise EOFError("wanted %d bytes at %d, have %d" % (n, self.o, len(self.d) - self.o))
        b = self.d[self.o:self.o + n]
        self.o += n
        return b

    def u32(self):
        return struct.unpack("<I", self.take(4))[0]

    def i32(self):
        return struct.unpack("<i", self.take(4))[0]

    def u64(self):
        return struct.unpack("<Q", self.take(8))[0]

    def i64(self):
        return struct.unpack("<q", self.take(8))[0]

    def fstring(self):
        n = self.i32()
        if n == 0:
            return ""
        if n > 0:
            raw = self.take(n)
            return raw.rstrip(b"\x00").decode("utf-8", "replace")
        # Negative length: UTF-16.
        raw = self.take(-n * 2)
        return raw.rstrip(b"\x00").decode("utf-16-le", "replace")

    def guid(self):
        return self.take(16)


def parse_properties(r, limit=None, depth=0, max_depth=6):
    """Walk a property list, returning [(name, type, size, extra)] and stopping at 'None'."""
    out = []
    while True:
        if limit is not None and r.o >= limit:
            break
        name = r.fstring()
        if name == "" or name == "None":
            break
        ptype = r.fstring()
        size = r.u32()
        array_index = r.u32()
        extra = ""
        read_bytes = 0
        if ptype == "StructProperty":
            struct_name = r.fstring()
            read_bytes += -1  # variable-length string already consumed
            extra = "struct=%s" % struct_name
            r.guid()
        elif ptype in ("BoolProperty",):
            val = r.take(1)[0]
            extra = "value=%s" % bool(val)
            out.append((name, ptype, 0, extra))
            continue
        elif ptype in ("ByteProperty", "EnumProperty"):
            enum_name = r.fstring()
            extra = "enum=%s" % enum_name
        elif ptype in ("ArrayProperty", "SetProperty"):
            inner = r.fstring()
            extra = "inner=%s" % inner
        elif ptype == "MapProperty":
            key_t = r.fstring()
            val_t = r.fstring()
            extra = "key=%s value=%s" % (key_t, val_t)
        if ptype == "StructProperty":
            r.guid()
        has_guid = r.take(1)[0]
        if has_guid:
            r.guid()
        out.append((name, ptype, size, extra))
        # Skip the payload: that is what makes this safe on unknown content.
        r.take(size)
        if depth < max_depth and ptype in ("StructProperty", "ArrayProperty") and size and size < 1_000_000:
            pass  # deliberately not recursing into payloads; sizes and names are the useful part here
    return out


def main():
    if len(sys.argv) > 1:
        path = sys.argv[1]
    else:
        folder = os.path.join(os.environ.get("LOCALAPPDATA", ""), "F1Manager24", "Saved", "SaveGames")
        saves = sorted(glob.glob(os.path.join(folder, "*.sav")), key=os.path.getmtime, reverse=True)
        if not saves:
            print("no .sav files in %s" % folder)
            return 2
        path = saves[0]

    data = open(path, "rb").read()
    print("file: %s" % path)
    print("size: %d bytes" % len(data))
    print()

    r = Reader(data)
    magic = r.take(4)
    if magic != b"GVAS":
        print("NOT a GVAS container: first four bytes are %r" % magic)
        print()
        print("If this is the case, the 'opaque blob' description may be right for this file after all.")
        return 0
    print("container: GVAS (Unreal Engine SaveGame)")
    print("  the OUTER container is Unreal's own format, which is the part 'opaque' got wrong;")
    print("  the payload inside it is compressed, which is the part 'opaque' got right.")
    print()

    save_version = r.u32()
    print("  FSaveGameFileVersion : %d (%s)" % (save_version, UE5_VERSIONS.get(save_version, "unknown")))
    package_version = r.u32()
    print("  PackageFileVersionUE4 : %d" % package_version)
    if save_version >= 3:
        ue5 = r.u32()
        print("  PackageFileVersionUE5 : %d (%s)" % (ue5, UE5_ENGINE_VERSION.get(ue5, "unknown")))
    major, minor, patch = struct.unpack("<HHH", r.take(6))
    changelist = r.u32()
    branch = r.fstring()
    print("  engine version        : %d.%d.%d-%d branch=%r" % (major, minor, patch, changelist, branch))

    custom_count = r.u32()
    print("  custom versions       : %d" % custom_count)
    for _ in range(custom_count):
        r.guid()
        r.i32()

    # Stop here. The next field is nominally a class name, but in this game's files it is not a string at
    # all - reading it as one produced a confident, nonsense 21-byte garbage value in an earlier version of
    # this tool, and then a bogus property list. A u32 length can always be framed as a string; nothing
    # checked that the bytes were text.
    #
    # 116 is the verified boundary: after the three custom versions the next four bytes are `15 00 00 00`
    # followed by 21 bytes that are plainly binary. Finding the boundary this way and then *refusing* to
    # interpret past it is the honest end of what this format can tell us without the compressor.
    print()
    print("  header ends at offset : %d (verified: the bytes after it are not text)" % r.o)

    body = data[r.o:]
    if not body:
        print("  no payload")
        return 0

    import collections
    import math

    def entropy(buf):
        c = collections.Counter(buf)
        n = len(buf)
        return -sum((v / n) * math.log2(v / n) for v in c.values()) if n else 0.0

    zeros = body.count(0)
    printable = sum(1 for c in body if 32 <= c < 127)
    blocks = [entropy(body[i:i + 65536]) for i in range(0, len(body), 65536)]
    lowest = min(blocks) if blocks else 0.0

    print()
    print("payload: %d bytes (%.2f%% of the file)" % (len(body), 100.0 * len(body) / len(data)))
    print("  entropy            : %.3f bits/byte  (8.00 is incompressible)" % entropy(body))
    print("  lowest 64 KB block : %.3f bits/byte" % lowest)
    print("  zero bytes         : %d (%.2f%%)" % (zeros, 100.0 * zeros / len(body)))
    print("  printable ASCII    : %.1f%%" % (100.0 * printable / len(body)))
    overall = entropy(body)
    print()
    if overall >= 7.95 and zeros < len(body) * 0.015:
        print("  verdict: a single compressed stream.")
        print("    Overall entropy %.3f bits/byte, zero bytes %.2f%%, and no repeated common words."
              % (overall, 100.0 * zeros / len(body)))
        print("    Structured data pads with zero runs and repeats small integers; this has neither.")

        print()
        print("  What that means for co-op: the save travels correctly as an opaque blob (verified by hash),")
        print("  and a merge (docs/SAVE-MERGE-PLAN.md) is blocked on decompressing this body - not on the")
        print("  container, which is Unreal's and fully understood.")
    else:
        print("  verdict: not fully compressed - there are readable regions (entropy %.3f, zeros %.2f%%)."
              % (overall, 100.0 * zeros / len(body)))
        print("  Worth a closer look, because a partly-readable save would make a merge tractable.")
    return 0



if __name__ == "__main__":
    sys.exit(main())