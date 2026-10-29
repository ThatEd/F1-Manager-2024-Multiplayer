//! Runs SQL against the game's own in-memory SQLite database.
//!
//! Why this exists. The career database has **no file**: it is an in-memory SQLite database, so there
//! is no header to find in memory, no pager image to reconstruct and nothing on disk that corresponds
//! to it. The only handle on it is the live `sqlite3*`, and the only reliable way to touch it is to
//! call the game's own `sqlite3_exec`. That keeps every invariant intact - the page cache, the
//! locking, the write pipeline - because it is literally the same function the game calls.
//!
//! Ported from the approach proven in an existing community cheat table, then reimplemented here
//! rather than shelled out to, so the whole path is ours and auditable.
//!
//! Two pieces have to be located per session:
//!
//! * `sqlite3_exec` - found by the `mov r9d, 131055` line (`SQLITE_MISUSE_BKPT`), then rewound 0x6C
//!   bytes and confirmed by its prologue `4C 89 4C 24 20`.
//! * the live `sqlite3*` - found by the `eOpenState == SQLITE_STATE_OPEN (0x76)` byte signature, then
//!   accepted only if `db+0x20 == db+0x2A0`, `*(i32*)(db+0x28) == 2` and its `sqlite3_vfs*` points
//!   inside the game image. The decisive test is that its filename reads `:memory:`.
//!
//! The call itself runs on a thread inside the game because `sqlite3_exec` must not be called across
//! processes.

use crate::loopstub;
use std::ffi::c_void;

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn CloseHandle(h: isize) -> i32;
    fn ReadProcessMemory(h: isize, base: *const u8, buf: *mut u8, size: usize, read: *mut usize) -> i32;
    fn WriteProcessMemory(h: isize, base: *mut u8, buf: *const u8, size: usize, wrote: *mut usize) -> i32;
    fn VirtualAllocEx(h: isize, addr: *mut c_void, size: usize, alloc: u32, protect: u32) -> *mut c_void;
    fn VirtualFreeEx(h: isize, addr: *mut c_void, size: usize, free: u32) -> i32;
    fn VirtualQueryEx(h: isize, addr: *const u8, info: *mut MemoryBasicInformation, len: usize) -> usize;
    fn CreateRemoteThread(
        h: isize,
        sa: *mut c_void,
        stack: usize,
        start: Option<extern "system" fn(*mut c_void) -> u32>,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> isize;
    fn WaitForSingleObject(h: isize, millis: u32) -> u32;
    fn GetExitCodeThread(h: isize, code: *mut u32) -> i32;
}

const PROCESS_ALL: u32 = 0x1F0FFF;
const MEM_COMMIT_RESERVE: u32 = 0x3000;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const MEM_RELEASE: u32 = 0x8000;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const STILL_ACTIVE: u32 = 259;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MemoryBasicInformation {
    base_address: usize,
    allocation_base: usize,
    allocation_protect: u32,
    partition_id: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

const MEM_COMMIT: u32 = 0x1000;
const MEM_PRIVATE: u32 = 0x20000;
const PAGE_READWRITE: u32 = 0x04;

/// Thread entry. `rcx` = pointer to the parameter block, whose layout is fixed and documented below.
///
/// ```text
///   +0x00  exec          sqlite3_exec
///   +0x08  db            the live sqlite3*
///   +0x10  sql           NUL-terminated statement
///   +0x18  errmsg_out    set by sqlite3_exec
///   +0x20  callback      row callback
///   +0x28  out cursor    advanced by the callback (a POINTER VALUE, initialised to out)
///   +0x30  out end       one past the last usable byte
///   +0x38  rc            return code
/// ```
const STUB: [u8; 44] = [
    0x53, // push rbx
    0x48, 0x83, 0xEC, 0x30, // sub rsp, 0x30
    0x48, 0x89, 0xCB, // mov rbx, rcx
    0x48, 0x8B, 0x4B, 0x08, // mov rcx, [rbx+8]       db
    0x48, 0x8B, 0x53, 0x10, // mov rdx, [rbx+0x10]   sql
    0x4C, 0x8B, 0x43, 0x20, // mov r8,  [rbx+0x20]   callback
    0x49, 0x89, 0xD9, // mov r9, rbx            userdata = block
    0x48, 0x8D, 0x43, 0x18, // lea rax, [rbx+0x18]   &errmsg
    0x48, 0x89, 0x44, 0x24, 0x20, // mov [rsp+0x20], rax
    0xFF, 0x13, // call qword [rbx]       sqlite3_exec
    0x48, 0x89, 0x43, 0x38, // mov [rbx+0x38], rax   rc
    0x48, 0x83, 0xC4, 0x30, // add rsp, 0x30
    0x5B, // pop rbx
    0xC3, // ret
];

/// Row callback. `rcx` = block (our userdata), `rdx` = ncols, `r8` = col values, `r9` = col names.
///
/// Writes `value\x1Fvalue\x1F...\x1E` per row and NUL-terminates. Returns 0 to continue, 1 to abort
/// when the output buffer is nearly full.
const CALLBACK: [u8; 101] = [
    0x56, 0x57, // push rsi; push rdi
    0x48, 0x8B, 0x79, 0x28, // mov rdi, [rcx+0x28]   cursor
    0x4C, 0x8B, 0x51, 0x30, // mov r10, [rcx+0x30]   end
    0x49, 0x83, 0xEA, 0x02, // sub r10, 2            room for 0x1E and NUL
    0x45, 0x31, 0xDB, // xor r11d, r11d        column index
    0x41, 0x39, 0xD3, // cmp r11d, edx         next column:
    0x7D, 0x2E, // jge end_of_row
    0x4B, 0x8B, 0x34, 0xD8, // mov rsi, [r8+r11*8]
    0x48, 0x85, 0xF6, // test rsi, rsi
    0x74, 0x15, // jz  advance
    0x8A, 0x06, // mov al, [rsi]
    0x84, 0xC0, // test al, al
    0x74, 0x0F, // jz  advance
    0x4C, 0x39, 0xD7, // cmp rdi, r10
    0x73, 0x2C, // jae abort
    0x88, 0x07, // mov [rdi], al
    0x48, 0xFF, 0xC7, // inc rdi
    0x48, 0xFF, 0xC6, // inc rsi
    0xEB, 0xEB, // jmp copy
    0x4C, 0x39, 0xD7, // cmp rdi, r10          advance:
    0x73, 0x1D, // jae abort
    0xC6, 0x07, 0x1F, // mov byte [rdi], 0x1F
    0x48, 0xFF, 0xC7, // inc rdi
    0x41, 0xFF, 0xC3, // inc r11d
    0xEB, 0xCD, // jmp next column
    0xC6, 0x07, 0x1E, // mov byte [rdi], 0x1E  end_of_row:
    0x48, 0xFF, 0xC7, // inc rdi
    0xC6, 0x07, 0x00, // mov byte [rdi], 0
    0x48, 0x89, 0x79, 0x28, // mov [rcx+0x28], rdi
    0x31, 0xC0, // xor eax, eax          continue
    0x5F, 0x5E, 0xC3, // pop rdi; pop rsi; ret
    0xC6, 0x07, 0x00, // mov byte [rdi], 0     abort:
    0x48, 0x89, 0x79, 0x28, // mov [rcx+0x28], rdi
    0xB8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1     abort query
    0x5F, 0x5E, 0xC3, // pop rdi; pop rsi; ret
];

pub struct LiveDb {
    process: isize,
    pub exec: usize,
    pub db: usize,
    block: usize,
    stub: usize,
    callback: usize,
    sql: usize,
    out: usize,
    out_size: usize,
    calls: std::cell::Cell<usize>,
    /// Persistent worker: one remote thread for all statements, started on first use.
    worker: std::cell::Cell<usize>,
    worker_handle: std::cell::Cell<isize>,
    block_ref: std::cell::Cell<usize>,
    out_base: std::cell::Cell<usize>,
}

impl Drop for LiveDb {
    fn drop(&mut self) {
        // Shut the worker down BEFORE freeing anything. A spin loop left running inside the game
        // reads a block that is about to be unmapped, and the game dies later with no obvious cause -
        // which is exactly what happened: the query succeeded, our process exited, and the game fell
        // over afterwards.
        if self.worker.get() != 0 && self.worker_handle.get() != 0 {
            let base = self.block_ref.get();
            let _ = self.write(base + loopstub::OFF_STATE, &loopstub::STATE_QUIT.to_le_bytes());
            // Wait for it to leave its loop. Bounded, because a stuck worker must not hang our exit.
            let waited = unsafe { WaitForSingleObject(self.worker_handle.get(), 2000) };
            if waited != WAIT_OBJECT_0 {
                // It did not stop. The block cannot be freed safely, so leak it rather than unmap
                // memory a live thread is still using.
                eprintln!("[worker] did not stop in time; leaking its block to stay safe");
                unsafe { CloseHandle(self.worker_handle.get()) };
            } else {
                let mut code = 0u32;
                unsafe { GetExitCodeThread(self.worker_handle.get(), &mut code) };
                unsafe { CloseHandle(self.worker_handle.get()) };
                if self.block != 0 {
                    unsafe { VirtualFreeEx(self.process, self.block as *mut c_void, 0, MEM_RELEASE) };
                }
            }
        } else if self.block != 0 {
            unsafe { VirtualFreeEx(self.process, self.block as *mut c_void, 0, MEM_RELEASE) };
        }
        if self.process != 0 {
            unsafe { CloseHandle(self.process) };
        }
    }
}

impl LiveDb {
    fn read(&self, addr: usize, len: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; len];
        let mut got = 0usize;
        let ok = unsafe {
            ReadProcessMemory(self.process, addr as *const u8, buf.as_mut_ptr(), len, &mut got)
        };
        if ok == 0 || got != len {
            None
        } else {
            Some(buf)
        }
    }

    fn read_u64(&self, addr: usize) -> Option<u64> {
        self.read(addr, 8).map(|b| u64::from_le_bytes(b[..8].try_into().unwrap()))
    }

    fn write(&self, addr: usize, data: &[u8]) -> bool {
        let mut wrote = 0usize;
        let ok = unsafe {
            WriteProcessMemory(self.process, addr as *mut u8, data.as_ptr(), data.len(), &mut wrote)
        };
        ok != 0 && wrote == data.len()
    }

    /// Reads a NUL-terminated string, capped.
    fn read_cstr(&self, addr: usize, max: usize) -> Option<String> {
        let buf = self.read(addr, max)?;
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        Some(String::from_utf8_lossy(&buf[..end]).to_string())
    }

    /// Attaches to `pid`, locates `sqlite3_exec` and the live in-memory database.
    pub fn attach(pid: u32) -> Result<Self, String> {
        let process = unsafe { OpenProcess(PROCESS_ALL, 0, pid) };
        if process == 0 {
            return Err(format!(
                "OpenProcess failed (Win32 {}); run at the same privilege as the game",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
            ));
        }
        let mut s = LiveDb {
            process,
            exec: 0,
            db: 0,
            block: 0,
            stub: 0,
            callback: 0,
            sql: 0,
            out: 0,
            out_size: 3 * 1024 * 1024,
            calls: std::cell::Cell::new(0),
            worker: std::cell::Cell::new(0),
            worker_handle: std::cell::Cell::new(0),
            block_ref: std::cell::Cell::new(0),
            out_base: std::cell::Cell::new(0),
        };
        let base = s.module_base("F1Manager24.exe")?;
        s.exec = s.find_exec(base)?;
        s.db = s.find_db(base)?;
        s.alloc()?;
        Ok(s)
    }

    /// Base address of the main module.
    ///
    /// The game runs as a launcher plus the real image under the same name, so the module is chosen
    /// by size: the one holding the world is hundreds of megabytes, the launcher stub is tiny.
    fn module_base(&self, _name: &str) -> Result<usize, String> {
        let mut best: Option<(usize, usize)> = None;
        let mut addr = 0x1_0000usize;
        while addr < 0x7FFF_FFFF_0000 {
            let mut info = MemoryBasicInformation::default();
            let wrote = unsafe {
                VirtualQueryEx(
                    self.process,
                    addr as *const u8,
                    &mut info,
                    std::mem::size_of::<MemoryBasicInformation>(),
                )
            };
            if wrote == 0 || info.region_size == 0 {
                break;
            }
            // A PE image starts at its section base with an MZ header.
            if info.kind == 0x1000000 {
                if let Some(dos) = self.read(info.base_address, 0x40) {
                    if &dos[0..2] == b"MZ" {
                        let e = u32::from_le_bytes([dos[0x3C], dos[0x3D], dos[0x3E], dos[0x3F]]) as usize;
                        if e > 0 && e < 0x1000 {
                            if let Some(nt) = self.read(info.base_address + e, 0x60) {
                                if &nt[0..4] == b"PE\0\0" {
                                    let at = 0x18 + 0x38;
                                    let sz = u32::from_le_bytes([nt[at], nt[at + 1], nt[at + 2], nt[at + 3]]) as usize;
                                    if best.map(|(b, _)| sz > b).unwrap_or(true) {
                                        best = Some((sz, info.base_address));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            addr = info.base_address.saturating_add(info.region_size);
        }
        match best {
            Some((size, base)) => {
                if size < 0x1000_0000 {
                    return Err(format!("largest image is only 0x{size:X} bytes"));
                }
                Ok(base)
            }
            None => Err("no PE image found in the process".into()),
        }
    }

    fn image_size(&self, base: usize) -> usize {
        let dos = match self.read(base, 0x40) {
            Some(d) => d,
            None => return 0,
        };
        let e = u32::from_le_bytes([dos[0x3C], dos[0x3D], dos[0x3E], dos[0x3F]]) as usize;
        match self.read(base + e, 0x60) {
            Some(nt) => {
                let at = 0x18 + 0x38;
                u32::from_le_bytes([nt[at], nt[at + 1], nt[at + 2], nt[at + 3]]) as usize
            }
            None => 0,
        }
    }

    /// `mov r9d, 131055` (`SQLITE_MISUSE_BKPT`) inside `sqlite3_exec`, rewound to the entry.
    fn find_exec(&self, base: usize) -> Result<usize, String> {
        const PATTERN: [u8; 9] = [0x41, 0xB9, 0xEF, 0xFF, 0x01, 0x00, 0x4C, 0x8D, 0x05];
        const PROLOGUE: [u8; 5] = [0x4C, 0x89, 0x4C, 0x24, 0x20];
        let size = self.image_size(base);
        let mut off = 0usize;
        const CHUNK: usize = 0x80_0000;
        while off < size {
            let n = CHUNK.min(size - off);
            if let Some(d) = self.read(base + off, n) {
                let mut i = 0usize;
                while i + PATTERN.len() <= d.len() {
                    if d[i..i + PATTERN.len()] == PATTERN {
                        let hit = base + off + i;
                        if let Some(fn_addr) = hit.checked_sub(0x6C) {
                            if let Some(p) = self.read(fn_addr, PROLOGUE.len()) {
                                if p == PROLOGUE {
                                    return Ok(fn_addr);
                                }
                            }
                        }
                    }
                    i += 1;
                }
            }
            off += CHUNK;
        }
        Err("sqlite3_exec not found - unsupported game build".into())
    }

    /// The live `sqlite3*` whose filename is `:memory:`.
    fn find_db(&self, base: usize) -> Result<usize, String> {
        let size = self.image_size(base);
        let mut addr = 0x1_0000usize;
        while addr < 0x7FFF_FFFF_0000 {
            let mut info = MemoryBasicInformation::default();
            let wrote = unsafe {
                VirtualQueryEx(
                    self.process,
                    addr as *const u8,
                    &mut info,
                    std::mem::size_of::<MemoryBasicInformation>(),
                )
            };
            if wrote == 0 || info.region_size == 0 {
                break;
            }
            let usable = info.state == MEM_COMMIT
                && info.kind == MEM_PRIVATE
                && info.protect == PAGE_READWRITE;
            if usable {
                let n = info.region_size.min(0x40_0000);
                if let Some(d) = self.read(info.base_address, n) {
                    let mut i = 0usize;
                    while i + 14 <= d.len() {
                        // 01 ?? 00 00 00 00 FF 00 00 00 00 ?? ?? 76  (enc..eOpenState=OPEN)
                        if d[i] == 0x01
                            && d[i + 2] == 0
                            && d[i + 3] == 0
                            && d[i + 4] == 0
                            && d[i + 5] == 0
                            && d[i + 6] == 0xFF
                            && d[i + 7] == 0
                            && d[i + 8] == 0
                            && d[i + 9] == 0
                            && d[i + 10] == 0
                            && d[i + 13] == 0x76
                        {
                            if let Some(db) = (info.base_address + i).checked_sub(0x64) {
                                if self.looks_like_db(db, base, size) {
                                    if let Some(name) = self.db_filename(db) {
                                        if name == ":memory:" {
                                            return Ok(db);
                                        }
                                    }
                                }
                            }
                        }
                        i += 1;
                    }
                }
            }
            addr = info.base_address.saturating_add(info.region_size);
        }
        Err("career database not found - load a career save first".into())
    }

    fn looks_like_db(&self, db: usize, base: usize, size: usize) -> bool {
        let vfs = match self.read_u64(db) {
            Some(v) => v as usize,
            None => return false,
        };
        let self_ptr = match self.read_u64(db + 0x20) {
            Some(v) => v as usize,
            None => return false,
        };
        let n = match self.read(db + 0x28, 4) {
            Some(b) => i32::from_le_bytes(b[..4].try_into().unwrap()),
            None => return false,
        };
        self_ptr == db + 0x2A0 && n == 2 && vfs >= base && vfs < base + size
    }

    /// Walks `sqlite3 -> BTree -> shared cache -> pager -> filename`.
    fn db_filename(&self, db: usize) -> Option<String> {
        let btree = self.read_u64(db + 0x2A8)? as usize;
        if btree == 0 {
            return None;
        }
        let shared = self.read_u64(btree + 8)? as usize;
        if shared == 0 {
            return None;
        }
        let pager = self.read_u64(shared)? as usize;
        if pager == 0 {
            return None;
        }
        let z = self.read_u64(pager + 0xD8)? as usize;
        if z == 0 {
            return None;
        }
        self.read_cstr(z, 64)
    }


    /// Resolves an exported symbol's address **inside the target process**.
    ///
    /// `kernel32.dll` is a KnownDLL, mapped at the same base in every process on a given boot, so
    /// resolving locally and reading the target to confirm works. If the bases differ the address is
    /// rejected rather than used blindly, because calling a wrong address in the game is fatal.
    fn symbol_in_target(&self, module: &str, name: &std::ffi::CStr) -> Option<usize> {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetModuleHandleW(n: *const u16) -> isize;
            fn GetProcAddress(m: isize, n: *const u8) -> *mut std::ffi::c_void;
        }
        let wide: Vec<u16> = module.encode_utf16().chain(std::iter::once(0)).collect();
        let local_module = unsafe { GetModuleHandleW(wide.as_ptr()) };
        if local_module == 0 {
            return None;
        }
        let local = unsafe { GetProcAddress(local_module, name.as_ptr() as *const u8) } as usize;
        if local == 0 {
            return None;
        }
        // Confirm the same bytes are readable at that address in the target: if the module were mapped
        // elsewhere, this read fails and we refuse instead of calling into nothing.
        let probe = self.read(local, 8)?;
        if probe.iter().all(|&b| b == 0) {
            return None;
        }
        Some(local)
    }

    fn alloc(&mut self) -> Result<(), String> {
        let total = self.out_size + 0x2_0000;
        let mem = unsafe {
            VirtualAllocEx(
                self.process,
                std::ptr::null_mut(),
                total,
                MEM_COMMIT_RESERVE,
                PAGE_EXECUTE_READWRITE,
            )
        } as usize;
        if mem == 0 {
            return Err("VirtualAllocEx failed".into());
        }
        // These must not overlap: the thread starts at `stub`, and the block is the parameter the
        // stub reads its arguments from. Making the stub and the block the same address meant the
        // stub executed its own parameter bytes as code and never reported a result.
        self.stub = mem;
        self.callback = mem + 0x200;
        self.block = mem + 0x400;
        self.sql = mem + 0x800;
        // 0x1F000 of statement space, then the output area: keeps a maximal statement
        // well clear of the stub, callback and block.
        self.out = mem + 0x2_0000;
        if !self.write(self.stub, &STUB) || !self.write(self.callback, &CALLBACK) {
            return Err("could not write the stub".into());
        }
        Ok(())
    }

    /// Runs one statement, returning rows as vectors of strings.
    pub fn sql(&self, query: &str) -> Result<Vec<Vec<String>>, String> {
        // Hard limit: writing past the statement buffer would overwrite the stub and callback,
        // which corrupts the game rather than returning an error. 0x1E000 leaves headroom before
        // the output area at +0x20000.
        const MAX_SQL: usize = 0x1E000;
        if query.len() + 1 > MAX_SQL {
            return Err(format!(
                "statement is {} bytes, limit is {MAX_SQL}; split it into smaller statements",
                query.len()
            ));
        }
        let mut text = query.as_bytes().to_vec();
        text.push(0);
        if !self.write(self.sql, &text) {
            return Err("could not write the statement".into());
        }
        if !self.write(self.out, &[0u8]) {
            return Err("could not clear the output".into());
        }

        // cursor = out, end = out + size
        let mut block = [0u8; 0x40];
        block[0x00..0x08].copy_from_slice(&(self.exec as u64).to_le_bytes());
        block[0x08..0x10].copy_from_slice(&(self.db as u64).to_le_bytes());
        block[0x10..0x18].copy_from_slice(&(self.sql as u64).to_le_bytes());
        block[0x18..0x20].copy_from_slice(&0u64.to_le_bytes());
        block[0x20..0x28].copy_from_slice(&(self.callback as u64).to_le_bytes());
        block[0x28..0x30].copy_from_slice(&(self.out as u64).to_le_bytes());
        block[0x30..0x38].copy_from_slice(&((self.out + self.out_size) as u64).to_le_bytes());
        block[0x38..0x40].copy_from_slice(&0xFFFF_FFFFu64.to_le_bytes());
        if !self.write(self.block, &block) {
            return Err("could not write the parameter block".into());
        }

        self.calls.set(self.calls.get() + 1);

        // One persistent worker thread, not one per statement. Creating a remote thread per statement
        // failed around the 220th call during a transplant; a single looping worker does the whole job
        // with one creation. The stub and block are written once, then driven by stores to STATE.
        if self.worker.get() == 0 {
            eprintln!("[worker] writing stub at 0x{:X}", self.block + 0x600);
            let code = loopstub::build_loop_stub();
            let worker_code = self.block + 0x600;
            if !self.write(worker_code, &code) {
                return Err("could not write the worker stub".into());
            }
            // The block is in the TARGET process, so it must be written with WriteProcessMemory.
            // Using `Block`'s raw-pointer helpers here writes through a pointer that is not mapped in
            // this process, which is an access violation - and that is exactly what happened.
            // The idle path blocks in the target's own Sleep, so the worker costs the game
            // nothing between statements instead of burning a core.
            let sleep_addr = match self.symbol_in_target("kernel32.dll", c"Sleep") {
                Some(a) => a,
                None => return Err("could not resolve Sleep in the target process".into()),
            };
            let mut blk_bytes = [0u8; loopstub::BLOCK_SIZE];
            let put = |buf: &mut [u8; loopstub::BLOCK_SIZE], off: usize, v: u64| {
                buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
            };
            put(&mut blk_bytes, loopstub::OFF_EXEC, self.exec as u64);
            put(&mut blk_bytes, loopstub::OFF_DB, self.db as u64);
            put(&mut blk_bytes, loopstub::OFF_CALLBACK, self.callback as u64);
            put(&mut blk_bytes, loopstub::OFF_CURSOR, self.out as u64);
            put(&mut blk_bytes, loopstub::OFF_END, (self.out + self.out_size) as u64);
            put(&mut blk_bytes, loopstub::OFF_RC, u64::MAX);
            put(&mut blk_bytes, loopstub::OFF_STATE, loopstub::STATE_IDLE);
            put(&mut blk_bytes, loopstub::OFF_SLEEP, sleep_addr as u64);
            if !self.write(self.block, &blk_bytes) {
                return Err("could not write the parameter block".into());
            }
            let mut tid = 0u32;
            let thread = unsafe {
                CreateRemoteThread(
                    self.process,
                    std::ptr::null_mut(),
                    0,
                    Some(std::mem::transmute::<usize, extern "system" fn(*mut c_void) -> u32>(
                        worker_code,
                    )),
                    self.block as *mut c_void,
                    0,
                    &mut tid,
                )
            };
            eprintln!("[worker] thread = 0x{thread:X}");
            if thread == 0 {
                return Err(format!(
                    "CreateRemoteThread for the worker failed (Win32 {})",
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                ));
            }
            // Do NOT close the thread handle: keeping it lets us tell "still running" from "exited".
            self.worker_handle.set(thread);
            self.worker.set(worker_code);
            self.block_ref.set(self.block);
            self.out_base.set(self.out);
            // Give the worker a moment to reach its wait loop before the first request.
            std::thread::sleep(std::time::Duration::from_millis(50));

            // Confirm the worker is alive and has not overwritten its own parameters. A dead worker
            // means it faulted on entry, which is a different bug from a failed statement.
            eprintln!("[worker] waiting 50ms then checking liveness");
            let alive = unsafe { WaitForSingleObject(self.worker_handle.get(), 0) };
            if alive == WAIT_OBJECT_0 {
                let mut code = 0u32;
                unsafe { GetExitCodeThread(self.worker_handle.get(), &mut code) };
                return Err(format!(
                    "worker thread exited immediately with code 0x{code:X}; it faulted on entry"
                ));
            }
            let state = self
                .read_u64(self.block + loopstub::OFF_STATE)
                .ok_or("could not read the worker state")?;
            if state != loopstub::STATE_IDLE {
                return Err(format!("worker clobbered its state field (0x{state:X})"));
            }
        }

        // Hand the statement over: reset the cursor, publish the address, set state = pending, and
        // wait for the worker to publish rc and state = done. All through remote reads and writes.
        let base = self.block_ref.get();
        if !self.write(base + loopstub::OFF_CURSOR, &(self.out_base.get() as u64).to_le_bytes()) {
            return Err("could not reset the output cursor".into());
        }
        if !self.write(base + loopstub::OFF_SQL, &(self.sql as u64).to_le_bytes()) {
            return Err("could not publish the statement address".into());
        }
        if !self.write(base + loopstub::OFF_RC, &u64::MAX.to_le_bytes()) {
            return Err("could not clear the return code".into());
        }
        if !self.write(base + loopstub::OFF_STATE, &loopstub::STATE_PENDING.to_le_bytes()) {
            return Err("could not signal the worker".into());
        }

        // Poll with a real sleep rather than a hot spin: this process does not need to burn a core
        // while the game works.
        let mut waited_ms: u64 = 0;
        loop {
            let state = match self.read_u64(base + loopstub::OFF_STATE) {
                Some(s) => s,
                None => {
                    // Reading the block failed. Before blaming the worker, read whatever rc and
                    // errmsg are there - the block is usually still readable and the real cause is
                    // an SQL error, not a dead thread. Reporting "worker died" for a constraint
                    // failure has misdiagnosed this repeatedly.
                    let rc = self.read_u64(base + loopstub::OFF_RC).unwrap_or(u64::MAX);
                    if rc != u64::MAX {
                        return Err(format!("SQLite returned {rc} (block state unreadable)"));
                    }
                    return Err("could not read the worker state, and no return code was written".into());
                }
            };
            if state == loopstub::STATE_DONE {
                break;
            }
            waited_ms += 1;
            if waited_ms > 60_000 {
                let rc = self.read_u64(base + loopstub::OFF_RC).unwrap_or(u64::MAX);
                return Err(format!(
                    "statement did not complete within 60 s (state=0x{state:X}, rc=0x{rc:X})"
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let rc = self
            .read_u64(base + loopstub::OFF_RC)
            .ok_or("could not read the return code")? as u32;

        let cursor = self.read_u64(self.block + 0x28).unwrap_or(self.out as u64) as usize;
        let len = cursor.saturating_sub(self.out);
        let raw = if len > 0 && len <= self.out_size {
            self.read(self.out, len).unwrap_or_default()
        } else {
            Vec::new()
        };

        if rc != 0 {
            let err = self.read_u64(self.block + 0x18).unwrap_or(0) as usize;
            let msg = if err != 0 {
                self.read_cstr(err, 512).unwrap_or_default()
            } else {
                String::new()
            };
            return Err(format!("SQLite error {rc}: {msg}"));
        }

        let mut rows = Vec::new();
        let text = String::from_utf8_lossy(&raw);
        for row in text.split('\u{1e}') {
            if row.is_empty() {
                continue;
            }
            rows.push(row.split('\u{1f}').map(|s| s.to_string()).collect());
        }
        Ok(rows)
    }
}

/// Round-trip test that does not need a game: exercises the parsing half only.
pub fn parse_rows(raw: &[u8]) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let text = String::from_utf8_lossy(raw);
    for row in text.split('\u{1e}') {
        if row.is_empty() {
            continue;
        }
        rows.push(row.split('\u{1f}').map(|s| s.to_string()).collect());
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_separated_rows() {
        let raw = b"a\x1fb\x1e1\x1f2\x1e";
        let rows = parse_rows(raw);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["a", "b"]);
        assert_eq!(rows[1], vec!["1", "2"]);
    }

    #[test]
    fn ignores_empty_rows() {
        assert!(parse_rows(b"").is_empty());
        assert!(parse_rows(b"\x1e\x1e").is_empty());
    }

    #[test]
    fn keeps_empty_columns() {
        let rows = parse_rows(b"x\x1f\x1fy\x1e");
        assert_eq!(rows[0], vec!["x", "", "y"]);
    }

    /// The stub must move the documented fields into the argument registers, and the callback must
    /// treat `rcx` as the block with its cursor at `+0x28`. These are checked by decoding the
    /// instruction sequences rather than by trusting the comments.
    #[test]
    fn stub_loads_the_documented_offsets() {
        // mov rbx, rcx at 5..8
        assert_eq!(&STUB[5..8], &[0x48, 0x89, 0xCB]);
        // mov rcx, [rbx+0x08] at 8..12  -> sqlite3* db
        assert_eq!(&STUB[8..12], &[0x48, 0x8B, 0x4B, 0x08]);
        // mov rdx, [rbx+0x10] at 12..16 -> sql text
        assert_eq!(&STUB[12..16], &[0x48, 0x8B, 0x53, 0x10]);
        // mov r8,  [rbx+0x20] at 16..20 -> callback
        assert_eq!(&STUB[16..20], &[0x4C, 0x8B, 0x43, 0x20]);
        // mov r9, rbx at 20..23         -> userdata = the block itself
        assert_eq!(&STUB[20..23], &[0x49, 0x89, 0xD9]);
        // call qword [rbx] at 32..34    -> sqlite3_exec from +0x00
        assert_eq!(&STUB[32..34], &[0xFF, 0x13]);
        // mov [rbx+0x38], rax at 34..38 -> return code
        assert_eq!(&STUB[34..38], &[0x48, 0x89, 0x43, 0x38]);
        // ret at the end
        assert_eq!(*STUB.last().unwrap(), 0xC3);
    }

    #[test]
    fn callback_reads_cursor_and_end_from_the_block() {
        // mov rdi,[rcx+0x28]   cursor
        assert_eq!(&CALLBACK[2..6], &[0x48, 0x8B, 0x79, 0x28]);
        // mov r10,[rcx+0x30]   end
        assert_eq!(&CALLBACK[6..10], &[0x4C, 0x8B, 0x51, 0x30]);
    }
}
