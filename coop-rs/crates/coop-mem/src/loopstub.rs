//! A persistent worker thread for running many statements through the game's `sqlite3_exec`.
//!
//! Why this replaces one-thread-per-statement: a transplant is ~600 statements, and creating a remote
//! thread for each one failed around the 220th call. Thread creation is heavy and rate-limited, so using
//! it per statement is the wrong shape. One thread that loops — waiting for work, executing, publishing
//! the result — does 600 statements with a single creation.
//!
//! The stub is **generated**, not hand-assembled. Every relative jump is a computed displacement, so a
//! wrong offset cannot happen by construction; hand-written jump targets were already wrong twice.
//!
//! Shared block layout (all single 64-bit stores, so no locking is needed):
//!
//! ```text
//!   +0x00  exec            sqlite3_exec
//!   +0x08  db              the live sqlite3*
//!   +0x10  sql             NUL-terminated statement for the current request
//!   +0x18  errmsg_out      set by sqlite3_exec
//!   +0x20  callback        row callback
//!   +0x28  out cursor      advanced by the callback (a POINTER VALUE, initialised to out)
//!   +0x30  out end         one past the last usable byte
//!   +0x38  rc              return code of the last statement
//!   +0x40  state           0 idle, 1 request pending, 2 done, 3 quit
//! ```

/// Emits the worker stub with computed jump displacements.
pub fn build_loop_stub() -> Vec<u8> {
    let mut c: Vec<u8> = Vec::with_capacity(96);
    c.push(0x53); // push rbx
    c.push(0x56); // push rsi
    c.extend_from_slice(&[0x48, 0x83, 0xEC, 0x28]); // sub rsp,0x28
    c.extend_from_slice(&[0x48, 0x89, 0xCB]); // mov rbx,rcx

    let wait = c.len();
    c.extend_from_slice(&[0x48, 0x8B, 0x43, 0x40]); // mov rax,[rbx+0x40]  state
    c.extend_from_slice(&[0x48, 0x83, 0xF8, 0x01]); // cmp rax,1
    let je_run_patch = c.len() + 1;
    c.extend_from_slice(&[0x74, 0x00]); // je run
    c.extend_from_slice(&[0x48, 0x83, 0xF8, 0x03]); // cmp rax,3
    let je_done_patch = c.len() + 1;
    c.extend_from_slice(&[0x74, 0x00]); // je done
    // Idle path: block in the target's Sleep(1) rather than spinning. A hot loop inside the game
    // starves it, and that is the one property common to every failure mode observed so far.
    c.extend_from_slice(&[0x48, 0xC7, 0xC1, 0x01, 0x00, 0x00, 0x00]); // mov rcx,1
    c.extend_from_slice(&[0x48, 0x8B, 0x43, 0x48]); // mov rax,[rbx+0x48]  Sleep
    c.extend_from_slice(&[0xFF, 0xD0]); // call rax
    let jmp_wait_patch = c.len() + 1;
    c.extend_from_slice(&[0xEB, 0x00]); // jmp wait

    let run = c.len();
    c.extend_from_slice(&[0x48, 0x8B, 0x4B, 0x08]); // mov rcx,[rbx+0x08]   db
    c.extend_from_slice(&[0x48, 0x8B, 0x53, 0x10]); // mov rdx,[rbx+0x10]   sql
    c.extend_from_slice(&[0x4C, 0x8B, 0x43, 0x20]); // mov r8,[rbx+0x20]    callback
    c.extend_from_slice(&[0x49, 0x89, 0xD9]); // mov r9,rbx           userdata
    c.extend_from_slice(&[0x48, 0x8D, 0x43, 0x18]); // lea rax,[rbx+0x18]   &errmsg
    c.extend_from_slice(&[0x48, 0x89, 0x44, 0x24, 0x20]); // mov [rsp+0x20],rax
    c.extend_from_slice(&[0x48, 0x8B, 0x03]); // mov rax,[rbx]        exec
    c.extend_from_slice(&[0xFF, 0xD0]); // call rax
    c.extend_from_slice(&[0x48, 0x89, 0x43, 0x38]); // mov [rbx+0x38],rax   rc
    c.extend_from_slice(&[0x48, 0xC7, 0x43, 0x40, 0x02, 0x00, 0x00, 0x00]); // mov qword [rbx+0x40],2
    let jmp_wait2_patch = c.len() + 1;
    c.extend_from_slice(&[0xEB, 0x00]); // jmp wait

    let done = c.len();
    c.extend_from_slice(&[0x31, 0xC0]); // xor eax,eax
    c.extend_from_slice(&[0x48, 0x83, 0xC4, 0x28]); // add rsp,0x28
    c.push(0x5E); // pop rsi
    c.push(0x5B); // pop rbx
    c.push(0xC3); // ret

    let rel8 = |from_next: usize, to: usize| -> u8 {
        let d = to as isize - from_next as isize;
        assert!((-128..=127).contains(&d), "jump out of rel8 range: {d}");
        d as i8 as u8
    };
    c[je_run_patch] = rel8(je_run_patch + 1, run);
    c[je_done_patch] = rel8(je_done_patch + 1, done);
    c[jmp_wait_patch] = rel8(jmp_wait_patch + 1, wait);
    c[jmp_wait2_patch] = rel8(jmp_wait2_patch + 1, wait);
    c
}

/// Block offsets the stub and callback rely on.
pub const OFF_EXEC: usize = 0x00;
pub const OFF_DB: usize = 0x08;
pub const OFF_SQL: usize = 0x10;
pub const OFF_ERRMSG: usize = 0x18;
pub const OFF_CALLBACK: usize = 0x20;
pub const OFF_CURSOR: usize = 0x28;
pub const OFF_END: usize = 0x30;
pub const OFF_RC: usize = 0x38;
pub const OFF_STATE: usize = 0x40;
/// Pointer to the target process's `Sleep`, so the idle path blocks instead of spinning.
pub const OFF_SLEEP: usize = 0x48;
pub const BLOCK_SIZE: usize = 0x58;

pub const STATE_IDLE: u64 = 0;
pub const STATE_PENDING: u64 = 1;
pub const STATE_DONE: u64 = 2;
pub const STATE_QUIT: u64 = 3;

/// Host-side view of the shared block.
#[derive(Clone, Copy)]
pub struct Block {
    pub addr: usize,
}

impl Block {
    pub fn new(addr: usize) -> Self {
        Self { addr }
    }

    pub fn write(&self, off: usize, v: u64) {
        unsafe { std::ptr::write_unaligned((self.addr + off) as *mut u64, v) };
    }

    pub fn read(&self, off: usize) -> u64 {
        unsafe { std::ptr::read_unaligned((self.addr + off) as *const u64) }
    }

    /// Publishes everything the stub needs before the thread starts.
    pub fn prepare(&self, exec: usize, db: usize, callback: usize, out: usize, out_end: usize) {
        self.write(OFF_EXEC, exec as u64);
        self.write(OFF_DB, db as u64);
        self.write(OFF_CALLBACK, callback as u64);
        self.write(OFF_CURSOR, out as u64);
        self.write(OFF_END, out_end as u64);
        self.write(OFF_RC, u64::MAX);
        self.write(OFF_STATE, STATE_IDLE);
    }

    /// Hands one statement over and waits for it. Returns `(rc, bytes_written)`.
    ///
    /// `sql_addr` is the address of the NUL-terminated statement, which may be in another process.
    pub fn run_once(&self, sql_addr: usize) -> (u32, usize) {
        // `out` is the caller-supplied base of the output buffer; the cursor is reset to it before
        // each statement so rows do not accumulate across calls.
        let base = self.read(OFF_CURSOR) as usize;
        self.write(OFF_SQL, sql_addr as u64);
        self.write(OFF_RC, u64::MAX);
        self.write(OFF_STATE, STATE_PENDING);

        let mut spins: u64 = 0;
        loop {
            if self.read(OFF_STATE) == STATE_DONE {
                break;
            }
            spins += 1;
            if spins > 2_000_000_000 {
                return (u32::MAX, 0);
            }
            std::hint::spin_loop();
        }
        let rc = self.read(OFF_RC) as u32;
        let cursor = self.read(OFF_CURSOR) as usize;
        (rc, cursor.saturating_sub(base))
    }

    /// Resets the output cursor before a statement, so rows do not accumulate across calls.
    pub fn reset_cursor(&self, out: usize) {
        self.write(OFF_CURSOR, out as u64);
    }

    pub fn quit(&self) {
        self.write(OFF_STATE, STATE_QUIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_stub_ends_with_ret_and_has_four_jumps() {
        let stub = build_loop_stub();
        assert_eq!(*stub.last().unwrap(), 0xC3, "must end with ret");
        assert!(stub.len() < 128, "rel8 jumps require the stub to stay under 128 bytes, got {}", stub.len());
        let jumps = stub.iter().filter(|b| **b == 0x74 || **b == 0xEB).count();
        assert!(jumps >= 4, "expected at least 4 jumps, found {jumps}");
    }

    #[test]
    fn stub_accesses_every_documented_field() {
        let s = build_loop_stub();
        let has = |p: &[u8]| s.windows(p.len()).any(|w| w == p);
        assert!(has(&[0x48, 0x8B, 0x43, 0x40]), "reads state +0x40");
        assert!(has(&[0x48, 0x8B, 0x4B, 0x08]), "loads db +0x08");
        assert!(has(&[0x48, 0x8B, 0x53, 0x10]), "loads sql +0x10");
        assert!(has(&[0x4C, 0x8B, 0x43, 0x20]), "loads callback +0x20");
        assert!(has(&[0x48, 0x89, 0x43, 0x38]), "stores rc +0x38");
        assert!(has(&[0x48, 0xC7, 0x43, 0x40]), "stores state +0x40");
        assert!(has(&[0x48, 0x8B, 0x43, 0x48]), "loads Sleep from +0x48");
        assert!(!has(&[0xF3, 0x90]), "must not spin: the pause instruction is gone");
    }

    #[test]
    fn state_values_are_distinct() {
        let all = [STATE_IDLE, STATE_PENDING, STATE_DONE, STATE_QUIT];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn block_layout_does_not_overlap() {
        let offs = [
            OFF_EXEC, OFF_DB, OFF_SQL, OFF_ERRMSG, OFF_CALLBACK, OFF_CURSOR, OFF_END, OFF_RC,
            OFF_STATE,
            OFF_SLEEP,
        ];
        for w in offs.windows(2) {
            assert!(w[1] >= w[0] + 8, "fields at 0x{:X} and 0x{:X} overlap", w[0], w[1]);
        }
        assert!(OFF_STATE + 8 <= BLOCK_SIZE);
    }
}
