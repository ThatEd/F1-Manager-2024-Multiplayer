"""Sample race-control and pit state continuously, to map red flag / restart transitions.

Written because the reported bug is: "the entire grid pitted on host and on client they did not", and
"cars leaving the pits on red flag from a restart not being held".

Two questions, both answerable from read-only sampling:

1. What do `flags` / `rc_state` / `rf_state` actually do across a red flag and a restart? The `StartHold`
   keys on `flags & 0x100` (WaitingForLightsOut), which is the *initial* start. A red-flag restart is a
   different state, and guessing its bit is how previous sessions were lost.

2. Do the pit fields (`PIT 0x8A8`, `PIT_REQ 0x8A9`, `STAY_OUT 0x904`) change on the host when the grid
   pits? They are not in `hookmail::FIELDS`, so if they move during a pit transition the client is being
   told positions without being told the cars are in the pit lane.

Prints a line whenever anything changes, so a 20-minute race produces a compact transition log rather
than a wall of samples. Read-only.

Usage:
    python tools/watch_rc.py <pid> <rsm-hex> [seconds] [outfile]
"""

import ctypes
import ctypes.wintypes as w
import struct
import sys
import time

PROCESS_VM_READ = 0x0010
PROCESS_QUERY_INFORMATION = 0x0400

FLAGS = 0x24C
RC_STATE = 0xA045C0
RF_STATE = 0xA13289
CAR_BASE = 0x95BC00
CAR_STRIDE = 0x10D8
PIT = 0x8A8
PIT_REQ = 0x8A9
STAY_OUT = 0x904
DIST = 0x194
GARAGE_PENDING = 0x46

k32 = ctypes.windll.kernel32
k32.OpenProcess.restype = w.HANDLE
k32.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
k32.ReadProcessMemory.argtypes = [w.HANDLE, ctypes.c_void_p, ctypes.c_void_p,
                                  ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)]
k32.CloseHandle.argtypes = [w.HANDLE]

# Flag bits we believe we know, from coop-proto::layout and RACESIM-HIJACK. Anything else is reported
# numerically so an unmapped bit shows up as "unmapped" instead of being silently ignored.
KNOWN_FLAGS = {
    0x100: 'WAITING_FOR_LIGHTS_OUT',
    0x400: 'DRS_ENABLED',
    0x200: 'RED?',          # hypothesis only, not established
    0x800: '?0x800',
    0x1000: '?0x1000',
}


def read(h, addr, size):
    buf = ctypes.create_string_buffer(size)
    got = ctypes.c_size_t(0)
    if not k32.ReadProcessMemory(h, ctypes.c_void_p(addr), buf, size, ctypes.byref(got)):
        return None
    return buf.raw[:got.value] if got.value == size else None


def u32(h, addr):
    b = read(h, addr, 4)
    return None if b is None else struct.unpack('<I', b)[0]


def f32(h, addr):
    b = read(h, addr, 4)
    return None if b is None else struct.unpack('<f', b)[0]


def u8(h, addr):
    b = read(h, addr, 1)
    return None if b is None else b[0]


def describe_flags(v):
    if v is None:
        return 'n/a'
    if v == 0:
        return '0x0 (none)'
    named = [name for bit, name in KNOWN_FLAGS.items() if v & bit]
    leftover = v & ~sum(KNOWN_FLAGS)
    extra = (' | unmapped 0x%X' % leftover) if leftover else ''
    return '0x%X (%s)%s' % (v, ', '.join(named) if named else 'no known bit', extra)


def snapshot(h, rsm):
    """Everything we track, as a comparable tuple."""
    flags = u32(h, rsm + FLAGS)
    rc = u8(h, rsm + RC_STATE)
    rf = u8(h, rsm + RF_STATE)
    pits = 0
    pit_reqs = 0
    stay_outs = 0
    moving = 0
    maxd = 0.0
    for car in range(22):
        base = rsm + CAR_BASE + car * CAR_STRIDE
        p = u8(h, base + PIT)
        pr = u8(h, base + PIT_REQ)
        so = u8(h, base + STAY_OUT)
        d = f32(h, base + DIST)
        if p:
            pits += 1
        if pr:
            pit_reqs += 1
        if so:
            stay_outs += 1
        if d is not None:
            if abs(d) > 1.0:
                moving += 1
            maxd = max(maxd, abs(d))
    return (flags, rc, rf, pits, pit_reqs, stay_outs, moving), maxd


def main():
    pid = int(sys.argv[1])
    rsm = int(sys.argv[2], 16)
    seconds = float(sys.argv[3]) if len(sys.argv) > 3 else 600.0
    outfile = sys.argv[4] if len(sys.argv) > 4 else 'rc-watch.txt'

    h = k32.OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, False, pid)
    if not h:
        raise SystemExit('cannot open pid %d' % pid)

    out = open(outfile, 'w', encoding='utf-8')
    start = time.time()
    last = None
    print('watching pid %d rsm 0x%X for %.0fs -> %s' % (pid, rsm, seconds, outfile))

    header = ('  t     flags                        rc  rf  pits preqs stay moving  max_dist')
    print(header)
    out.write(header + '\n')

    while time.time() - start < seconds:
        snap, maxd = snapshot(h, rsm)
        if snap != last:
            t = time.time() - start
            flags, rc, rf, pits, preqs, so, moving = snap
            line = '%6.1f  %-28s %-3s %-3s %-4d %-4d %-4d %-6d  %.1f' % (
                t, describe_flags(flags), rc, rf, pits, preqs, so, moving, maxd)
            print(line)
            out.write(line + '\n')
            out.flush()
            last = snap
        time.sleep(0.2)

    out.close()
    k32.CloseHandle(h)
    print('\ndone -> %s' % outfile)


if __name__ == '__main__':
    main()
