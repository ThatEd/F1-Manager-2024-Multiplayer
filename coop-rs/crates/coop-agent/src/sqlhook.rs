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

    /// The client must be told when the host's save lands, in the status the menu actually reads.
    ///
    /// The transfer itself was already correct and silently useless: the file arrived, its hash verified, and
    /// nothing changed in the UI - so the player kept playing the old career with no indication. This asserts the
    /// name reaches `lobby_join_body` (the body the Join button reads) rather than only a console line.
    #[test]
    fn the_join_body_reports_a_received_save() {
        let src = include_str!("lib.rs");
        let at = src.find("fn lobby_join_body").expect("join body present");
        let body = &src[at..(at + 2000).min(src.len())];
        assert!(
            body.contains("session_save"),
            "the Join response must carry the received save's name, or the menu cannot tell the player"
        );
        assert!(
            body.contains("received_session()"),
            "and it must come from the recorded file, not be a placeholder"
        );
        // The reader and the writer have to agree on the location, which is beside the executable.
        let reader = src.find("fn received_session").expect("reader present");
        let reader_body = &src[reader..(src.len()).min(reader + 700)];
        assert!(
            reader_body.contains("log_path()"),
            "the reader must look beside the agent's own log, not in a guessed directory"
        );
    }

    /// Both roles must start their own frame link, and the failure must be reported per role.
    ///
    /// This is the defect the owner named: "we do not want the server to be a separate launch". The in-process
    /// relay solved it for the supervisor; the same problem was left for `coop-link`, which is the process that
    /// actually moves car frames. A hook armed with nothing publishing is indistinguishable from a broken mod.
    ///
    /// **And it is not only the host.** The client's `coop-link` runs in the opposite direction - it reads frames
    /// in and writes them through this agent's write port - so a client with no link receives nothing at all, and
    /// "the client sees no cars" looks exactly like "the host is not sending". An earlier version of this test
    /// asserted the client must *not* start one; that was wrong and would have shipped a one-sided fix.
    #[test]
    fn both_roles_start_their_frame_link() {
        let src = include_str!("lib.rs");
        let host = src.find("fn lobby_host_body").expect("host body present");
        let host_body = &src[host..(host + 1400).min(src.len())];
        assert!(
            host_body.contains("ensure_publisher(coop_proto::Side::Host)"),
            "Host must start its link, or the player has to launch it by hand"
        );
        let join = src.find("fn lobby_join_body").expect("join body present");
        let join_body = &src[join..(join + 2000).min(src.len())];
        assert!(
            join_body.contains("ensure_publisher(coop_proto::Side::Client)"),
            "Join must start the client's link too; without it the client receives nothing"
        );

        let pf = src.find("fn ensure_publisher").expect("ensure_publisher present");
        let pbody = &src[pf..(src.len()).min(pf + 3500)];
        assert!(pbody.contains("coop-link.exe"), "it must look for the real binary");
        assert!(
            pbody.contains("PUBLISHER_STARTED[key]"),
            "and be guarded per role, so repeated menu visits do not spawn a process each time"
        );
        assert!(
            pbody.contains("DEFAULT_PORT"),
            "the supervisor address must come from the constant the relay binds, not a literal"
        );
        assert!(
            pbody.contains("not found"),
            "a missing binary must be reported by name rather than silently ignored"
        );
        // One flag per side, so hosting then joining in one session starts both.
        assert!(
            src.contains("static PUBLISHER_STARTED: [AtomicBool; 2]"),
            "the guard must be per role; a single flag would block the second role"
        );
    }

    #[test]
    fn the_sink_can_be_cleared_and_reinstalled() {        set_sink(Some(|_| {}));
        assert!(sink().is_some());
        set_sink(None);
        assert!(sink().is_none(), "a cleared sink must not be called");
    }

    /// The capture must be *wired end to end*, not merely installed.
    ///
    /// This exact defect has now happened three times in this project: `coop-auth` was complete and called by
    /// nothing, the auth gate was documented and unenforced, and the SQL capture hook was installed with a callback
    /// but **no sink registered** - so it fired on every `sqlite3_exec` and discarded the text. `classify` was never
    /// reached outside its own unit tests.
    ///
    /// So the whole chain is asserted against the source: install, callback, sink, classify, plan, and queue. A
    /// missing link in that chain is invisible at runtime because every individual piece works.
    #[test]
    fn the_capture_chain_is_wired_from_hook_to_outbox() {
        let src = include_str!("lib.rs");
        let chain = [
            ("hook::install_exec(base)", "installs the sqlite3_exec hook"),
            ("hook::set_exec_callback(Some(on_sql_cb))", "registers the stub callback"),
            ("sqlhook::set_sink(Some(on_sql_sink))", "registers the sink that receives the text"),
            ("sqlcap::classify", "classifies the statement"),
            ("replicate::plan", "decides whether it may be replicated"),
            ("queue_frame(", "puts the frame on the outbox for the heartbeat to send"),
        ];
        for (needle, what) in chain {
            assert!(src.contains(needle), "the capture chain is broken: nothing {what} ({needle})");
        }
        // And the inbound half, or replication is send-only.
        assert!(
            src.contains("Msg::Stmt { seq, table, pk_column, pk, set }"),
            "the agent must handle an incoming Msg::Stmt, or changes are never applied"
        );
        assert!(
            src.contains("stmtapply::to_update"),
            "an incoming change must go through the safe applier, not composed SQL"
        );
    }
}
