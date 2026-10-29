//! Exercises the loop stub at the volume that broke the live attempt.
//!
//! The live transplant failed around its 220th statement because it created a remote thread per
//! statement. This test drives 600 statements through ONE worker thread against a real SQLite
//! (`winsqlite3.dll`) — the same workload, same order, minus the game. A wrong handshake fails here in
//! a second instead of crashing a running game.
//!
//! The stub comes from `coop_mem::loopstub::build_loop_stub()`, the same generator that ships, so this
//! cannot drift from a stale copy.

#![cfg(windows)]

use coop_mem::loopstub::{build_loop_stub, Block, BLOCK_SIZE};
use std::ffi::c_void;

#[link(name = "kernel32")]
extern "system" {
    fn VirtualAlloc(addr: *mut c_void, size: usize, alloc: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(addr: *mut c_void, size: usize, free: u32) -> i32;
    fn GetCurrentProcess() -> isize;
    fn FlushInstructionCache(h: isize, base: *const c_void, size: usize) -> i32;
    fn LoadLibraryW(name: *const u16) -> isize;
    fn GetProcAddress(module: isize, name: *const u8) -> *mut c_void;
}

const MEM_COMMIT_RESERVE: u32 = 0x3000;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const MEM_RELEASE: u32 = 0x8000;

type Sqlite3Exec =
    unsafe extern "system" fn(usize, *const u8, usize, usize, *mut usize) -> i32;
type Sqlite3Open = unsafe extern "system" fn(*const u8, *mut usize) -> i32;
type Sqlite3Close = unsafe extern "system" fn(usize) -> i32;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The row callback: emits `value\x1F...\x1E` per row.
const CALLBACK: [u8; 101] = [
    0x56, 0x57, 0x48, 0x8B, 0x79, 0x28, 0x4C, 0x8B, 0x51, 0x30, 0x49, 0x83, 0xEA, 0x02, 0x45, 0x31,
    0xDB, 0x41, 0x39, 0xD3, 0x7D, 0x2E, 0x4B, 0x8B, 0x34, 0xD8, 0x48, 0x85, 0xF6, 0x74, 0x15, 0x8A,
    0x06, 0x84, 0xC0, 0x74, 0x0F, 0x4C, 0x39, 0xD7, 0x73, 0x2C, 0x88, 0x07, 0x48, 0xFF, 0xC7, 0x48,
    0xFF, 0xC6, 0xEB, 0xEB, 0x4C, 0x39, 0xD7, 0x73, 0x1D, 0xC6, 0x07, 0x1F, 0x48, 0xFF, 0xC7, 0x41,
    0xFF, 0xC3, 0xEB, 0xCD, 0xC6, 0x07, 0x1E, 0x48, 0xFF, 0xC7, 0xC6, 0x07, 0x00, 0x48, 0x89, 0x79,
    0x28, 0x31, 0xC0, 0x5F, 0x5E, 0xC3, 0xC6, 0x07, 0x00, 0x48, 0x89, 0x79, 0x28, 0xB8, 0x01, 0x00,
    0x00, 0x00, 0x5F, 0x5E, 0xC3,
];

fn load() -> Option<(Sqlite3Exec, Sqlite3Open, Sqlite3Close)> {
    unsafe {
        let module = LoadLibraryW(wide("winsqlite3.dll").as_ptr());
        if module == 0 {
            return None;
        }
        let exec = GetProcAddress(module, c"sqlite3_exec".as_ptr() as *const u8);
        let open = GetProcAddress(module, c"sqlite3_open".as_ptr() as *const u8);
        let close = GetProcAddress(module, c"sqlite3_close".as_ptr() as *const u8);
        if exec.is_null() || open.is_null() || close.is_null() {
            return None;
        }
        Some((
            std::mem::transmute::<*mut c_void, Sqlite3Exec>(exec),
            std::mem::transmute::<*mut c_void, Sqlite3Open>(open),
            std::mem::transmute::<*mut c_void, Sqlite3Close>(close),
        ))
    }
}

/// Converts callback output to rows, dropping the separator the callback writes after the last column.
fn parse_rows(raw: &[u8]) -> Vec<Vec<String>> {
    let text = String::from_utf8_lossy(raw);
    let mut out = Vec::new();
    for row in text.split('\u{1e}') {
        if row.is_empty() {
            continue;
        }
        let mut fields: Vec<String> = row.split('\u{1f}').map(|s| s.to_string()).collect();
        if fields.last().map(|s| s.is_empty()).unwrap_or(false) {
            fields.pop();
        }
        out.push(fields);
    }
    out
}

#[test]
fn six_hundred_statements_through_one_thread() {
    let (exec, open, close) = match load() {
        Some(f) => f,
        None => {
            eprintln!("skipping: winsqlite3.dll unavailable");
            return;
        }
    };

    unsafe {
        let mem = VirtualAlloc(
            std::ptr::null_mut(),
            0x10_0000,
            MEM_COMMIT_RESERVE,
            PAGE_EXECUTE_READWRITE,
        ) as usize;
        assert_ne!(mem, 0, "VirtualAlloc failed");

        let stub = mem;
        let callback = mem + 0x200;
        let block_addr = mem + 0x400;
        let sqlbuf = mem + 0x800;
        let out = mem + 0x2_0000;
        let out_end = out + 0x2_0000;

        let code = build_loop_stub();
        std::ptr::copy_nonoverlapping(code.as_ptr(), stub as *mut u8, code.len());
        std::ptr::copy_nonoverlapping(CALLBACK.as_ptr(), callback as *mut u8, CALLBACK.len());
        FlushInstructionCache(GetCurrentProcess(), stub as *const c_void, code.len());
        FlushInstructionCache(GetCurrentProcess(), callback as *const c_void, CALLBACK.len());

        let mut db: usize = 0;
        assert_eq!((open)(c":memory:".as_ptr() as *const u8, &mut db), 0, "open failed");
        let mut errmsg: usize = 0;
        let ddl = c"CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)";
        assert_eq!((exec)(db, ddl.as_ptr() as *const u8, 0, 0, &mut errmsg), 0, "ddl failed");

        let block = Block::new(block_addr);
        block.prepare(exec as usize, db, callback, out, out_end);

        // One worker thread for all 600 statements. This is the whole point: the live attempt created
        // a thread per statement and died at ~220.
        let entry: extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(stub);
        let worker = std::thread::spawn(move || entry(block_addr as *mut c_void));

        for i in 0..600 {
            let stmt = format!("INSERT INTO t(v) VALUES('row{i}')");
            std::ptr::copy_nonoverlapping(stmt.as_ptr(), sqlbuf as *mut u8, stmt.len());
            *(sqlbuf as *mut u8).add(stmt.len()) = 0;
            block.reset_cursor(out);
            let (rc, _) = block.run_once(sqlbuf);
            assert_eq!(rc, 0, "statement {i} failed with sqlite rc {rc}");
        }

        block.quit();
        worker.join().expect("worker thread panicked");

        // Prove the data landed, via the same callback path.
        let q = c"SELECT count(*) FROM t";
        std::ptr::copy_nonoverlapping(q.as_ptr() as *const u8, sqlbuf as *mut u8, q.to_bytes_with_nul().len());
        block.reset_cursor(out);
        assert_eq!((exec)(db, sqlbuf as *const u8, callback, block_addr, std::ptr::null_mut()), 0);
        let cursor = block.read(coop_mem::loopstub::OFF_CURSOR) as usize;
        let raw = std::slice::from_raw_parts(out as *const u8, cursor - out);
        let rows = parse_rows(raw);
        eprintln!("count row: {rows:?}");
        assert_eq!(rows.first().and_then(|r| r.first()).map(|s| s.as_str()), Some("600"));

        (close)(db);
        VirtualFree(mem as *mut c_void, 0, MEM_RELEASE);
    }
}

#[test]
fn parse_drops_the_trailing_separator() {
    let rows = parse_rows(b"7\x1fbravo\x1f\x1e");
    assert_eq!(rows[0], vec!["7", "bravo"]);
    // A genuinely empty last column is dropped too - a documented limitation of the callback, which
    // writes a separator after every column. Stated here rather than hidden.
    let rows = parse_rows(b"a\x1f\x1e");
    assert_eq!(rows[0], vec!["a"]);
}

#[test]
fn block_is_large_enough() {
    assert!(BLOCK_SIZE >= 0x48);
}
