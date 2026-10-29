"""Find damage fields by watching which bytes in a car's record change during a race.

The design assumption to test: damage is *not* a known offset. The hijack notes claim it lives only in
`RaceSimCarIncident` / `RaceSimCarMechanicalIssues`, reachable through the event bus - which would make
field-level damage sync impossible without reflection work.

But a damage model has to be *stored* somewhere for the sim to read each step, and the car record is a
flat 0x10D8-byte struct. So rather than trust the claim, this scans the whole record and reports which
offsets actually move during a live race, then narrows to the ones that look like damage rather than
kinematics.

How to tell damage from motion:
  * kinematic fields (dist/lat/head/spd/pos) change continuously, every frame;
  * wear and damage change *slowly and monotonically*, and most stay exactly constant for long stretches;
  * a damage fraction normally lives in 0..1, a temperature in tens-to-hundreds, a boolean in {0,1}.

So the useful output is three classes: "changes every sample" (ignore), "changed once or twice during
the window" (candidate: accumulating wear/damage), "never changed" (constant, or untouched).

Read-only. Run it while a race is live.

Usage:
    python tools/scan_damage.py <pid> <rsm-hex> [seconds] [car] [outfile]
"""

import ctypes
import ctypes.wintypes as w
import struct
import sys
import time

PROCESS_VM_READ = 0x0010
PROCESS_QUERY_INFORMATION = 0x0400

CAR_BASE = 0x95BC00
CAR_STRIDE = 0x10D8

# Fields we already replicate, so they can be excluded from the report.
KNOWN = {
    0x170: 'lat', 0x184: 'head', 0x18C: 'splt', 0x190: 'seg', 0x194: 'dist',
    0x198: 'spd', 0x19C: 'acc', 0x1AC: 'stuck', 0x1E0: 'pos0', 0x1E8: 'pos1',
    0x1F0: 'pos2', 0x1FC: 'node', 0x7E0: 'race_pos', 0x7E4: 'lap', 0x7E8: 'lap_time',
    0x7F1: 'down', 0x7F3: 'done', 0x870: 'drs', 0x8A8: 'pit', 0x8A9: 'pit_req',
    0x904: 'stay_out', 0xEFA: 'lap_type', 0x778: 'fuel', 0x924: 'tyre_lo', 0x928: 'tyre_hi',
    0x97C: 'tyre0', 0x988: 'tyre1', 0x994: 'tyre2', 0x9A0: 'tyre3',
}

k32 = ctypes.windll.kernel32
k32.OpenProcess.restype = w.HANDLE
k32.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
k32.ReadProcessMemory.argtypes = [w.HANDLE, ctypes.c_void_p, ctypes.c_void_p,
                                  ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)]
k32.CloseHandle.argtypes = [w.HANDLE]


def read(h, addr, size):
    buf = ctypes.create_string_buffer(size)
    got = ctypes.c_size_t(0)
    if not k32.ReadProcessMemory(h, ctypes.c_void_p(addr), buf, size, ctypes.byref(got)):
        return None
    if got.value != size:
        return None
    return buf.raw


def classify(raw):
    """Best guess at a 4-byte group's type, purely for making the report readable."""
    f = struct.unpack('<f', raw)[0]
    i = struct.unpack('<i', raw)[0]
    if f == f and abs(f) < 1e6 and abs(f) > 0:
        if 0.0 <= f <= 1.5:
            return 'f32[0..1]'
        if -100.0 <= f <= 500.0:
            return 'f32'
    if raw in (b'\x00\x00\x00\x00',):
        return 'zero'
    if 0 <= i <= 4:
        return 'small-int'
    return 'other'


def main():
    pid = int(sys.argv[1])
    rsm = int(sys.argv[2], 16)
    seconds = float(sys.argv[3]) if len(sys.argv) > 3 else 30.0
    car = int(sys.argv[4]) if len(sys.argv) > 4 else 0
    outfile = sys.argv[5] if len(sys.argv) > 5 else 'damage-scan.txt'

    h = k32.OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, False, pid)
    if not h:
        raise SystemExit('cannot open pid %d' % pid)

    base = rsm + CAR_BASE + car * CAR_STRIDE
    print('scanning car %d at 0x%X for %.0fs, every 4 bytes of 0x%X' % (car, base, seconds, CAR_STRIDE))

    first = read(h, base, CAR_STRIDE)
    if first is None:
        k32.CloseHandle(h)
        raise SystemExit('car record unreadable - is this the live sim?')

    # Per 4-byte slot: how many distinct values, and the first/last seen.
    n_slots = CAR_STRIDE // 4
    prev = [first[i * 4:(i + 1) * 4] for i in range(n_slots)]
    changes = [0] * n_slots
    firstval = list(prev)
    lastval = list(prev)

    deadline = time.time() + seconds
    samples = 1
    while time.time() < deadline:
        time.sleep(0.25)
        cur = read(h, base, CAR_STRIDE)
        if cur is None:
            break
        samples += 1
        for i in range(n_slots):
            v = cur[i * 4:(i + 1) * 4]
            if v != prev[i]:
                changes[i] += 1
                lastval[i] = v
                prev[i] = v
    k32.CloseHandle(h)

    print('%d samples' % samples)

    rows = []
    for i in range(n_slots):
        off = i * 4
        if changes[i] == 0:
            continue
        label = KNOWN.get(off, '')
        rate = changes[i] / max(samples - 1, 1)
        rows.append((off, rate, firstval[i], lastval[i], label))

    rows.sort(key=lambda r: r[1])

    with open(outfile, 'w', encoding='utf-8') as f:
        f.write('# car %d, %d samples over %.0fs\n' % (car, samples, seconds))
        f.write('# off    rate    first            last             type         known\n')
        for off, rate, a, b, label in rows:
            f.write('+0x%04X  %.2f  %-16s %-16s %-12s %s\n' % (
                off, rate,
                struct.unpack('<f', a)[0] if classify(a).startswith('f32') else a.hex(),
                struct.unpack('<f', b)[0] if classify(b).startswith('f32') else b.hex(),
                classify(b), label))

    print()
    print('SLOW-CHANGING offsets (candidates for wear/damage) - changed in under 20%% of samples:')
    shown = 0
    for off, rate, a, b, label in rows:
        if rate < 0.20 and off not in KNOWN:
            fa = struct.unpack('<f', a)[0]
            fb = struct.unpack('<f', b)[0]
            print('  +0x%04X  rate %.2f  %-14s -> %-14s %s %s' % (
                off, rate, ('%.4f' % fa) if classify(a).startswith('f32') else a.hex(),
                ('%.4f' % fb) if classify(b).startswith('f32') else b.hex(),
                classify(b), ('(%s)' % label) if label else ''))
            shown += 1
            if shown >= 40:
                break
    print()
    print('%d slow-changing slots written to %s' % (shown, outfile))


if __name__ == '__main__':
    main()
