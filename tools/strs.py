import sys, mmap, re

path = sys.argv[1]
needle = sys.argv[2] if len(sys.argv) > 2 else None
minlen = int(sys.argv[3]) if len(sys.argv) > 3 else 5

with open(path, 'rb') as f:
    data = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
    if needle:
        pat = needle.encode()
        start = 0
        hits = 0
        while True:
            i = data.find(pat, start)
            if i < 0:
                break
            a = i
            while a > 0 and 32 <= data[a-1] < 127:
                a -= 1
            b = i
            while b < len(data) and 32 <= data[b] < 127:
                b += 1
            print('0x%X  %s' % (a, data[a:b].decode('latin1')[:200]))
            hits += 1
            if hits > 400:
                print('... truncated')
                break
            start = b
        print('total:', hits)
    else:
        # print all ascii runs >= minlen
        run = re.compile(rb'[\x20-\x7e]{%d,}' % minlen)
        n = 0
        for m in run.finditer(data):
            print('%s' % m.group().decode('latin1'))
            n += 1
            if n > 400:
                break
    data.close()
