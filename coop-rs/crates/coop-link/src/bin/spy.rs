//! Captures the arguments of a function call in a running game, using a debugger breakpoint.
//!
//! Why a debugger rather than our own inline hook: the 5-byte patch crashed this game once at a
//! different site and the cause was never proven. `INT3` is handled by the OS, and on the breakpoint
//! Windows hands us a complete `CONTEXT` — every general-purpose register — for the faulting thread.
//! That is exactly what is needed to learn the calling convention of a function, with no code of ours
//! executing inside the game at all.
//!
//! The breakpoint byte is restored before the thread is resumed, so the game continues normally. The
//! debugger then detaches, leaving the process as it was.
//!
//! Use this to answer: "what does the game pass in RCX/RDX when a UI action fires?" — then replay it
//! with real values instead of synthesising a structure and hoping.

use std::ffi::c_void;

#[link(name = "kernel32")]
extern "system" {
    fn DebugActiveProcess(pid: u32) -> i32;
    fn DebugActiveProcessStop(pid: u32) -> i32;
    fn WaitForDebugEvent(event: *mut DebugEvent, millis: u32) -> i32;
    fn ContinueDebugEvent(pid: u32, tid: u32, status: u32) -> i32;
    fn ReadProcessMemory(h: isize, base: *const u8, buf: *mut u8, size: usize, read: *mut usize) -> i32;
    fn WriteProcessMemory(h: isize, base: *mut u8, buf: *const u8, size: usize, wrote: *mut usize) -> i32;
    fn GetThreadContext(thread: isize, ctx: *mut u8) -> i32;
    fn SetThreadContext(thread: isize, ctx: *const u8) -> i32;
    fn OpenThread(access: u32, inherit: i32, id: u32) -> isize;
    fn CloseHandle(h: isize) -> i32;
    fn FlushInstructionCache(h: isize, base: *const c_void, size: usize) -> i32;
}

const DBG_CONTINUE: u32 = 0x0001_0002;
const DBG_EXCEPTION_NOT_HANDLED: u32 = 0x8001_0001;
const EXCEPTION_DEBUG_EVENT: u32 = 1;
const CREATE_PROCESS_DEBUG_EVENT: u32 = 3;
const EXIT_PROCESS_DEBUG_EVENT: u32 = 5;
const EXCEPTION_BREAKPOINT: u32 = 0x8000_0003;

const THREAD_GET_CONTEXT: u32 = 0x0008;
const THREAD_SET_CONTEXT: u32 = 0x0010;
const THREAD_SUSPEND_RESUME: u32 = 0x0002;
const THREAD_QUERY_INFORMATION: u32 = 0x0040;

const CONTEXT_SIZE: usize = 1232;
const CONTEXT_FLAGS_ALL: u32 = 0x0010_0000 | 0x0000_0002;
const OFF_CONTEXT_FLAGS: usize = 0x30;
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

/// The union in `DEBUG_EVENT` is large; a byte buffer of adequate size avoids modelling every variant.
const UNION_SIZE: usize = 176;

#[repr(C)]
struct DebugEvent {
    code: u32,
    pid: u32,
    tid: u32,
    _pad: u32,
    union: [u8; UNION_SIZE],
}

fn read_u32(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

fn read_u64(buf: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(buf[at..at + 8].try_into().unwrap())
}

/// Owns the arming state so it is undone on every exit path, including being killed.
///
/// The first version put cleanup at the end of `main()`. The process was killed instead, so the
/// `INT3` stayed in game memory and a thread stayed blocked on it, and the game died with
/// `ERROR_PARTIAL_COPY`. Order matters: restore the byte, resume every thread we suspended, and only
/// then detach. Never detach while a thread's RIP sits on our patch.
struct Arming {
    pid: u32,
    at: usize,
    original: Option<u8>,
    process: isize,
    armed: bool,
}

impl Arming {
    fn restore(&mut self) {
        if !self.armed {
            return;
        }
        if let (Some(b), true) = (self.original, self.process != 0) {
            let one = [b];
            let mut wrote = 0usize;
            unsafe {
                WriteProcessMemory(self.process, self.at as *mut u8, one.as_ptr(), 1, &mut wrote);
                FlushInstructionCache(self.process, self.at as *const c_void, 1);
            }
            println!("  restored {:02x} at 0x{:X}", b, self.at);
        }
        self.armed = false;
    }
}

impl Drop for Arming {
    fn drop(&mut self) {
        self.restore();
        // Detach last, and only after the byte is back: a detach with a patch in place and a thread
        // parked on it is what corrupted the previous attempt.
        if self.pid != 0 {
            unsafe { DebugActiveProcessStop(self.pid) };
            println!("detached from pid {}", self.pid);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |name: &str| -> Option<String> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
    };
    let pid: u32 = match get("--pid").and_then(|s| s.parse().ok()) {
        Some(p) => p,
        None => {
            eprintln!("coop-spy --pid <pid> --at <hex address> [--hits N]");
            eprintln!();
            eprintln!("Attaches as a debugger, breaks once at the address, prints every register at");
            eprintln!("that instant, restores the original byte and detaches. The game keeps running.");
            std::process::exit(2);
        }
    };
    let at = match get("--at").and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok()) {
        Some(a) => a,
        None => {
            eprintln!("--at <hex address> is required");
            std::process::exit(2);
        }
    };
    let hits: usize = get("--hits").and_then(|s| s.parse().ok()).unwrap_or(1);

    println!("attaching to pid {pid}, breaking at 0x{at:X}");

    if unsafe { DebugActiveProcess(pid) } == 0 {
        let err = std::io::Error::last_os_error();
        eprintln!("DebugActiveProcess failed: {err}");
        eprintln!("(a process already debugging the game, or anti-debug, will cause this)");
        std::process::exit(1);
    }

    // Access to the target's memory comes with the debug handle we receive on CREATE_PROCESS.
    let mut guard = Arming { pid, at, original: None, process: 0, armed: false };
    let mut process_handle: isize = 0;
    let mut seen = 0usize;
    let mut event = DebugEvent { code: 0, pid: 0, tid: 0, _pad: 0, union: [0; UNION_SIZE] };

    loop {
        let wait_ms: u32 = args
            .iter()
            .position(|a| a == "--wait-secs")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse::<u32>().ok())
            .map(|s| s * 1000)
            .unwrap_or(300_000);
        if unsafe { WaitForDebugEvent(&mut event, wait_ms) } == 0 {
            eprintln!("no debug event for {} s; detaching", wait_ms / 1000);
            break;
        }

        let mut status = DBG_CONTINUE;

        match event.code {
            CREATE_PROCESS_DEBUG_EVENT => {
                // CREATE_PROCESS_DEBUG_INFO begins with hFile, then hProcess, hThread.
                let _h_file = read_u64(&event.union, 0x00) as isize;
                let h_process = read_u64(&event.union, 0x08) as isize;
                process_handle = h_process;
                println!("  process handle 0x{h_process:X}");

                // Arm the breakpoint only now that we have a handle to write through.
                let mut before = [0u8; 1];
                let mut got = 0usize;
                let ok = unsafe {
                    ReadProcessMemory(h_process, at as *const u8, before.as_mut_ptr(), 1, &mut got)
                };
                if ok == 0 || got != 1 {
                    eprintln!("  cannot read 0x{at:X}");
                    break;
                }
                guard.original = Some(before[0]);
                println!("  original byte at target: {:02x}", before[0]);
                if before[0] == 0xCC {
                    eprintln!("  a breakpoint is already there; refusing to double-set");
                    break;
                }
                let int3 = [0xCCu8];
                let mut wrote = 0usize;
                let ok = unsafe {
                    WriteProcessMemory(h_process, at as *mut u8, int3.as_ptr(), 1, &mut wrote)
                };
                if ok == 0 || wrote != 1 {
                    eprintln!("  cannot write the breakpoint");
                    break;
                }
                unsafe { FlushInstructionCache(h_process, at as *const c_void, 1) };
                guard.process = h_process;
                guard.armed = true;
                println!("  breakpoint armed");
            }

            EXCEPTION_DEBUG_EVENT => {
                // EXCEPTION_DEBUG_INFO: ExceptionRecord (code at 0x00), dwFirstChance at 0x98.
                let code = read_u32(&event.union, 0x00);
                let exception_addr = read_u64(&event.union, 0x18) as usize;

                if code == EXCEPTION_BREAKPOINT && guard.armed && (exception_addr == at || exception_addr == at + 1) {
                    let h_thread = unsafe {
                        OpenThread(
                            THREAD_GET_CONTEXT | THREAD_SET_CONTEXT | THREAD_SUSPEND_RESUME | THREAD_QUERY_INFORMATION,
                            0,
                            event.tid,
                        )
                    };
                    if h_thread == 0 {
                        eprintln!("  cannot open thread {}", event.tid);
                    } else {
                        let mut ctx = vec![0u8; CONTEXT_SIZE];
                        let flags = CONTEXT_FLAGS_ALL.to_le_bytes();
                        ctx[OFF_CONTEXT_FLAGS..OFF_CONTEXT_FLAGS + 4].copy_from_slice(&flags);
                        let h_process = process_handle;
                    if unsafe { GetThreadContext(h_thread, ctx.as_mut_ptr()) } != 0 {
                            seen += 1;
                            println!("\n=== hit {seen} at 0x{at:X} (thread {}) ===", event.tid);
                            let show = |name: &str, off: usize| {
                                println!("  {name:4} = 0x{:016X}", read_u64(&ctx, off));
                            };
                            show("RAX", OFF_RAX);
                            show("RCX", OFF_RCX);
                            show("RDX", OFF_RDX);
                            show("RBX", OFF_RBX);
                            show("RSI", OFF_RSI);
                            show("RDI", OFF_RDI);
                            show("R8", OFF_R8);
                            show("R9", OFF_R9);
                            let rsp = read_u64(&ctx, OFF_RSP);
                            let rbp = read_u64(&ctx, OFF_RBP);
                            let rip = read_u64(&ctx, OFF_RIP);
                            println!("  RSP  = 0x{rsp:016X}");
                            println!("  RBP  = 0x{rbp:016X}");
                            println!("  RIP  = 0x{rip:016X}  ({} the target)", if rip == (at as u64) + 1 { "after" } else if rip == at as u64 { "at" } else { "elsewhere from" });

                            // The return address identifies the caller without needing a stack walk.
                            let mut slot = [0u8; 8];
                            let mut got = 0usize;
                            if unsafe {
                                ReadProcessMemory(h_process, rsp as *const u8, slot.as_mut_ptr(), 8, &mut got)
                            } != 0 && got == 8
                            {
                                let ret = u64::from_le_bytes(slot);
                                println!("  [rsp] return address = 0x{ret:X}");
                            }

                            // Peek at what the first argument points to, since that is what we need.
                            let rdx = read_u64(&ctx, OFF_RDX);
                            if rdx >= 0x1_0000 {
                                let mut head = [0u8; 64];
                                if unsafe {
                                    ReadProcessMemory(h_process, rdx as *const u8, head.as_mut_ptr(), 64, &mut got)
                                } != 0 && got == 64
                                {
                                    let hex: String = head.iter().map(|b| format!("{b:02x}")).collect();
                                    println!("  [RDX] = {hex}");
                                }
                            }

                            // Rewind RIP past the INT3 and restore the real instruction byte.
                            let at_le = at.to_le_bytes();
                            ctx[OFF_RIP..OFF_RIP + 8].copy_from_slice(&at_le);
                            if let Some(b) = guard.original {
                                let one = [b];
                                let mut wrote = 0usize;
                                unsafe {
                                    WriteProcessMemory(h_process, at as *mut u8, one.as_ptr(), 1, &mut wrote);
                                    FlushInstructionCache(h_process, at as *const c_void, 1);
                                }
                            }
                            unsafe { SetThreadContext(h_thread, ctx.as_ptr()) };
                            guard.armed = false;
                            println!("  breakpoint removed, original byte restored");
                        }
                        unsafe { CloseHandle(h_thread) };
                    }
                    if seen >= hits {
                        println!("\ncaptured {seen} hit(s); detaching");
                        status = DBG_EXCEPTION_NOT_HANDLED;
                    }
                } else if code == 0x8000_0004 {
                    // Single-step: not used, but do not treat it as fatal.
                }
            }

            EXIT_PROCESS_DEBUG_EVENT => {
                println!("  target exited");
                break;
            }
            _ => {}
        }

        unsafe { ContinueDebugEvent(event.pid, event.tid, status) };

        if seen >= hits && hits > 0 {
            break;
        }
    }

    // Cleanup and detach are owned by `guard`; dropping it restores the byte first.
}
