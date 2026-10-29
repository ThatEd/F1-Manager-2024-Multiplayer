"""Fast RIP-relative xref finder (numpy, chunked) for large PE images.

  python fastxref.py <exe> <rva-hex> [<rva-hex> ...]
  python fastxref.py <exe> --str <text>
"""
import sys
import mmap
import struct
import numpy as np

IMAGE_SCN_MEM_EXECUTE = 0x20000000


class PE:
    def __init__(self, path):
        self.f = open(path, 'rb')
        self.mm = mmap.mmap(self.f.fileno(), 0, access=mmap.ACCESS_READ)
        d = self.mm
        e = struct.unpack_from('<I', d, 0x3C)[0]
        assert d[e:e + 4] == b'PE\0\0'
        coff = e + 4
        nsec = struct.unpack_from('<H', d, coff + 2)[0]
        optsz = struct.unpack_from('<H', d, coff + 16)[0]
        opt = coff + 20
        self.image_base = struct.unpack_from('<Q', d, opt + 24)[0]
        sect = opt + optsz
        self.sections = []
        for i in range(nsec):
            o = sect + i * 40
            name = d[o:o + 8].rstrip(b'\0').decode('latin1')
            vsize, vaddr, rawsize, rawptr = struct.unpack_from('<IIII', d, o + 8)
            chars = struct.unpack_from('<I', d, o + 36)[0]
            self.sections.append(dict(name=name, va=vaddr, vsize=vsize, raw=rawptr,
                                      rawsize=rawsize, chars=chars))

    def off2rva(self, off):
        for s in self.sections:
            if s['raw'] <= off < s['raw'] + s['rawsize']:
                return off - s['raw'] + s['va']
        return None

    def rva2off(self, rva):
        for s in self.sections:
            if s['va'] <= rva < s['va'] + max(s['vsize'], s['rawsize']):
                return rva - s['va'] + s['raw']
        return None

    def exec_sections(self):
        return [s for s in self.sections if s['chars'] & IMAGE_SCN_MEM_EXECUTE]


def xrefs(pe, target_rva):
    hits = []
    d = pe.mm
    for s in pe.exec_sections():
        base = s['raw']
        lo = s['va']
        end = min(base + s['rawsize'], len(d))
        total = end - base
        for off in range(4):
            n = (total - off) // 4
            if n <= 0:
                continue
            arr = np.frombuffer(d, dtype='<i4', count=n, offset=base + off)
            idx = np.arange(n, dtype=np.int64)
            rhs = target_rva - lo - 4 - off - 4 * idx
            m = np.nonzero(arr == rhs)[0]
            for k in m:
                hits.append(base + off + 4 * int(k))
    return hits


def show(pe, off, back=20, fwd=24):
    a = max(0, off - back)
    b = min(len(pe.mm), off + fwd)
    return pe.mm[a:b].hex(' ')


def main():
    pe = PE(sys.argv[1])
    print('image_base 0x%X' % pe.image_base)
    if sys.argv[2] == '--str':
        text = sys.argv[3].encode()
        start, offs = 0, []
        while True:
            i = pe.mm.find(text, start)
            if i < 0:
                break
            offs.append(i)
            start = i + 1
        targets = []
        for o in offs:
            r = pe.off2rva(o)
            print('string %r at file 0x%X rva 0x%X va 0x%X'
                  % (text.decode(), o, r, pe.image_base + r))
            targets.append(r)
    else:
        targets = [int(x, 16) for x in sys.argv[2:]]

    for t in targets:
        hs = xrefs(pe, t)
        print('\nxrefs to rva 0x%X: %d' % (t, len(hs)))
        for h in hs:
            r = pe.off2rva(h)
            print('  rva 0x%X va 0x%X  bytes: %s' % (r, pe.image_base + r, show(pe, h)))


if __name__ == '__main__':
    main()
