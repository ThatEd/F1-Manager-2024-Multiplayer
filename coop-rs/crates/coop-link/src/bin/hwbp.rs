//! Captures a function's arguments using a **hardware** breakpoint.
//!
//! The software-breakpoint version writes `INT3` into the target's code and must restore it on every
//! exit path. A force-kill cannot run cleanup, so a killed run leaves the byte behind and the game
//! dies later with no obvious cause - which happened twice.
//!
//! A hardware breakpoint uses the CPU's debug registers instead. Nothing is written to the target's
//! memory, so there is nothing to restore and a killed run is harmless: the registers belong to the
//! debuggee's thread and are cleared when the debugger detaches or dies.
//!
//! Mechanism: `GetThreadContext`/`SetThreadContext` with `CONTEXT_DEBUG_REGISTERS` sets `Dr0` to the
//! address and the local-enable bits in `Dr7`. The debug exception surfaces as
//! `EXCEPTION_SINGLE_STEP` (0x80000004) rather than a breakpoint.
//!
//! Limitation, stated up front: debug registers are per-thread. This arms threads that exist at attach
//! time; a thread created afterwards is not covered. For a UI event that runs on an existing main or
//! render thread, that is sufficient.

use std::ffi::c_void;

#[link(name = "kernel32")]
extern "system" {
    fn DebugActiveProcess(pid: u32) -> i32;
    fn DebugActiveProcessStop(pid: u32) -> i32;
    fn WaitForDebugEvent(event: *mut DebugEvent, millis: u32) -> i32;
    fn ContinueDebugEvent(pid: u32, tid: u32, status: u32) -> i32;
    fn ReadProcessMemory(h: isize, base: *const u8, buf: *mut u8, size: usize, read: *mut usize) -> i32;
    fn GetThreadContext(thread: isize, ctx: *mut u8) -> i32;
    fn SetThreadContext(thread: isize, ctx: *const u8) -> i32;
    fn OpenThread(access: u32, inherit: i32, id: u32) -> isize;
    fn CloseHandle(h: isize) -> i32;
    fn GetCurrentProcessId() -> u32;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> isize;
    fn Thread32First(snap: isize, entry: *mut ThreadEntry32) -> i32;
    fn Thread32Next(snap: isize, entry: *mut ThreadEntry32) -> i32;
}

#[repr(C)]
struct ThreadEntry32 {
    dw_size: u32,
    cnt_usage: u32,
    th32_thread_id: u32,
    th32_owner_process_id: u32,
    tp_base_pri: i32,
    tp_delta_pri: i32,
    dw_flags: u32,
}

const DBG_CONTINUE: u32 = 0x0001_0002;
const DBG_EXCEPTION_NOT_HANDLED: u32 = 0x8001_0001;
const CREATE_PROCESS_DEBUG_EVENT: u32 = 3;
const CREATE_THREAD_DEBUG_EVENT: u32 = 2;
const EXCEPTION_DEBUG_EVENT: u32 = 1;
const EXIT_PROCESS_DEBUG_EVENT: u32 = 5;
const EXCEPTION_SINGLE_STEP: u32 = 0x8000_0004;

const THREAD_ALL: u32 = 0x1F03FF;
const TH32CS_SNAPTHREAD: u32 = 0x4;
const UNION_SIZE: usize = 176;

// CONTEXT offsets for x64.
const CTX_SIZE: usize = 1232;
const OFF_CONTEXT_FLAGS: usize = 0x30;
const OFF_DR0: usize = 0x48;
const OFF_DR1: usize = 0x50;
const OFF_DR2: usize = 0x58;
const OFF_DR3: usize = 0x60;
const OFF_DR6: usize = 0x68;
const OFF_DR7: usize = 0x70;
const OFF_RAX: usize = 0x78;
const OFF_RCX: usize = 0x80;
const OFF_RDX: usize = 0x88;
const OFF_RBX: usize = 0x90;
const OFF_RSP: usize = 0x98;
const OFF_RBP: usize = 0xA0;
const OFF_RSI: usize = 0xA8;
const OFF_RDI: usize = 0xB0;
const OFF_R8: usize = 0xB8;
const OFF_R9: usize = 0xC0;
const OFF_RIP: usize = 0xF8;

const CONTEXT_DEBUG_REGISTERS: u32 = 0x0010_0000;
const CONTEXT_FULL: u32 = 0x0010_0007;

#[repr(C)]
struct DebugEvent {
    code: u32,
    pid: u32,
    tid: u32,
    _pad: u32,
    union: [u8; UNION_SIZE],
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn put_u64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

/// `Dr7` bits: L0 enables Dr0; RW0/LEN0 pick the access type. Execute is RW=00; read/write is RW=11
/// with LEN=00 for a single byte. `LE` (bit 8) asks for exact-match reporting.
fn dr7_for(watch_read: bool) -> u64 {
    if watch_read {
        // L0 | RW0=11 (read or write) | LEN0=00 (1 byte) | LE
        (1 << 0) | (0b11 << 16) | (0b00 << 18) | (1 << 8)
    } else {
        // L0 | RW0=00 (execute) | LEN0=00 | LE
        (1 << 0) | (0b00 << 16) | (0b00 << 18) | (1 << 8)
    }
}

/// Arms `Dr0` on every existing thread of `pid`.
///
/// Returns the threads it armed, so they can be disarmed on the way out. `Dr7` local-enable bits:
/// `L0` is bit 0; the RW/LEN fields for `Dr0` stay zero, which means "execute" and length 1.
fn arm_threads(pid: u32, addr: usize, watch_read: bool) -> Vec<isize> {
    let mut armed = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == 0 || snap == -1 {
            return armed;
        }
        let mut te: ThreadEntry32 = std::mem::zeroed();
        te.dw_size = std::mem::size_of::<ThreadEntry32>() as u32;
        let mut ok = Thread32First(snap, &mut te) != 0;
        while ok {
            if te.th32_owner_process_id == pid {
                let th = OpenThread(THREAD_ALL, 0, te.th32_thread_id);
                if th != 0 {
                    let mut ctx = vec![0u8; CTX_SIZE];
                    // Ask for the debug registers only; a full context on every thread is unnecessary.
                    let flags = (CONTEXT_DEBUG_REGISTERS | 0x100000).to_le_bytes();
                    ctx[OFF_CONTEXT_FLAGS..OFF_CONTEXT_FLAGS + 4].copy_from_slice(&flags);
                    if GetThreadContext(th, ctx.as_mut_ptr()) != 0 {
                        put_u64(&mut ctx, OFF_DR0, addr as u64);
                        // L0 (bit 0) enables Dr0; also set LE (bit 8) for exact-match reporting.
                        put_u64(&mut ctx, OFF_DR7, dr7_for(watch_read));
                        if SetThreadContext(th, ctx.as_ptr()) != 0 {
                            armed.push(th);
                        } else {
                            CloseHandle(th);
                        }
                    } else {
                        CloseHandle(th);
                    }
                }
            }
            te.dw_size = std::mem::size_of::<ThreadEntry32>() as u32;
            ok = Thread32Next(snap, &mut te) != 0;
        }
        CloseHandle(snap);
    }
    armed
}

fn disarm_threads(handles: &[isize]) {
    for &th in handles {
        unsafe {
            let mut ctx = vec![0u8; CTX_SIZE];
            let flags = CONTEXT_DEBUG_REGISTERS.to_le_bytes();
            ctx[OFF_CONTEXT_FLAGS..OFF_CONTEXT_FLAGS + 4].copy_from_slice(&flags);
            if GetThreadContext(th, ctx.as_mut_ptr()) != 0 {
                put_u64(&mut ctx, OFF_DR0, 0);
                put_u64(&mut ctx, OFF_DR7, 0);
                SetThreadContext(th, ctx.as_ptr());
            }
            CloseHandle(th);
        }
    }
}

/// Finds `needle` in the target's committed memory, preferring the last occurrence.
///
/// The last copy is normally the native registry entry rather than the authored JS source, which
/// appears earlier. Watching the registry copy is what reveals the dispatch code.
fn find_string(pid: u32, needle: &str) -> Option<usize> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn VirtualQueryEx(h: isize, addr: *const u8, info: *mut Mbi, len: usize) -> usize;
        fn CloseHandle(h: isize) -> i32;
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Mbi {
        base: usize,
        alloc_base: usize,
        alloc_protect: u32,
        partition: u16,
        region_size: usize,
        state: u32,
        protect: u32,
        kind: u32,
    }
    const MEM_COMMIT: u32 = 0x1000;
    let h = unsafe { OpenProcess(0x0410, 0, pid) };
    if h == 0 {
        return None;
    }
    let bytes = needle.as_bytes();
    let mut best: Option<usize> = None;
    let mut addr = 0x1_0000usize;
    unsafe {
        while addr < 0x7FFF_FFFF_0000 {
            let mut info = Mbi::default();
            if VirtualQueryEx(h, addr as *const u8, &mut info, std::mem::size_of::<Mbi>()) == 0 {
                break;
            }
            if info.region_size == 0 {
                break;
            }
            let readable = info.state == MEM_COMMIT
                && (info.protect & 0xFF) != 0x01
                && (info.protect & 0x100) == 0;
            if readable {
                let n = info.region_size.min(0x40_0000);
                let mut buf = vec![0u8; n];
                let mut got = 0usize;
                if ReadProcessMemory(h, info.base as *const u8, buf.as_mut_ptr(), n, &mut got) != 0
                    && got == n
                {
                    let mut i = 0usize;
                    while i + bytes.len() <= buf.len() {
                        if &buf[i..i + bytes.len()] == bytes {
                            best = Some(info.base + i);
                        }
                        i += 1;
                    }
                }
            }
            addr = info.base.saturating_add(info.region_size);
        }
        CloseHandle(h);
    }
    best
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |n: &str| -> Option<String> {
        args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).cloned()
    };
    let pid: u32 = match get("--pid").and_then(|s| s.parse().ok()) {
        Some(p) => p,
        None => {
            eprintln!("coop-hwbp --pid <pid> --at <hex> [--hits N] [--wait-secs N]");
            eprintln!();
            eprintln!("Hardware breakpoint: writes NOTHING to the target, so a killed run is harmless.");
            std::process::exit(2);
        }
    };
    // Addresses move between sessions, so `--find <text>` locates the string fresh every run. That
    // makes the tool usable without me hand-carrying an address from a previous process.
    let at = if let Some(needle) = get("--find") {
        match find_string(pid, &needle) {
            Some(a) => {
                println!("found {needle:?} at 0x{a:X}");
                a
            }
            None => {
                eprintln!("could not find {needle:?} in the target");
                std::process::exit(1);
            }
        }
    } else {
        match get("--at").and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok()) {
            Some(a) => a,
            None => {
                eprintln!("--at <hex> or --find <text> is required");
                std::process::exit(2);
            }
        }
    };
    let hits_wanted: usize = get("--hits").and_then(|s| s.parse().ok()).unwrap_or(1);
    // --rw read watches READS of a byte rather than execution. That answers a different question:
    // "which code looks at this string?" - the way to find an event dispatcher whose name is only
    // ever compared, never addressed.
    let watch_read = args.iter().any(|a| a == "--rw-read");
    let wait_ms: u32 = get("--wait-secs")
        .and_then(|s| s.parse::<u32>().ok())
        .map(|s| s * 1000)
        .unwrap_or(300_000);

    println!("attaching to pid {pid}, hardware breakpoint at 0x{at:X}");
    if unsafe { DebugActiveProcess(pid) } == 0 {
        eprintln!("DebugActiveProcess failed: {}", std::io::Error::last_os_error());
        std::process::exit(1);
    }

    let mut handle: isize = 0;
    let mut seen = 0usize;
    let mut ev = DebugEvent { code: 0, pid: 0, tid: 0, _pad: 0, union: [0; UNION_SIZE] };
    let mut armed: Vec<isize> = Vec::new();
    let mut arm_after_create = false;

    loop {
        if unsafe { WaitForDebugEvent(&mut ev, wait_ms) } == 0 {
            eprintln!("no debug event for {} s; detaching", wait_ms / 1000);
            break;
        }
        let mut status = DBG_CONTINUE;

        match ev.code {
            CREATE_PROCESS_DEBUG_EVENT => {
                handle = u64_at(&ev.union, 0x08) as isize;
                println!("  process handle 0x{handle:X}");
                arm_after_create = true;
            }
            CREATE_THREAD_DEBUG_EVENT => {
                // A thread that appears after we armed is not covered by the debug registers we set,
                // so arm it too. This is what makes the approach hold for threads spawned later.
                let th = u64_at(&ev.union, 0x08) as isize;
                if th != 0 && !armed.is_empty() {
                    let mut ctx = vec![0u8; CTX_SIZE];
                    let flags = CONTEXT_DEBUG_REGISTERS.to_le_bytes();
                    ctx[OFF_CONTEXT_FLAGS..OFF_CONTEXT_FLAGS + 4].copy_from_slice(&flags);
                    unsafe {
                        if GetThreadContext(th, ctx.as_mut_ptr()) != 0 {
                            put_u64(&mut ctx, OFF_DR0, at as u64);
                            put_u64(&mut ctx, OFF_DR7, dr7_for(watch_read));
                            SetThreadContext(th, ctx.as_ptr());
                        }
                    }
                }
            }
            EXCEPTION_DEBUG_EVENT => {
                let code = u32_at(&ev.union, 0x00);
                if code == EXCEPTION_SINGLE_STEP {
                    let th = unsafe { OpenThread(THREAD_ALL, 0, ev.tid) };
                    if th != 0 {
                        let mut ctx = vec![0u8; CTX_SIZE];
                        let flags = CONTEXT_FULL.to_le_bytes();
                        ctx[OFF_CONTEXT_FLAGS..OFF_CONTEXT_FLAGS + 4].copy_from_slice(&flags);
                        if unsafe { GetThreadContext(th, ctx.as_mut_ptr()) } != 0 {
                            let rip = u64_at(&ctx, OFF_RIP);
                            // For an execute breakpoint RIP is the watched address. For a data
                            // breakpoint RIP is whatever instruction touched it - that is the answer
                            // we want, so accept either case.
                            if watch_read || rip == at as u64 || rip == at as u64 + 1 {
                                seen += 1;
                                println!("\n=== hit {seen} at 0x{at:X} (thread {}) ===", ev.tid);
                                for (n, o) in [
                                    ("RAX", OFF_RAX), ("RCX", OFF_RCX), ("RDX", OFF_RDX), ("RBX", OFF_RBX),
                                    ("RSI", OFF_RSI), ("RDI", OFF_RDI), ("R8", OFF_R8), ("R9", OFF_R9),
                                    ("RSP", OFF_RSP), ("RBP", OFF_RBP), ("RIP", OFF_RIP),
                                ] {
                                    println!("  {n:4} = 0x{:016X}", u64_at(&ctx, o));
                                }
                                // Return address identifies the caller without a stack walk.
                                let rsp = u64_at(&ctx, OFF_RSP) as usize;
                                let mut slot = [0u8; 8];
                                let mut got = 0usize;
                                if unsafe {
                                    ReadProcessMemory(handle, rsp as *const u8, slot.as_mut_ptr(), 8, &mut got)
                                } != 0
                                    && got == 8
                                {
                                    println!("  [rsp] return = 0x{:X}", u64::from_le_bytes(slot));
                                }
                                for (name, o) in [("RCX", OFF_RCX), ("RDX", OFF_RDX), ("R8", OFF_R8), ("R9", OFF_R9)] {
                                    let p = u64_at(&ctx, o) as usize;
                                    if p >= 0x1_0000 {
                                        let mut head = [0u8; 48];
                                        if unsafe {
                                            ReadProcessMemory(handle, p as *const u8, head.as_mut_ptr(), 48, &mut got)
                                        } != 0
                                            && got == 48
                                        {
                                            let hex: String = head.iter().map(|b| format!("{b:02x}")).collect();
                                            println!("  [{name}] = {hex}");
                                        }
                                    }
                                }
                                // Single-step exceptions repeat until DR6 is cleared.
                                put_u64(&mut ctx, OFF_DR6, 0);
                                unsafe { SetThreadContext(th, ctx.as_ptr()) };
                            }
                        }
                        unsafe { CloseHandle(th) };
                    }
                } else {
                    // Any other exception is not ours: let the game's own handlers see it.
                    status = DBG_EXCEPTION_NOT_HANDLED;
                }
            }
            EXIT_PROCESS_DEBUG_EVENT => {
                println!("  target exited");
                break;
            }
            _ => {}
        }

        unsafe { ContinueDebugEvent(ev.pid, ev.tid, status) };

        if arm_after_create {
            arm_after_create = false;
            armed = arm_threads(pid, at, watch_read);
            println!("  armed {} thread(s) via Dr0 (nothing written to the target)", armed.len());
        }
        if seen >= hits_wanted && hits_wanted > 0 {
            break;
        }
    }

    disarm_threads(&armed);
    unsafe { DebugActiveProcessStop(pid) };
    println!("detached; no bytes were modified, so the target is unharmed");
    let _ = unsafe { GetCurrentProcessId() };
}
