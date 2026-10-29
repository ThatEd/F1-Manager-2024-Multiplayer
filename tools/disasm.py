"""Disassemble a byte range of a PE file:  python dis.py <exe> <rva-hex> <length-hex>"""
import sys
from capstone import Cs, CS_ARCH_X86, CS_MODE_64

from fastxref import PE


def main():
    pe = PE(sys.argv[1])
    rva = int(sys.argv[2], 16)
    n = int(sys.argv[3], 16)
    off = pe.rva2off(rva)
    code = pe.mm[off:off + n]
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    for ins in md.disasm(code, pe.image_base + rva):
        line = '%016X  %-24s %s %s' % (ins.address, ins.bytes.hex(), ins.mnemonic, ins.op_str)
        print(line)


if __name__ == '__main__':
    main()
