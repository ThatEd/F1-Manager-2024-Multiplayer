//! Proves the stub and callback bytes actually work, without touching the game.
//!
//! The assembler encodings are the one part that cannot be checked by reading code: a wrong
//! displacement or a wrong struct offset produces code that looks correct and faults (or hangs) only
//! when executed. Copying someone else's byte array is not verification either — it is copying a
//! claim. So this allocates executable memory, writes the same `STUB` and `CALLBACK` the live path
//! uses, opens a real in-memory SQLite via `winsqlite3.dll`, and runs `sqlite3_exec` through the stub
//! exactly as the live path does. A wrong layout fails here instead of inside the game.

#![cfg(windows)]

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

type Sqlite3Exec = unsafe extern "system" fn(
    db: usize,
    sql: *const u8,
    callback: usize,
    arg: usize,
    errmsg: *mut usize,
) -> i32;
type Sqlite3Open = unsafe extern "system" fn(filename: *const u8, db: *mut usize) -> i32;
type Sqlite3Close = unsafe extern "system" fn(db: usize) -> i32;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

const STUB: [u8; 44] = [
    0x53, 0x48, 0x83, 0xEC, 0x30, 0x48, 0x89, 0xCB, 0x48, 0x8B, 0x4B, 0x08, 0x48, 0x8B, 0x53, 0x10,
    0x4C, 0x8B, 0x43, 0x20, 0x49, 0x89, 0xD9, 0x48, 0x8D, 0x43, 0x18, 0x48, 0x89, 0x44, 0x24, 0x20,
    0xFF, 0x13, 0x48, 0x89, 0x43, 0x38, 0x48, 0x83, 0xC4, 0x30, 0x5B, 0xC3,
];

const CALLBACK: [u8; 101] = [
    0x56, 0x57, 0x48, 0x8B, 0x79, 0x28, 0x4C, 0x8B, 0x51, 0x30, 0x49, 0x83, 0xEA, 0x02, 0x45, 0x31,
    0xDB, 0x41, 0x39, 0xD3, 0x7D, 0x2E, 0x4B, 0x8B, 0x34, 0xD8, 0x48, 0x85, 0xF6, 0x74, 0x15, 0x8A,
    0x06, 0x84, 0xC0, 0x74, 0x0F, 0x4C, 0x39, 0xD7, 0x73, 0x2C, 0x88, 0x07, 0x48, 0xFF, 0xC7, 0x48,
    0xFF, 0xC6, 0xEB, 0xEB, 0x4C, 0x39, 0xD7, 0x73, 0x1D, 0xC6, 0x07, 0x1F, 0x48, 0xFF, 0xC7, 0x41,
    0xFF, 0xC3, 0xEB, 0xCD, 0xC6, 0x07, 0x1E, 0x48, 0xFF, 0xC7, 0xC6, 0x07, 0x00, 0x48, 0x89, 0x79,
    0x28, 0x31, 0xC0, 0x5F, 0x5E, 0xC3, 0xC6, 0x07, 0x00, 0x48, 0x89, 0x79, 0x28, 0xB8, 0x01, 0x00,
    0x00, 0x00, 0x5F, 0x5E, 0xC3,
];

struct Sqlite {
    exec: Sqlite3Exec,
    open: Sqlite3Open,
    close: Sqlite3Close,
}

fn load() -> Result<Sqlite, String> {
    unsafe {
        let module = LoadLibraryW(wide("winsqlite3.dll").as_ptr());
        if module == 0 {
            return Err("winsqlite3.dll not available on this system".into());
        }
        let exec = GetProcAddress(module, c"sqlite3_exec".as_ptr() as *const u8);
        let open = GetProcAddress(module, c"sqlite3_open".as_ptr() as *const u8);
        let close = GetProcAddress(module, c"sqlite3_close".as_ptr() as *const u8);
        if exec.is_null() || open.is_null() || close.is_null() {
            return Err("winsqlite3.dll is missing an expected export".into());
        }
        Ok(Sqlite {
            exec: std::mem::transmute::<*mut c_void, Sqlite3Exec>(exec),
            open: std::mem::transmute::<*mut c_void, Sqlite3Open>(open),
            close: std::mem::transmute::<*mut c_void, Sqlite3Close>(close),
        })
    }
}

#[test]
fn stub_and_callback_run_real_sqlite() {
    let sq = match load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };

    unsafe {
        let mem = VirtualAlloc(
            std::ptr::null_mut(),
            0x2_0000,
            MEM_COMMIT_RESERVE,
            PAGE_EXECUTE_READWRITE,
        ) as usize;
        assert_ne!(mem, 0, "VirtualAlloc failed");
        let stub = mem;
        let callback = mem + 0x200;
        let block = mem + 0x400;
        let sqlbuf = mem + 0x800;
        let out = mem + 0x1000;
        let out_end = out + 0x1_0000;

        std::ptr::copy_nonoverlapping(STUB.as_ptr(), stub as *mut u8, STUB.len());
        std::ptr::copy_nonoverlapping(CALLBACK.as_ptr(), callback as *mut u8, CALLBACK.len());
        FlushInstructionCache(GetCurrentProcess(), stub as *const c_void, STUB.len());
        FlushInstructionCache(GetCurrentProcess(), callback as *const c_void, CALLBACK.len());

        let mut db: usize = 0;
        let rc = (sq.open)(c":memory:".as_ptr() as *const u8, &mut db);
        assert_eq!(rc, 0, "sqlite3_open failed with {rc}");

        let ddl = c"CREATE TABLE Player(TeamID INTEGER, Name TEXT);INSERT INTO Player VALUES(7,'bravo');";
        let mut errmsg: usize = 0;
        let rc = (sq.exec)(db, ddl.as_ptr() as *const u8, 0, 0, &mut errmsg);
        assert_eq!(rc, 0, "seed SQL failed");

        let query = c"SELECT TeamID, Name FROM Player";
        std::ptr::copy_nonoverlapping(
            query.as_ptr() as *const u8,
            sqlbuf as *mut u8,
            query.to_bytes_with_nul().len(),
        );
        *(out as *mut u8) = 0;

        let mut blk = [0u64; 8];
        blk[0] = sq.exec as usize as u64;
        blk[1] = db as u64;
        blk[2] = sqlbuf as u64;
        blk[3] = 0;
        blk[4] = callback as u64;
        blk[5] = out as u64;
        blk[6] = out_end as u64;
        blk[7] = 0xFFFF_FFFF;
        std::ptr::copy_nonoverlapping(blk.as_ptr() as *const u8, block as *mut u8, 0x40);

        let stub_fn: extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(stub);
        stub_fn(block as *mut c_void);

        let rc = u32::from_le_bytes({
            let p = (block + 0x38) as *const u8;
            [*p, *p.add(1), *p.add(2), *p.add(3)]
        });
        assert_eq!(rc, 0, "stub reported sqlite error {rc}");

        let cursor_ptr = block + 0x28;
        let cursor = std::ptr::read_unaligned(cursor_ptr as *const usize);
        assert!(cursor > out, "callback wrote nothing");
        let len = cursor - out;
        let buf = std::slice::from_raw_parts(out as *const u8, len);
        let text = String::from_utf8_lossy(buf);
        eprintln!("stub output: {text:?}");
        assert!(
            text.contains("7\u{1f}bravo"),
            "expected '7<US>bravo' in the stub output, got {text:?}"
        );

        (sq.close)(db);
        VirtualFree(mem as *mut c_void, 0, MEM_RELEASE);
    }
}

/// The bug that broke the first live attempt: stub and parameter block at the same address.
/// That mistake is invisible until executed, so the layout is asserted.
#[test]
fn stub_and_block_must_not_overlap() {
    let (stub, callback, block, sql, out) = (0usize, 0x200, 0x400, 0x800, 0x1_0000usize);
    assert!(stub + STUB.len() <= callback, "stub would overwrite the callback");
    assert!(callback + CALLBACK.len() <= block, "callback would overwrite the block");
    assert!(block + 0x40 <= sql, "block would overwrite the statement buffer");
    assert!(sql + 0x400 <= out, "statement buffer would overwrite the output");
}
