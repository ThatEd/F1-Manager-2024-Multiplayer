"""Static PE helper: find RIP-relative references to a given file offset/string.

Usage:
  python xrefs.py <exe> <string>         # locate the string, then every lea/mov that points at it
  python xrefs.py <exe> --rva 0x5f01788
"""
import sys
import struct

IMAGE_SCN_MEM_EXECUTE = 0x20000000


class PE:
    def __init__(self, path):
        self.f = open(path, 'rb')
        self.data = self.f.read()
        d = self.data
        e_lfanew = struct.unpack_from('<I', d, 0x3C)[0]
        assert d[e_lfanew:e_lfanew + 4] == b'PE\0\0'
        coff = e_lfanew + 4
        self.nsections = struct.unpack_from('<H', d, coff + 2)[0]
        self.opt_size = struct.unpack_from('<H', d, coff + 16)[0]
        opt = coff + 20
        self.magic = struct.unpack_from('<H', d, opt)[0]
        self.image_base = struct.unpack_from('<Q', d, opt + 24)[0]
        self.size_of_image = struct.unpack_from('<I', d, opt + 56)[0]
        sect = opt + self.opt_size
        self.sections = []
        for i in range(self.nsections):
            o = sect + i * 40
            name = d[o:o + 8].rstrip(b'\0').decode('latin1')
            vsize, vaddr, rawsize, rawptr = struct.unpack_from('<IIII', d, o + 8)
            chars = struct.unpack_from('<I', d, o + 36)[0]
            self.sections.append(dict(name=name, va=vaddr, vsize=vsize,
                                      raw=rawptr, rawsize=rawsize, chars=chars))

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


def find_string(pe, text):
    pat = text.encode()
    out = []
    start = 0
    while True:
        i = pe.data.find(pat, start)
        if i < 0:
            break
        out.append(i)
        start = i + 1
    return out


def xrefs_to_rva(pe, target_rva):
    """Every 4-byte disp in an executable section where next_insn + disp == target."""
    hits = []
    d = pe.data
    for s in pe.sections:
        if not (s['chars'] & IMAGE_SCN_MEM_EXECUTE):
            continue
        base = s['raw']
        end = min(base + s['rawsize'], len(d))
        lo = s['va']
        for p in range(base, end - 4):
            disp = struct.unpack_from('<i', d, p)[0]
            insn_end_rva = (p + 4 - base) + lo
            if insn_end_rva + disp == target_rva:
                hits.append(p)
    return hits


def describe(pe, off, back=16, fwd=16):
    d = pe.data
    a = max(0, off - back)
    b = min(len(d), off + fwd)
    return d[a:b].hex(' ')


def main():
    path = sys.argv[1]
    pe = PE(path)
    print('image_base 0x%X  size_of_image 0x%X  sections %d'
          % (pe.image_base, pe.size_of_image, len(pe.sections)))
    for s in pe.sections:
        print('  %-8s va 0x%08X vsize 0x%08X raw 0x%08X rawsize 0x%08X %s'
              % (s['name'], s['va'], s['vsize'], s['raw'], s['rawsize'],
                 'X' if s['chars'] & IMAGE_SCN_MEM_EXECUTE else '-'))

    if sys.argv[2] == '--rva':
        rvas = [int(sys.argv[3], 16)]
    else:
        text = sys.argv[2]
        offs = find_string(pe, text)
        print('\nstring %r -> %d hit(s)' % (text, len(offs)))
        rvas = []
        for o in offs:
            r = pe.off2rva(o)
            print('  file 0x%X  rva 0x%X  va 0x%X' % (o, r, pe.image_base + r))
            rvas.append(r)

    for rva in rvas:
        hits = xrefs_to_rva(pe, rva)
        print('\nxrefs to rva 0x%X: %d' % (rva, len(hits)))
        for p in hits:
            r = pe.off2rva(p)
            print('  insn-end at rva 0x%X (va 0x%X)  %s'
                  % (r, pe.image_base + r, describe(pe, p - 3, 12, 10)))


if __name__ == '__main__':
    main()
