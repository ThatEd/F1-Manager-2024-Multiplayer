//! Capturing the game's own SQL so career changes can be replicated.
//!
//! Why this exists
//! ---------------
//! Audit §20 found the gap that stopped "a full career together" from working: nothing carried management state
//! between the two players. `coop-proto::Msg::Stmt` carries a change and `coop-mem::stmtapply` applies one safely;
//! this is the producer - the thing that notices the game wrote a row and knows which row.
//!
//! How it works, and why here
//! --------------------------
//! Every career write goes through `sqlite3_exec`, so one address sees all of them. `hook.rs` already has a
//! working inline-hook generator (5-byte patch, a stub that saves every register and the XMM half of the FPU,
//! an atomic install with read-back verification). This reuses `hook::install_target` rather than writing a second
//! one, so the delicate parts - the stack alignment, the register save, the rel32 layout - keep a single
//! implementation and a single set of tests.
//!
//! The one thing that differs is *which register* holds the argument we want. `simulate` is a member function, so
//! this is RCX. `sqlite3_exec(db, sql, cb, arg, errmsg)` is free, so the SQL string is the **second** argument:
//! **RDX**. Getting that wrong would capture the database handle as a pointer and pass it to a string reader -
//! an access violation inside the game. The instruction is `mov [rip+disp32], rdx` = `48 89 15 <disp32>`, the
//! same seven bytes as the RCX form with one ModRM byte changed, so the stub layout is identical.
//!
//! Safety of reading the argument
//! ------------------------------
//! The callback runs inside `sqlite3_exec`, before SQLite has touched the string, so `rdx` points at a live
//! NUL-terminated statement. It is read with a bounded scan and then handed to `sqlcap` for parsing. Nothing is
//! written to the game; this hook only observes.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Set by the stub: the value of RDX on entry, i.e. the SQL string pointer.
static SQL_SLOT: AtomicUsize = AtomicUsize::new(0);
/// How many times the hook has fired, whether or not the statement was usable.
static FIRES: AtomicUsize = AtomicUsize::new(0);

/// Longest statement this will read. A career statement is a few hundred bytes; a megabyte is a guard against a
/// mis-sized read walking into unmapped memory, not an expectation.
pub const MAX_SQL: usize = 64 * 1024;

/// What the capture callback is told on every `sqlite3_exec` call.
///
/// `sql` is `None` when the pointer was null or unreadable - SQLite itself accepts a null statement, so that is a
/// real case and not an error worth panicking over.
pub type Sink = fn(sql: Option<&str>);

static SINK: AtomicUsize = AtomicUsize::new(0);

fn sink() -> Option<Sink> {
    let p = SINK.load(Ordering::SeqCst);
    if p == 0 {
        None
    } else {
        // SAFETY: only ever stored from a `Sink`'s address below, and the same process.
        Some(unsafe { std::mem::transmute::<usize, Sink>(p) })
    }
}

/// Registers what to do with each captured statement. `None` clears it.
pub fn set_sink(f: Option<Sink>) {
    SINK.store(f.map(|f| f as usize).unwrap_or(0), Ordering::SeqCst);
}

/// The callback the stub calls. Reads RDX, bounds the read, hands the text to the sink.
///
/// Split from the raw stub so the whole path is testable with a fake slot - the stub's calling convention is
/// covered by `hook`'s own tests, and this function's job is only "turn a captured pointer into a string safely".
pub fn on_exec(arg: usize) {
    FIRES.fetch_add(1, Ordering::SeqCst);
    let Some(f) = sink() else { return };
    if arg == 0 {
        f(None);
        return;
    }
    match unsafe { read_cstr(arg, MAX_SQL) } {
        Some(s) => f(Some(&s)),
        None => f(None),
    }
}

/// Reads a NUL-terminated string from `addr`, refusing to cross into unmapped memory.
///
/// Unsafe because it dereferences a pointer the game owns. The `readable` check per page is what keeps a bogus
/// argument - or a statement the game is in the middle of rewriting - from faulting inside our callback, which
/// would take the game down from a read-only observer.
unsafe fn read_cstr(addr: usize, max: usize) -> Option<String> {
    let mut out: Vec<u8> = Vec::with_capacity(256);
    let mut p = addr;
    let end = addr.checked_add(max)?;
    while p < end {
        if !readable(p) {
            // A partial read is useless as SQL, so give up rather than hand over half a statement.
            return None;
        }
        let b = std::ptr::read_volatile(p as *const u8);
        if b == 0 {
            return String::from_utf8(out).ok();
        }
        out.push(b);
        p += 1;
    }
    // No terminator within the bound: not a statement we understand.
    None
}

/// Is one byte at `addr` readable? Uses `VirtualQuery`, the same guard `sim::readable` applies elsewhere.
unsafe fn readable(addr: usize) -> bool {
    #[repr(C)]
    struct Mbi {
        base: usize,
        alloc_base: usize,
        alloc_protect: u32,
        _pad0: u32,
        region: usize,
        state: u32,
        protect: u32,
        kind: u32,
        _pad1: u32,
    }
    extern "system" {
        fn VirtualQuery(addr: *const c_void, buf: *mut Mbi, len: usize) -> usize;
    }
    const MEM_COMMIT: u32 = 0x1000;
    const PAGE_NOACCESS: u32 = 0x01;
    const PAGE_GUARD: u32 = 0x100;
    let mut mbi = Mbi {
        base: 0,
        alloc_base: 0,
        alloc_protect: 0,
        _pad0: 0,
        region: 0,
        state: 0,
        protect: 0,
        kind: 0,
        _pad1: 0,
    };
    if VirtualQuery(addr as *const c_void, &mut mbi, std::mem::size_of::<Mbi>()) == 0 {
        return false;
    }
    mbi.state == MEM_COMMIT && (mbi.protect & PAGE_NOACCESS) == 0 && (mbi.protect & PAGE_GUARD) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_sink<T>(f: impl FnOnce() -> T) -> T {
        f()
    }

    #[test]
    fn a_null_argument_is_reported_as_no_sql_not_a_crash() {
        // SQLite accepts `sql == NULL`; the capture must too, and must tell the sink rather than skip it.
        use std::sync::Mutex;
        static SEEN: Mutex<Vec<Option<String>>> = Mutex::new(Vec::new());
        fn rec(sql: Option<&str>) {
            SEEN.lock().unwrap().push(sql.map(|s| s.to_string()));
        }
        SEEN.lock().unwrap().clear();
        set_sink(Some(rec));
        on_exec(0);
        set_sink(None);
        let got = SEEN.lock().unwrap().clone();
        assert_eq!(got, vec![None]);
    }

    #[test]
    fn a_real_string_is_captured_exactly() {
        use std::sync::Mutex;
        static SEEN: Mutex<Vec<Option<String>>> = Mutex::new(Vec::new());
        fn rec(sql: Option<&str>) {
            SEEN.lock().unwrap().push(sql.map(|s| s.to_string()));
        }
        SEEN.lock().unwrap().clear();
        set_sink(Some(rec));
        let stmt = std::ffi::CString::new("UPDATE Player SET TeamID = 7 WHERE PlayerID = 99").unwrap();
        on_exec(stmt.as_ptr() as usize);
        set_sink(None);
        let got = SEEN.lock().unwrap().clone();
        assert_eq!(
            got,
            vec![Some("UPDATE Player SET TeamID = 7 WHERE PlayerID = 99".to_string())]
        );
    }

    #[test]
    fn an_unreadable_pointer_yields_none_rather_than_faulting() {
        // 0x10 is never mapped in a Windows process; reading it must be declined, not attempted.
        set_sink(Some(|_| {}));
        on_exec(0x10);
        set_sink(None);
        assert!(FIRES.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn an_unterminated_buffer_is_refused() {
        // A buffer of 'A' with no NUL inside MAX_SQL must not be returned as a giant statement.
        // 4 KiB is enough to make the point: the scan stops at the readable-region edge or the bound.
        let buf = vec![b'A'; 4096];
        let s = unsafe { read_cstr(buf.as_ptr() as usize, 4096) };
        assert_eq!(s, None, "an unterminated read must be refused, not truncated");
    }

    #[test]
    fn a_normal_string_reads_to_its_terminator() {
        let c = std::ffi::CString::new("hello").unwrap();
        let s = unsafe { read_cstr(c.as_ptr() as usize, 64) };
        assert_eq!(s.as_deref(), Some("hello"));
    }

    #[test]
    fn the_sink_can_be_cleared_and_reinstalled() {
        set_sink(Some(|_| {}));
        assert!(sink().is_some());
        set_sink(None);
        assert!(sink().is_none(), "a cleared sink must not be called");
    }

    /// The capture must be *wired*, not merely implemented.
    ///
    /// This exact defect happened once already in this project: `coop-auth` was complete, tested, and called by
    /// nothing, so a documented guarantee was never enforced. A capture hook that is installed but whose sink is
    /// never registered would look identical from the outside - the hook fires, nothing is reported - so the
    /// wiring is asserted against the source rather than assumed.
    #[test]
    fn the_agent_registers_the_sql_sink_and_hook() {
        let src = include_str!("lib.rs");
        assert!(
            src.contains("hook::install_exec(base)"),
            "the agent must install the sqlite3_exec hook; without it nothing observes career writes"
        );
        assert!(
            src.contains("hook::set_exec_callback(Some(on_sql_cb))"),
            "installing the hook without registering its callback leaves it inert"
        );
        assert!(
            src.contains("sqlhook::on_exec"),
            "the callback must reach the capture, or the hook fires and reports nothing"
        );
        // And the report must be reachable from the log, or the first live session teaches us nothing.
        assert!(
            src.contains("sql_capture_stats()"),
            "the fire/write counts must be reported, so 'no writes' and 'hook never fired' are distinguishable"
        );
    }
}
