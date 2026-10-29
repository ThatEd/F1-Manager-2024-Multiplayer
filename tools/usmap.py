"""Minimal .usmap parser: list classes, their super chain and properties.

  python usmap.py <file.usmap> --find <substr>      find classes/super/structs matching
  python usmap.py <file.usmap> --class <Name>       dump that class's properties
"""
import sys
import struct


class Reader:
    def __init__(self, data, pos=0):
        self.d = data
        self.p = pos

    def u8(self):
        v = self.d[self.p]
        self.p += 1
        return v

    def u16(self):
        v = struct.unpack_from('<H', self.d, self.p)[0]
        self.p += 2
        return v

    def u32(self):
        v = struct.unpack_from('<I', self.d, self.p)[0]
        self.p += 4
        return v

    def i32(self):
        v = struct.unpack_from('<i', self.d, self.p)[0]
        self.p += 4
        return v

    def u64(self):
        v = struct.unpack_from('<Q', self.d, self.p)[0]
        self.p += 8
        return v


def read_props(r, names):
    n = r.u32()
    out = []
    for _ in range(n):
        pname = names[r.i32()]
        ptype = names[r.i32()]
        array_dim = r.i32()
        enum_name = None
        if ptype == 'EnumProperty':
            enum_name = names[r.i32()]
        inner = None
        if ptype in ('ArrayProperty', 'SetProperty'):
            inner = names[r.i32()]
        elif ptype == 'MapProperty':
            inner = names[r.i32()] + ' -> ' + names[r.i32()]
        struct_name = None
        if ptype == 'StructProperty':
            struct_name = names[r.i32()]
        out.append(dict(name=pname, type=ptype, enum=enum_name, inner=inner,
                        struct=struct_name, dim=array_dim))
    return out


def load(path):
    d = open(path, 'rb').read()
    r = Reader(d)
    if r.u16() != 0xC263:
        raise SystemExit('not a usmap')
    r.u8()
    r.u32()  # hasVersioning
    r.u32()  # numEpochs
    for _ in range(r.u32()):
        r.u32()
    n = r.u32()
    names = []
    for _ in range(n):
        ln = r.u16()
        names.append(d[r.p:r.p + ln].decode('utf-8', 'replace'))
        r.p += ln

    enums = []
    for _ in range(r.u32()):
        enums.append(dict(name=names[r.i32()], values=None))
    structs = []
    for _ in range(r.u32()):
        name = names[r.i32()]
        sup = names[r.i32()]
        props = read_props(r, names)
        structs.append(dict(name=name, super=sup, props=props))
    classes = []
    for _ in range(r.u32()):
        name = names[r.i32()]
        sup = names[r.i32()]
        props = read_props(r, names)
        classes.append(dict(name=name, super=sup, props=props))
    return dict(names=names, enums=enums, structs=structs, classes=classes)


def main():
    m = load(sys.argv[1])
    print('names %d  enums %d  structs %d  classes %d'
          % (len(m['names']), len(m['enums']), len(m['structs']), len(m['classes'])))
    if '--find' in sys.argv:
        sub = sys.argv[sys.argv.index('--find') + 1].lower()
        for kind in ('classes', 'structs'):
            for c in m[kind]:
                if sub in c['name'].lower() or sub in (c['super'] or '').lower():
                    extras = ''
                    for p in c['props']:
                        if any(sub in str(v).lower() for v in
                               (p['name'], p['struct'], p['inner'], p['enum']) if v):
                            extras += '\n      %s : %s' % (p['name'], p['type'])
                    print('%-9s %-46s : %s%s' % (kind[:-1], c['name'], c['super'], extras))
    if '--class' in sys.argv:
        want = sys.argv[sys.argv.index('--class') + 1]
        for c in m['classes'] + m['structs']:
            if c['name'] == want:
                print('\n%s : %s  (%d props)' % (c['name'], c['super'], len(c['props'])))
                for p in c['props']:
                    t = p['type']
                    if p['struct']:
                        t += '<' + p['struct'] + '>'
                    if p['inner']:
                        t += '<' + p['inner'] + '>'
                    if p['enum']:
                        t += '<' + p['enum'] + '>'
                    print('    %-40s %s' % (p['name'], t))


if __name__ == '__main__':
    main()
