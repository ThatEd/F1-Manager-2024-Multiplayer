//! Crash and hang evidence: a trace of what the agent was doing, and a minidump when it dies.
//!
//! Why this exists. The first attempt to install the sim hook crashed the game with no Windows
//! Error Reporting event and no dump at all, so the only fact left was "it crashed". Nothing could
//! be retried safely, because a second attempt would produce the same silence. This module makes
//! the next crash self-describing:
//!
//! * **trace** - a line per stage transition, written straight to disk with `WriteFile` on a handle
//!   opened at load time. No allocation, no `PathBuf`, so it is safe to call from inside an
//!   exception handler where the heap may already be corrupt. Process death does not lose data
//!   already written: the bytes are in the file, not in our address space.
//! * **first-chance log** - a vectored handler notes access violations *as they happen*, including
//!   whether the faulting instruction is inside our own hook. A first-chance fault that the game
//!   swallows is often the only visible symptom before a silent desync or a later crash.
//! * **minidump** on the unhandled exception, with the trace embedded as a comment stream so the
//!   dump and the breadcrumb can never be separated.
//!
//! What was measured on this build, because each of these looked like it should work:
//!
//! * a dump written from the exception dispatcher on the faulting thread fails with `ERROR_NOACCESS`;
//! * the same call from a second thread succeeds, so the filter hands the work over and waits for it;
//! * `dbghelp.dll` forwards `MiniDumpWriteDump` to `dbgcore.dll`, and the call fails with
//!   `ERROR_NOACCESS` whenever an `EXCEPTION_POINTERS` is supplied - `ClientPointers` TRUE or FALSE,
//!   filter thread or second thread. So no exception record is passed, and the code and faulting
//!   address come from the trace instead;
//! * a `MINIDUMP_USER_STREAM_INFORMATION` comment stream also faults inside dbgcore, so the reason is
//!   in the file name instead.
//!
//! The once-only flag means a handler that faults cannot recurse.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicUsize, Ordering};

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> isize;
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThreadId() -> u32;
    fn GetTickCount64() -> u64;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *mut c_void,
        disposition: u32,
        flags: u32,
        template: isize,
    ) -> isize;
    fn WriteFile(h: isize, buf: *const u8, len: u32, written: *mut u32, overlapped: *mut c_void) -> i32;
    fn CloseHandle(h: isize) -> i32;
    fn GetLastError() -> u32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn CreateThread(
        sa: *mut c_void,
        stack: usize,
        start: Option<extern "system" fn(*mut c_void) -> u32>,
        param: *mut c_void,
        flags: u32,
        thread_id: *mut u32,
    ) -> isize;
    fn CreateEventW(sa: *mut c_void, manual_reset: i32, initial: i32, name: *const u16) -> isize;
    fn SetEvent(h: isize) -> i32;
    fn WaitForSingleObject(h: isize, millis: u32) -> u32;
    fn LoadLibraryW(name: *const u16) -> isize;
    fn GetProcAddress(module: isize, name: *const u8) -> *mut c_void;
    fn AddVectoredExceptionHandler(
        first: u32,
        handler: Option<extern "system" fn(*mut ExceptionPointers) -> i32>,
    ) -> *mut c_void;
    fn SetUnhandledExceptionFilter(
        filter: Option<extern "system" fn(*mut ExceptionPointers) -> i32>,
    ) -> *mut c_void;
}

const GENERIC_WRITE: u32 = 0x4000_0000;
/// Appends instead of writing at the file pointer. With plain `GENERIC_WRITE` every handle starts at
/// offset 0, so two agent builds in one process overwrite each other's lines.
const FILE_APPEND_DATA: u32 = 0x0000_0004;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const OPEN_ALWAYS: u32 = 4;
const CREATE_ALWAYS: u32 = 2;
const INVALID_HANDLE: isize = -1;

const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

const ACCESS_VIOLATION: u32 = 0xC000_0005;
const ILLEGAL_INSTRUCTION: u32 = 0xC000_001D;
const PRIVILEGED_INSTRUCTION: u32 = 0xC000_0096;
const STACK_OVERFLOW: u32 = 0xC000_00FD;

/// Offset of `Rip` inside an x64 `CONTEXT`. Everything before it is fixed-size, so this is stable.
const CONTEXT_RIP: usize = 0xF8;
const CONTEXT_RSP: usize = 0x98;

#[repr(C)]
pub struct ExceptionRecord {
    pub code: u32,
    pub flags: u32,
    pub record: *mut ExceptionRecord,
    pub address: *mut c_void,
    pub param_count: u32,
    pub params: [usize; 15],
}

#[repr(C)]
pub struct ExceptionPointers {
    pub exception_record: *mut ExceptionRecord,
    pub context_record: *mut u8,
}

#[repr(C)]
struct MinidumpExceptionInformation {
    thread_id: u32,
    exception_pointers: *mut ExceptionPointers,
    /// `BOOL`, not a pointer: the pointers in this structure are already server-side.
    client_pointers: i32,
}

type MiniDumpWriteDump = unsafe extern "system" fn(
    process: isize,
    pid: u32,
    file: isize,
    dump_type: u32,
    exception: *mut MinidumpExceptionInformation,
    user_streams: *mut c_void,
    callback: *mut c_void,
) -> i32;

/// MINIDUMP_TYPE flags. `NORMAL` already carries threads, stacks and the module list, which is what
/// identifies a faulting instruction; the extras add the kind of detail a bad write needs.
const MINIDUMP_NORMAL: u32 = 0x0000_0000;
const MINIDUMP_WITH_THREAD_INFO: u32 = 0x0000_1000;
const MINIDUMP_WITH_UNLOADED_MODULES: u32 = 0x0000_0020;

static TRACE_HANDLE: AtomicIsize = AtomicIsize::new(0);
static IN_TRACE: AtomicBool = AtomicBool::new(false);
static DUMPED: AtomicBool = AtomicBool::new(false);
static FIRST_CHANCE_LOGGED: AtomicU32 = AtomicU32::new(0);
static HOOK_BLOCK: AtomicUsize = AtomicUsize::new(0);
static HOOK_TARGET: AtomicUsize = AtomicUsize::new(0);
/// Set by the unhandled filter for the dump thread: the exception being handled, and the event the
/// dump thread signals when it is done.
static PENDING_EXCEPTION: AtomicUsize = AtomicUsize::new(0);
static DUMP_DONE: AtomicIsize = AtomicIsize::new(0);
/// `MiniDumpWriteDump`, resolved once at install time. Loading a DLL from a crash handler is both a
/// deadlock risk and, measured here, enough to make the dump call fail.
static DUMP_FN: AtomicUsize = AtomicUsize::new(0);

/// Where the trace and dumps go. Overridable so a test can point them at a temp directory instead
/// of next to the game's exe.
pub fn evidence_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("COOP_CRASH_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn trace_path() -> PathBuf {
    evidence_dir().join("coop_agent.trace")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Tells the crash handler which address ranges belong to our hook, so a fault inside them is named
/// instead of being reported as a bare address.
pub fn register_hook(block: usize, target: usize) {
    HOOK_BLOCK.store(block, Ordering::SeqCst);
    HOOK_TARGET.store(target, Ordering::SeqCst);
}

fn open_trace(truncate: bool) -> isize {
    let path = trace_path();
    let _ = std::fs::create_dir_all(evidence_dir());
    let name = wide(&path.to_string_lossy());
    let disposition = if truncate { CREATE_ALWAYS } else { OPEN_ALWAYS };
    unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_APPEND_DATA,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            disposition,
            0,
            0,
        )
    }
}

fn trace_handle() -> isize {
    let h = TRACE_HANDLE.load(Ordering::SeqCst);
    if h != 0 && h != INVALID_HANDLE {
        return h;
    }
    let h = open_trace(false);
    if h != INVALID_HANDLE {
        TRACE_HANDLE.store(h, Ordering::SeqCst);
    }
    h
}

fn put_bytes(buf: &mut [u8], at: &mut usize, bytes: &[u8]) {
    for &b in bytes {
        if *at < buf.len() {
            buf[*at] = b;
            *at += 1;
        }
    }
}

fn put_dec(buf: &mut [u8], at: &mut usize, mut v: u64) {
    let mut tmp = [0u8; 20];
    let mut n = 0;
    if v == 0 {
        put_bytes(buf, at, b"0");
        return;
    }
    while v > 0 && n < tmp.len() {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        put_bytes(buf, at, &tmp[n..n + 1]);
    }
}

fn put_hex(buf: &mut [u8], at: &mut usize, v: usize) {
    put_bytes(buf, at, b"0x");
    let mut started = false;
    for shift in (0..16).rev() {
        let nib = ((v >> (shift * 4)) & 0xF) as u8;
        if nib != 0 || started || shift == 0 {
            started = true;
            let c = if nib < 10 { b'0' + nib } else { b'a' + nib - 10 };
            put_bytes(buf, at, &[c]);
        }
    }
}

/// Appends one stage line to the trace. Safe to call from an exception handler: no allocation, no
/// locks, and it cannot recurse.
pub fn note(stage: &str) {
    if IN_TRACE.swap(true, Ordering::SeqCst) {
        return;
    }
    let h = trace_handle();
    if h != 0 && h != INVALID_HANDLE {
        let mut buf = [0u8; 256];
        let mut at = 0;
        put_bytes(&mut buf, &mut at, b"t=");
        put_dec(&mut buf, &mut at, unsafe { GetTickCount64() } / 1000);
        put_bytes(&mut buf, &mut at, b" ");
        put_bytes(&mut buf, &mut at, stage.as_bytes());
        put_bytes(&mut buf, &mut at, b"\n");
        let mut written = 0u32;
        unsafe { WriteFile(h, buf.as_ptr(), at as u32, &mut written, std::ptr::null_mut()) };
    }
    IN_TRACE.store(false, Ordering::SeqCst);
}

fn stage_violation(record: &ExceptionRecord, context: *mut u8) {
    let mut buf = [0u8; 256];
    let mut at = 0;
    put_bytes(&mut buf, &mut at, b"first-chance code=");
    put_hex(&mut buf, &mut at, record.code as usize);
    put_bytes(&mut buf, &mut at, b" at=");
    put_hex(&mut buf, &mut at, record.address as usize);
    if !context.is_null() {
        let rip = unsafe { std::ptr::read_unaligned(context.add(CONTEXT_RIP) as *const usize) };
        let rsp = unsafe { std::ptr::read_unaligned(context.add(CONTEXT_RSP) as *const usize) };
        put_bytes(&mut buf, &mut at, b" rip=");
        put_hex(&mut buf, &mut at, rip);
        put_bytes(&mut buf, &mut at, b" rsp=");
        put_hex(&mut buf, &mut at, rsp);

        let target = HOOK_TARGET.load(Ordering::SeqCst);
        let block = HOOK_BLOCK.load(Ordering::SeqCst);
        // The trampoline block and the patched function are the two places our own code runs.
        if target != 0 && rip >= target && rip < target + 0x100 {
            put_bytes(&mut buf, &mut at, b" IN-HOOK-TARGET");
        } else if block != 0 && rip >= block && rip < block + 0x1000 {
            put_bytes(&mut buf, &mut at, b" IN-HOOK-BLOCK");
        }
    }
    put_bytes(&mut buf, &mut at, b"\n");
    let mut written = 0u32;
    let h = trace_handle();
    if h != 0 && h != INVALID_HANDLE {
        unsafe { WriteFile(h, buf.as_ptr(), at as u32, &mut written, std::ptr::null_mut()) };
    }
}

extern "system" fn vectored(info: *mut ExceptionPointers) -> i32 {
    // Never swallows anything: the game's own handlers must still see every exception.
    if !info.is_null() {
        let pointers = unsafe { &*info };
        let record = pointers.exception_record;
        if !record.is_null() {
            let record = unsafe { &*record };
            let interesting = matches!(
                record.code,
                ACCESS_VIOLATION | ILLEGAL_INSTRUCTION | PRIVILEGED_INSTRUCTION | STACK_OVERFLOW
            );
            // A first-chance fault the game handles internally is normal during probing; logging
            // every one would grow without bound, so cap it.
            if interesting && FIRST_CHANCE_LOGGED.fetch_add(1, Ordering::Relaxed) < 32 {
                stage_violation(record, pointers.context_record);
            }
        }
    }
    EXCEPTION_CONTINUE_SEARCH
}

/// Resolves and caches the dump function. Call from a healthy thread, never from a handler.
pub fn preload() -> Result<(), String> {
    if DUMP_FN.load(Ordering::SeqCst) != 0 {
        return Ok(());
    }
    let (_, f) = dbghelp().ok_or("dbghelp.dll or its MiniDumpWriteDump export is unavailable")?;
    DUMP_FN.store(f as usize, Ordering::SeqCst);
    Ok(())
}

fn dump_function() -> Option<MiniDumpWriteDump> {
    let cached = DUMP_FN.load(Ordering::SeqCst);
    if cached != 0 {
        return Some(unsafe { std::mem::transmute::<usize, MiniDumpWriteDump>(cached) });
    }
    // Not preloaded: resolve now, and remember it, but the caller has been warned this is the fragile
    // path because a DLL load may happen at a bad moment.
    let (_, f) = dbghelp()?;
    DUMP_FN.store(f as usize, Ordering::SeqCst);
    Some(f)
}

fn dbghelp() -> Option<(isize, MiniDumpWriteDump)> {
    unsafe {
        let name = wide("dbghelp.dll");
        let module = LoadLibraryW(name.as_ptr());
        if module == 0 {
            return None;
        }
        let proc = GetProcAddress(module, c"MiniDumpWriteDump".as_ptr() as *const u8);
        if proc.is_null() {
            return None;
        }
            Some((module, std::mem::transmute::<*mut c_void, MiniDumpWriteDump>(proc)))
    }
}

/// Writes a minidump of this process. Returns the path so a caller can log it.
///
/// Only ever succeeds once per process: a second call after a crash would be running on a corrupted
/// heap, and the value of a second dump is far below the risk.
pub fn write_dump(reason: &str, exception: *mut ExceptionPointers) -> Result<PathBuf, String> {
    // No MINIDUMP_WITH_DATA_SEGS: it adds the writable data section of every loaded module, which on
    // this game took a requested dump from tens of megabytes to 90 MB. Thread stacks, thread info and
    // the module list are what identify a hang, and those stay.
    let kind = MINIDUMP_NORMAL | MINIDUMP_WITH_THREAD_INFO | MINIDUMP_WITH_UNLOADED_MODULES;
    dump_impl(reason, exception, kind)
}

/// The most conservative dump there is: normal type, no extra flags. Kept separate so a failing
/// richer dump can be narrowed down instead of guessed at.
pub fn write_dump_plain(reason: &str) -> Result<PathBuf, String> {
    dump_impl(reason, std::ptr::null_mut(), MINIDUMP_NORMAL)
}

/// Dump with an explicit `MINIDUMP_TYPE`. Exists so a failure can be bisected down to a single flag.
pub fn write_dump_custom(reason: &str, kind: u32) -> Result<PathBuf, String> {
    dump_impl(reason, std::ptr::null_mut(), kind)
}

/// A short, filename-safe token from the reason, so the file name alone says why it exists.
fn reason_tag(reason: &str) -> String {
    let mut tag: String = reason
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(24)
        .collect();
    if tag.is_empty() {
        tag.push_str("dump");
    }
    tag
}

/// Writes a minidump of this process. Returns the path so a caller can log it.
///
/// No user stream is attached, deliberately. `dbghelp.dll` forwards `MiniDumpWriteDump` to
/// `dbgcore.dll`, and on this Windows build dbgcore faults on *any* `MINIDUMP_USER_STREAM_INFORMATION`
/// (access violation inside dbgcore, `ERROR_NOACCESS` from the filter). Plain and flagged dumps both
/// work, so the reason and the breadcrumb go into the file name and into `coop_agent.trace` next to
/// the dump instead: the trace is written by us, cannot fault, and survives independently.
fn dump_impl(reason: &str, exception: *mut ExceptionPointers, kind: u32) -> Result<PathBuf, String> {
    let dump = dump_function().ok_or("dbghelp.dll is unavailable, cannot write a dump")?;

    let pid = unsafe { GetCurrentProcessId() };
    let path = evidence_dir().join(format!(
        "coop_dump_{pid}_{}_{}.dmp",
        unsafe { GetTickCount64() } / 1000,
        reason_tag(reason)
    ));
    let name = wide(&path.to_string_lossy());
    let file = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ,
            std::ptr::null_mut(),
            CREATE_ALWAYS,
            0,
            0,
        )
    };
    if file == INVALID_HANDLE {
        return Err("could not create the dump file".into());
    }

    // `client_pointers` must be TRUE: the pointers live in the caller's address space and dbghelp
    // has to marshal them itself. With it FALSE dbghelp reports ERROR_NOACCESS (998) and writes
    // nothing - measured, not assumed.
    let mut info = MinidumpExceptionInformation {
        thread_id: unsafe { GetCurrentThreadId() },
        exception_pointers: exception,
        client_pointers: 1,
    };
    let exception_arg = if exception.is_null() { std::ptr::null_mut() } else { &mut info };

    // A real handle with the access rights dbghelp needs, rather than the pseudo-handle: dbghelp
    // duplicates whatever it is given.
    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    const PROCESS_VM_READ: u32 = 0x0010;
    let self_handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
    let process = if self_handle != 0 { self_handle } else { unsafe { GetCurrentProcess() } };

    let ok = unsafe {
        dump(
            process,
            pid,
            file,
            kind,
            exception_arg,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if self_handle != 0 {
        unsafe { CloseHandle(self_handle) };
    }
    unsafe { CloseHandle(file) };
    if ok == 0 {
        let err = unsafe { GetLastError() };
        let _ = std::fs::remove_file(&path);
        return Err(format!(
            "MiniDumpWriteDump failed (Win32 {err}, kind {kind:#X}, process handle {process:#X}) for {path:?}"
        ));
    }
    Ok(path)
}

/// Writes the dump on a thread of its own, then signals the waiting filter.
///
/// `MiniDumpWriteDump` fails with `ERROR_NOACCESS` when it is called from the exception dispatcher on
/// the faulting thread - measured here, not assumed: the same call succeeds from a healthy thread and
/// from a second thread. That is why the filter hands the work over instead of doing it inline.
extern "system" fn dump_thread_entry(_param: *mut c_void) -> u32 {
    let pointers = PENDING_EXCEPTION.load(Ordering::SeqCst) as *mut ExceptionPointers;
    let code = if pointers.is_null() {
        0
    } else {
        let record = unsafe { (*pointers).exception_record };
        if record.is_null() { 0 } else { unsafe { (*record).code } }
    };
    let reason = format!("unhandled exception {code:#X}");
    note("dump thread started");
    // No exception info: every thread's stack and registers are already in a normal minidump, and
    // passing the record from a second thread while the process is crashed is exactly what fails.
    // No exception record is passed. On this build dbghelp/dbgcore fails with ERROR_NOACCESS (998)
    // and writes nothing whenever it is given one, with `ClientPointers` either TRUE or FALSE, from
    // the filter thread or from this one. Every thread's registers and stack are in a normal dump
    // anyway, and `coop_agent.trace` names the exception code and the faulting address.
    let _ = pointers;
    match write_dump(&reason, std::ptr::null_mut()) {
        Ok(path) => note(&format!("dump written: {}", path.display())),
        Err(e) => note(&format!("dump FAILED: {e}")),
    }
    let event = DUMP_DONE.load(Ordering::SeqCst);
    if event != 0 {
        unsafe { SetEvent(event) };
    }
    0
}

extern "system" fn unhandled(info: *mut ExceptionPointers) -> i32 {
    // Once only: a second attempt would be running on a heap that may already be corrupt.
    if DUMPED.swap(true, Ordering::SeqCst) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let code = if info.is_null() {
        0
    } else {
        let record = unsafe { (*info).exception_record };
        if record.is_null() {
            0
        } else {
            unsafe { (*record).code }
        }
    };
    note(&format!("UNHANDLED code={code:#X}, writing a dump"));

    PENDING_EXCEPTION.store(info as usize, Ordering::SeqCst);
    // Manual-reset and nameless: nobody else can observe it, and it dies with the process.
    let event = unsafe { CreateEventW(std::ptr::null_mut(), 1, 0, std::ptr::null()) };
    DUMP_DONE.store(event, Ordering::SeqCst);
    let thread = unsafe {
        CreateThread(
            std::ptr::null_mut(),
            0,
            Some(dump_thread_entry),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
        )
    };
    if thread == 0 || event == 0 {
        // No thread to hand over to: try inline, and accept that it may fail for the reason above.
        note("no dump thread; attempting an inline dump");
        match write_dump(&format!("unhandled exception {code:#X}"), info) {
            Ok(path) => note(&format!("dump written: {}", path.display())),
            Err(e) => note(&format!("dump FAILED: {e}")),
        }
    } else {
        // Bounded: a crash path must never hang the process, and the process is going away anyway.
        let waited = unsafe { WaitForSingleObject(event, 15_000) };
        if waited != 0 {
            note(&format!("dump thread did not finish (wait={waited}); continuing"));
        }
    }
    // Let the game's own handler and Windows Error Reporting still run; we only wanted the dump.
    EXCEPTION_CONTINUE_SEARCH
}

/// Installs the handler. Call once, first thing in the worker thread.
///
/// The trace is appended to, never truncated: an earlier agent build in the same process may still be
/// writing to it, and the history before a crash is the thing being preserved.
pub fn install() {
    let h = open_trace(false);
    if h != INVALID_HANDLE {
        TRACE_HANDLE.store(h, Ordering::SeqCst);
    }
    let pid = unsafe { GetCurrentProcessId() };
    note(&format!("crash handler installed, pid {pid}, evidence in {}", evidence_dir().display()));
    match preload() {
        Ok(()) => note("minidump function resolved"),
        Err(e) => note(&format!("minidump unavailable, dumps will be skipped: {e}")),
    }
    unsafe {
        AddVectoredExceptionHandler(1, Some(vectored));
        SetUnhandledExceptionFilter(Some(unhandled));
    }
}

/// One line for the log, so a reader knows where to look after a crash.
pub fn summary() -> String {
    format!("trace {} | dumps in {}", trace_path().display(), evidence_dir().display())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts the file is a minidump that actually contains threads and modules, rather than
    /// trusting a size threshold: the dump type changed once already and a byte count would have
    /// passed on a file that had lost its thread list.
    fn assert_real_dump(path: &std::path::Path) {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"));
        assert!(bytes.len() > 16, "dump is empty");
        assert_eq!(&bytes[..4], b"MDMP", "not a minidump");

        let read_u32 = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let streams = read_u32(8) as usize;
        let dir = read_u32(12) as usize;
        assert!(streams > 0 && dir > 0, "no stream directory");

        let mut threads = 0usize;
        let mut modules = 0usize;
        for i in 0..streams {
            let entry = dir + i * 12;
            let kind = read_u32(entry);
            let size = read_u32(entry + 4) as usize;
            let rva = read_u32(entry + 8) as usize;
            if size == 0 {
                continue;
            }
            match kind {
                3 => threads = read_u32(rva) as usize,
                4 => modules = read_u32(rva) as usize,
                _ => {}
            }
        }
        assert!(threads > 0, "dump has no threads");
        assert!(modules > 0, "dump has no modules");
    }

    #[test]
    fn dec_and_hex_render_expected_text() {
        let mut buf = [0u8; 64];
        let mut at = 0;
        put_dec(&mut buf, &mut at, 0);
        assert_eq!(&buf[..at], b"0");
        at = 0;
        put_dec(&mut buf, &mut at, 1234567);
        assert_eq!(&buf[..at], b"1234567");
        at = 0;
        put_hex(&mut buf, &mut at, 0x1422_98080);
        assert_eq!(&buf[..at], b"0x142298080");
        at = 0;
        put_hex(&mut buf, &mut at, 0);
        assert_eq!(&buf[..at], b"0x0");
    }

    /// The trace must survive the process that wrote it: a crash handler can only help if the last
    /// line is on disk afterwards.
    #[test]
    fn trace_survives_a_fatal_fault_and_leaves_a_dump() {
        let dir = std::env::temp_dir().join(format!("coop_crash_evidence_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let exe = std::env::current_exe().unwrap();
        let child = std::process::Command::new(exe)
            .args(["--exact", "crash::tests::fault_on_purpose", "--nocapture"])
            .env("COOP_CRASH_SELFTEST", "1")
            .env("COOP_CRASH_DIR", &dir)
            .output()
            .unwrap();

        assert!(
            !child.status.success(),
            "the child was supposed to die, but it exited with {:?}",
            child.status
        );
        let trace = std::fs::read_to_string(dir.join("coop_agent.trace")).unwrap_or_default();
        assert!(
            trace.contains("about to fault on purpose"),
            "trace did not keep the last stage: {trace:?}"
        );
        assert!(
            trace.contains("UNHANDLED"),
            "the unhandled filter did not run: {trace:?}"
        );
        let dumps: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "dmp").unwrap_or(false))
            .collect();
        assert_eq!(dumps.len(), 1, "expected exactly one dump, found {}", dumps.len());
        assert_real_dump(&dumps[0].path());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The dump call on its own, from a healthy thread. If this fails the problem is our arguments,
    /// not the fact that we were called from inside an exception handler.
    #[test]
    fn dump_from_a_healthy_thread_is_a_real_dump() {
        let dir = std::env::temp_dir().join(format!("coop_crash_healthy_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("COOP_CRASH_DIR", &dir);
        note("healthy-thread dump test");

        match write_dump("healthy thread", std::ptr::null_mut()) {
            Ok(path) => {
                assert_real_dump(&path);
            }
            Err(e) => panic!("{e}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Runs only in the child process spawned above: installs the handler, breadcrumbs, then faults.
    #[test]
    fn fault_on_purpose() {
        if std::env::var("COOP_CRASH_SELFTEST").ok().as_deref() != Some("1") {
            return;
        }
        install();
        register_hook(0x1000_0000, 0x1422_98080);
        note("about to fault on purpose");
        unsafe {
            std::ptr::write_volatile(0x10 as *mut u8, 1);
        }
        unreachable!("the fault above must not return");
    }
}
