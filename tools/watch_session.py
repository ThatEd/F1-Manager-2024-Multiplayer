"""Watch the host game during a session start and record what actually happens.

Purpose: a sprint start is a few seconds long and involves four things that must all line up -
the hook firing, the sim stepping, the publisher locking on, and the client applying. Watching that by
eye is impossible, and afterwards "it did not sync" is not evidence. This samples the host side twice a
second and writes a timeline, so the start can be read back.

Host side only. The client's evidence is in its own `coop_agent.log` and `client-run.log`.

Usage:
    python tools/watch_session.py <pid> [seconds] [logfile]

Read-only: finds the hook block by signature and reads car distances. Writes nothing to the game.
"""

import ctypes
import ctypes.wintypes as w
import sys
import time

PROCESS_VM_READ = 0x0010
PROCESS_QUERY_INFORMATION = 0x0400
MEM_COMMIT = 0x1000
PAGE_GUARD = 0x100
PAGE_NOACCESS = 0x01

CAR_BASE = 0x95BC00
CAR_STRIDE = 0x10D8
OFF_DIST = 0x194
SIM_TIME = 0x99AF1C
RC_STATE = 0xA045C0

k32 = ctypes.windll.kernel32
k32.OpenProcess.restype = w.HANDLE
k32.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
k32.ReadProcessMemory.argtypes = [w.HANDLE, ctypes.c_void_p, ctypes.c_void_p,
                                  ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)]
k32.VirtualQueryEx.argtypes = [w.HANDLE, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t]
k32.CloseHandle.argtypes = [w.HANDLE]

LAYOUTS = [(0x90, 0x98, 0xA0), (0x40, 0x48, 0x50)]


class MBI(ctypes.Structure):
    _fields_ = [('BaseAddress', ctypes.c_void_p), ('AllocationBase', ctypes.c_void_p),
                ('AllocationProtect', w.DWORD), ('PartitionId', w.WORD),
                ('_pad', w.WORD), ('RegionSize', ctypes.c_size_t), ('State', w.DWORD),
                ('Protect', w.DWORD), ('Type', w.DWORD), ('_pad2', w.DWORD)]


def read(h, addr, size):
    buf = ctypes.create_string_buffer(size)
    got = ctypes.c_size_t(0)
    if not k32.ReadProcessMemory(h, ctypes.c_void_p(addr), buf, size, ctypes.byref(got)):
        return None
    if got.value != size:
        return None
    return buf.raw


def ru64(h, addr):
    b = read(h, addr, 8)
    return None if b is None else int.from_bytes(b, 'little')


def rf32(h, addr):
    b = read(h, addr, 4)
    return None if b is None else struct_unpack_f(b)


def struct_unpack_f(b):
    import struct
    return struct.unpack('<f', b)[0]


def regions(h):
    addr = 0
    limit = 0x7FFFFFFF0000
    while addr < limit:
        mbi = MBI()
        if not k32.VirtualQueryEx(h, ctypes.c_void_p(addr), ctypes.byref(mbi), ctypes.sizeof(mbi)):
            break
        size = mbi.RegionSize or 0x1000
        if (mbi.State == MEM_COMMIT and not (mbi.Protect & (PAGE_GUARD | PAGE_NOACCESS))
                and mbi.Protect != 0):
            yield int(mbi.BaseAddress or addr), size
        addr = int(mbi.BaseAddress or addr) + size


def find_stub(h):
    for layout in LAYOUTS:
        slot_off, fires_off, _cb = layout
        d1 = (slot_off - 7).to_bytes(4, 'little')
        d2 = (fires_off - 0x0E).to_bytes(4, 'little')
        sig = b'\x48\x89\x0d' + d1 + b'\x48\xff\x05' + d2
        for base, size in regions(h):
            if size > 0x100000:
                continue
            data = read(h, base, size)
            if not data:
                continue
            i = data.find(sig)
            if i >= 0:
                return base + i, layout
    return None, None


def sim_candidates(h, rsm):
    """Measure the given sim plus its sibling CDO, so 'no motion' can be told from 'wrong object'."""
    out = []
    for addr in (rsm, rsm ^ 0x20200000, rsm + 0x20200000, rsm - 0x20200000):
        if addr <= 0 or addr in out:
            continue
        d = rf32(h, addr + CAR_BASE + OFF_DIST)
        if d is not None:
            out.append(addr)
    return out


def main():
    pid = int(sys.argv[1]) if len(sys.argv) > 1 else None
    seconds = float(sys.argv[2]) if len(sys.argv) > 2 else 120.0
    logfile = sys.argv[3] if len(sys.argv) > 3 else 'session-watch.log'
    if pid is None:
        raise SystemExit('usage: watch_session.py <pid> [seconds] [logfile]')

    h = k32.OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, False, pid)
    if not h:
        raise SystemExit('cannot open pid %d' % pid)

    block, layout = find_stub(h)
    if block is None:
        k32.CloseHandle(h)
        raise SystemExit('hook stub not found in pid %d - is the agent injected?' % pid)
    _, fires_off, cb_off = layout

    # The sim address the publisher locked onto, from the endpoint file it writes.
    rsm = 0
    try:
        with open('coop_rsm.txt', 'r') as f:
            parts = f.read().split()
            if len(parts) >= 2:
                rsm = int(parts[1], 16)
    except (OSError, ValueError):
        pass

    log = open(logfile, 'w', encoding='utf-8')
    print('watching pid %d for %.0fs -> %s' % (pid, seconds, logfile))
    print('block 0x%X  rsm %s' % (block, ('0x%X' % rsm) if rsm else 'unknown'))

    start = time.time()
    last_fires = ru64(h, block + fires_off) or 0
    last_state = None
    zero_since = None

    while time.time() - start < seconds:
        now = time.time() - start
        fires = ru64(h, block + fires_off)
        if fires is None:
            log.write('%6.1f GONE  process exited\n' % now)
            break

        maxd = None
        simt = None
        rc = None
        if rsm:
            maxd = 0.0
            for car in range(22):
                d = rf32(h, rsm + CAR_BASE + car * CAR_STRIDE + OFF_DIST)
                if d is None:
                    maxd = None
                    break
                if d == d:  # ignore NaN
                    maxd = max(maxd, abs(d))
            simt = rf32(h, rsm + SIM_TIME)
            b = read(h, rsm + RC_STATE, 1)
            rc = b[0] if b else None

        moving = maxd is not None and maxd > 1.0
        if moving:
            zero_since = None
        elif zero_since is None:
            zero_since = now

        state = (fires - last_fires) > 0, moving, rc
        # Write a line on any change, and a heartbeat every 5 s, so the file stays readable.
        if state != last_state or int(now) % 5 == 0:
            log.write('%6.1f fires=%-8d (+%-4d) max_dist=%-9s simtime=%-7s rc=%-4s %s\n' % (
                now, fires, fires - last_fires,
                ('%.1f' % maxd) if maxd is not None else 'n/a',
                ('%.2f' % simt) if simt is not None else 'n/a',
                rc if rc is not None else 'n/a',
                'MOVING' if moving else ('zero since %.1fs' % (now - zero_since)
                                         if zero_since is not None else '')))
            log.flush()
        last_state = state
        last_fires = fires
        time.sleep(0.25)

    log.write('\nSUMMARY: fires went %d -> %d\n' % (0, ru64(h, block + fires_off) or 0))
    log.close()
    k32.CloseHandle(h)
    print('done; timeline in %s' % logfile)


if __name__ == '__main__':
    main()
