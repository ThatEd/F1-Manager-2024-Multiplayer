import sys

p = sys.argv[1]
offs = [int(x, 16) for x in sys.argv[2:]]
f = open(p, 'rb')
d = f.read()
for off in offs:
    a = max(0, off - 160)
    b = min(len(d), off + 80)
    print('=== 0x%X ===' % off)
    print(d[a:b].decode('latin1').replace('\x00', '|'))
    print()
