"""Stand-in for coop_agent.dll: accept car frames and apply them to a fake sim object.

This is the only way to test the transport without a running game, and it exercises the real wire
format and the same "apply (offset, value) pairs" semantics `hookmail` uses on the game thread. If this
passes, the `coop-link` -> agent half of the pipeline is sound and any remaining failure is in the hook
(fires) rather than the plumbing.

Wire format, read out of coop-proto rather than guessed:

    magic    4 bytes   MAGIC
    version  u16 LE
    length   u32 LE    payload length
    checksum u32 LE    FNV-1a 32 over the payload
    payload  ...       tag u8, then the message

    Snapshot payload: tag=2, tick u32, race_time f32, count u8, then count x Car
    Car: id u8, dist f32, lat f32, head f32, spd f32, acc f32, splt f32, seg f32, node i32,
         pos u8, lap i32, fuel f32, lap_time f32, down u8, done u8, drs i32, pit u8, pit_req u8,
         stay_out u8, lap_type u8, tyre_lo f32, tyre_hi f32, 4 x (surface, carcass, condition) f32

Usage:
    python tools/fake_agent.py [port] [seconds]
"""

import os
import re
import socket
import struct
import sys

CAR_BASE = 0x95BC00
CAR_STRIDE = 0x10D8
OFF_LAT, OFF_HEAD, OFF_SPD, OFF_DIST = 0x170, 0x184, 0x198, 0x194

HEADER_LEN = 14
TAG_SNAPSHOT = 2
MAGIC = b'COOP'


def car_size():
    """Bytes per Car on the wire, computed from the field list rather than typed by hand."""
    return (1                    # id
            + 4 * 7              # dist lat head spd acc splt seg
            + 4                  # node i32
            + 1                  # pos
            + 4                  # lap i32
            + 4 + 4              # fuel lap_time
            + 1 + 1              # down done
            + 4                  # drs i32
            + 1 + 1 + 1 + 1      # pit pit_req stay_out lap_type
            + 4 + 4              # tyre_lo tyre_hi
            + 12 * 4)            # 4 tyres x 3 floats


def fnv1a32(data):
    h = 0x811C9DC5
    for b in data:
        h ^= b
        h = (h * 0x01000193) & 0xFFFFFFFF
    return h


def protocol_version():
    """Read the version constant from coop-proto rather than assuming it."""
    src = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'coop-rs',
                       'crates', 'coop-proto', 'src', 'lib.rs')
    try:
        with open(src, encoding='utf-8') as f:
            m = re.search(r'const\s+VERSION\s*:\s*u16\s*=\s*(\d+)', f.read())
        if m:
            return int(m.group(1))
    except OSError:
        pass
    return 1


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 45991
    seconds = float(sys.argv[2]) if len(sys.argv) > 2 else 15.0
    version = protocol_version()
    csz = car_size()

    memory = bytearray(CAR_BASE + 22 * CAR_STRIDE)
    stats = {'frames': 0, 'writes': 0, 'bad': 0, 'cars': set()}

    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(('127.0.0.1', port))
    srv.listen(1)
    print('fake agent on 127.0.0.1:%d (protocol v%d), %d bytes/car, %.1fs'
          % (port, version, csz, seconds))

    def serve(conn):
        buf = b''
        while True:
            try:
                chunk = conn.recv(65536)
            except OSError:
                break
            if not chunk:
                break
            buf += chunk
            while True:
                if len(buf) < HEADER_LEN:
                    break
                if buf[:4] != MAGIC:
                    stats['bad'] += 1
                    buf = buf[4:]
                    continue
                ver = struct.unpack_from('<H', buf, 4)[0]
                plen, chk = struct.unpack_from('<II', buf, 6)
                total = HEADER_LEN + plen
                if len(buf) < total:
                    break
                payload = buf[HEADER_LEN:total]
                buf = buf[total:]
                if ver != version or fnv1a32(payload) != chk:
                    stats['bad'] += 1
                    continue
                if not payload or payload[0] != TAG_SNAPSHOT:
                    continue
                n = payload[9]
                off = 10
                applied = 0
                for _ in range(n):
                    if off + csz > len(payload):
                        break
                    car_id = payload[off]
                    dist, lat, head, spd = struct.unpack_from('<ffff', payload, off + 1)
                    base = CAR_BASE + car_id * CAR_STRIDE
                    if base + CAR_STRIDE <= len(memory):
                        for field, value in ((OFF_DIST, dist), (OFF_LAT, lat),
                                             (OFF_HEAD, head), (OFF_SPD, spd)):
                            struct.pack_into('<f', memory, base + field, value)
                            applied += 1
                    stats['cars'].add(car_id)
                    off += csz
                stats['frames'] += 1
                stats['writes'] += applied
        conn.close()

    srv.settimeout(seconds)
    try:
        conn, addr = srv.accept()
        print('client connected from %s:%d' % addr)
        srv.settimeout(None)
        serve(conn)
    except socket.timeout:
        pass
    srv.close()

    print()
    print('frames decoded : %d' % stats['frames'])
    print('field writes   : %d' % stats['writes'])
    print('bad frames     : %d' % stats['bad'])
    print('distinct cars  : %d' % len(stats['cars']))

    nonzero = sum(1 for car in range(22)
                  if abs(struct.unpack_from('<f', memory,
                                            CAR_BASE + car * CAR_STRIDE + OFF_DIST)[0]) > 0.001)
    print('cars positioned in the fake object: %d of 22' % nonzero)

    ok = (stats['frames'] > 0 and stats['writes'] > 0 and stats['bad'] == 0
          and nonzero > 0 and len(stats['cars']) == 22)
    print('\nRESULT: %s' % ('PASS - frames arrived, checksums held, all 22 cars written'
                            if ok else 'FAIL - see the counters above'))
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
