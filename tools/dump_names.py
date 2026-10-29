"""Dump the FName table from F1 Manager 24's Mappings.usmap.

Format (no magic, UE4SS output): u32 count, then count x (u8 len, bytes).
Used to find class / property / UFunction names without a full usmap parser.
"""
import sys

d = open(sys.argv[1], 'rb').read()
p = 0
n = int.from_bytes(d[0:4], 'little')
p = 4
names = []
for _ in range(n):
    ln = d[p]
    p += 1
    names.append(d[p:p + ln].decode('latin1'))
    p += ln

out = sys.argv[2] if len(sys.argv) > 2 else None
if out:
    with open(out, 'w', encoding='utf-8') as f:
        for i, s in enumerate(names):
            f.write('%6d %s\n' % (i, s))
    print('wrote %d names to %s' % (len(names), out))
print('data after names starts at 0x%X of 0x%X' % (p, len(d)))
