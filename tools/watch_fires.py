"""Find the agent's hook block in the live game and read its fire counter.

The block address is only logged by newer builds, so this does not depend on the log. The stub's first
two instructions are RIP-relative and their displacements are *constants*, because they point at fixed
offsets inside the same block:

    mov [rip+disp32], rcx     at block+0x00, target block+SLOT_OFF
    inc qword [rip+disp32]    at block+0x07, target block+FIRES_OFF

so the displacement is SLOT_OFF-7 and FIRES_OFF-0x0E whatever the block's address is. For this layout
(0x90 / 0x98) that is `89 00 00 00` and `8A 00 00 00`; the earlier layout (0x40 / 0x48) gives `39` and
`3A`. Matching either identifies the stub exactly, and the match address IS the block base.

Read-only: nothing here writes to the game.
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

k32 = ctypes.windll.kernel32
k32.OpenProcess.restype = w.HANDLE
k32.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
k32.ReadProcessMemory.argtypes = [w.HANDLE, ctypes.c_void_p, ctypes.c_void_p,
                                  ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)]
k32.VirtualQueryEx.argtypes = [w.HANDLE, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t]
k32.CloseHandle.argtypes = [w.HANDLE]

# candidate (SLOT_OFF, FIRES_OFF, CALLBACK_OFF) layouts, newest first
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


def read_u64(h, addr):
    b = read(h, addr, 8)
    return None if b is None else int.from_bytes(b, 'little')


def regions(h):
    addr = 0
    limit = 0x7FFFFFFF0000
    while addr < limit:
        mbi = MBI()
        if not k32.VirtualQueryEx(h, ctypes.c_void_p(addr), ctypes.byref(mbi), ctypes.sizeof(mbi)):
            break
        size = mbi.RegionSize or 0x1000
        if (mbi.State == MEM_COMMIT and not (mbi.Protect & (PAGE_GUARD | PAGE_NOACCESS))
                and mbi.Protect not in (0,)):
            yield int(mbi.BaseAddress or addr), size, mbi.Protect
        addr = (int(mbi.BaseAddress or addr)) + size


def find_stub(h):
    """Return (block, layout) or (None, None). Only small private regions can hold it."""
    for layout in LAYOUTS:
        slot_off, fires_off, cb_off = layout
        d1 = (slot_off - 7).to_bytes(4, 'little')
        d2 = (fires_off - 0x0E).to_bytes(4, 'little')
        sig = b'\x48\x89\x0d' + d1 + b'\x48\xff\x05' + d2
        for base, size, protect in regions(h):
            if size > 0x100000:          # the block is one page; skip big regions to stay quick
                continue
            data = read(h, base, size)
            if not data:
                continue
            i = data.find(sig)
            if i >= 0:
                return base + i, layout
    return None, None


def main():
    pid = int(sys.argv[1]) if len(sys.argv) > 1 else None
    seconds = float(sys.argv[2]) if len(sys.argv) > 2 else 10.0
    if pid is None:
        raise SystemExit('usage: watch_fires.py <pid> [seconds]')

    h = k32.OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, False, pid)
    if not h:
        raise SystemExit('cannot open pid %d' % pid)

    block, layout = find_stub(h)
    if block is None:
        k32.CloseHandle(h)
        raise SystemExit('could not find the hook stub in pid %d.\n'
                         '  If the log says "hook installed", the build may use a different layout;\n'
                         '  otherwise the hook is not installed.' % pid)

    _, fires_off, cb_off = layout
    print('block 0x%X (layout slot=0x%X fires=0x%X)' % (block, layout[0], fires_off))
    cb = read_u64(h, block + cb_off) or 0
    print('callback 0x%X %s' % (cb, '' if cb else '<-- INERT, frames never apply'))

    first = read_u64(h, block + fires_off)
    print('fires at start: %s' % first)

    deadline = time.time() + seconds
    last = first if first is not None else 0
    while time.time() < deadline:
        time.sleep(1.0)
        now = read_u64(h, block + fires_off)
        if now is None:
            print('read failed (game exited?)')
            break
        print('fires %-12d (+%d)' % (now, now - last))
        last = now

    k32.CloseHandle(h)
    if first is not None and last > first:
        print('\nRESULT: hook is RUNNING - %d calls in %.1fs' % (last - first, seconds))
    else:
        print('\nRESULT: hook never fired - the patched function is not being called.'
              '\n        A race session must be running for it to fire.')


if __name__ == '__main__':
    main()
