//! External memory reader: the read half of the split.
//!
//! Reads are the safe half and the expensive half, so they live out of process. Nothing here can
//! stall the game thread, and a failure is an error code with a name instead of a frozen frame.
//!
//! The write half stays in-process on purpose (see the C++ plugin), because writes have to happen on
//! the game thread at a point the game considers safe.
//!
//! Raw Win32 FFI, no crates: the surface is three functions and keeping it dependency-free means the
//! toolchain cannot shift under a demo that is already hard to debug.

pub mod objects;

use std::ffi::c_void;
use std::fmt;

#[cfg(not(windows))]
compile_error!("coop-mem targets Windows only");

pub mod livedb;
pub mod binutil;
pub mod loopstub;
pub mod params;
pub mod statements;
pub mod retarget;
pub mod sqlcap;
pub mod replicate;
pub mod schema;
pub mod stmtapply;


const PROCESS_VM_READ: u32 = 0x0010;
/// Required to write into another process with `WriteProcessMemory`.
const PROCESS_VM_WRITE: u32 = 0x0020;
/// Required to change page protection with `VirtualProtectEx`.
const PROCESS_VM_OPERATION: u32 = 0x0008;

/// `MEMORY_BASIC_INFORMATION` for x64. `repr(C)` supplies the padding after `PartitionId`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MemoryBasicInformation {
    pub base_address: usize,
    pub allocation_base: usize,
    pub allocation_protect: u32,
    pub partition_id: u16,
    pub region_size: usize,
    pub state: u32,
    pub protect: u32,
    pub kind: u32,
}

const MEM_COMMIT: u32 = 0x1000;

#[repr(C)]
struct ProcessEntry32W {
    dw_size: u32,
    cnt_usage: u32,
    th32_process_id: u32,
    th32_default_heap_id: usize,
    th32_module_id: u32,
    cnt_threads: u32,
    th32_parent_process_id: u32,
    pc_pri_class_base: i32,
    dw_flags: u32,
    sz_exe_file: [u16; 260],
}
const PAGE_NOACCESS: u32 = 0x01;
const PAGE_GUARD: u32 = 0x100;
const PAGE_READWRITE: u32 = 0x04;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
/// Token access needed to read the integrity level.
const TOKEN_QUERY: u32 = 0x0008;
/// `TokenIntegrityLevel` from TOKEN_INFORMATION_CLASS.
const TOKEN_INTEGRITY_LEVEL: u32 = 25;

#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(process: isize, desired: u32, token: *mut isize) -> i32;
    fn GetTokenInformation(token: isize, class: u32, info: *mut c_void, len: u32, needed: *mut u32) -> i32;
}

/// Reads a process token's integrity RID. Higher means more privileged.
///
/// Returns 0 when it cannot be read, which makes every comparison false - an unknown level must not
/// be reported as a mismatch, because that would blame privileges for an unrelated failure.
fn integrity_level(process: isize) -> u32 {
    unsafe {
        let mut token: isize = 0;
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 || token == 0 {
            return 0;
        }
        let mut needed: u32 = 0;
        // First call sizes the buffer; the label is a SID whose last sub-authority is the RID.
        let _ = GetTokenInformation(token, TOKEN_INTEGRITY_LEVEL, std::ptr::null_mut(), 0, &mut needed);
        if needed == 0 || needed > 1024 {
            CloseHandle(token);
            return 0;
        }
        let mut buf = vec![0u8; needed as usize];
        let ok = GetTokenInformation(
            token,
            TOKEN_INTEGRITY_LEVEL,
            buf.as_mut_ptr() as *mut c_void,
            needed,
            &mut needed,
        );
        CloseHandle(token);
        if ok == 0 {
            return 0;
        }
        // TOKEN_MANDATORY_LABEL { SID_AND_ATTRIBUTES { PSID Sid; DWORD Attributes } }
        let sid = *(buf.as_ptr() as *const isize);
        if sid == 0 {
            return 0;
        }
        let count = *((sid as *const u8).add(1)) as usize;
        if count == 0 || count > 15 {
            return 0;
        }
        let rid_ptr = (sid as *const u8).add(8 + (count - 1) * 4);
        u32::from_le_bytes([
            *rid_ptr,
            *rid_ptr.add(1),
            *rid_ptr.add(2),
            *rid_ptr.add(3),
        ])
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(desired: u32, inherit: i32, pid: u32) -> isize;
    fn GetCurrentProcess() -> isize;
    fn ReadProcessMemory(h: isize, base: *const u8, buf: *mut u8, size: usize, read: *mut usize) -> i32;
    fn WriteProcessMemory(h: isize, base: *mut u8, buf: *const u8, size: usize, wrote: *mut usize) -> i32;
    fn VirtualProtectEx(h: isize, addr: *const u8, size: usize, new: u32, old: *mut u32) -> i32;
    fn CloseHandle(h: isize) -> i32;
    fn GetCurrentProcessId() -> u32;
    fn GetLastError() -> u32;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> isize;
    fn Process32FirstW(snap: isize, entry: *mut ProcessEntry32W) -> i32;
    fn Process32NextW(snap: isize, entry: *mut ProcessEntry32W) -> i32;
    fn VirtualQueryEx(h: isize, addr: *const u8, info: *mut MemoryBasicInformation, len: usize) -> usize;
}

#[derive(Debug)]
pub enum MemError {
    /// The process is not running, or not readable by this user.
    OpenFailed(u32),
    /// The read returned short: the address is unmapped, or the object moved underneath us.
    ReadFailed { addr: usize, want: usize, got: usize, code: u32 },
    /// The write was refused or short. A car record that only half landed would desync silently.
    WriteFailed { addr: usize, want: usize, wrote: usize, code: u32 },
    /// No loaded module matched the requested name.
    ModuleNotFound,
}

impl fmt::Display for MemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemError::OpenFailed(code) => write!(
                f,
                "OpenProcess failed (Win32 {code}). Run at the same privilege as the game; \
                 a process in a Sandboxie box needs the supervisor started inside that box."
            ),
            MemError::ReadFailed { addr, want, got, code } => write!(
                f,
                "ReadProcessMemory at 0x{addr:X} wanted {want} byte(s), got {got} (Win32 {code})"
            ),
            MemError::WriteFailed { addr, want, wrote, code } => write!(
                f,
                "WriteProcessMemory at 0x{addr:X} wanted {want} byte(s), wrote {wrote} (Win32 {code})"
            ),
            MemError::ModuleNotFound => write!(f, "no loaded module matched that name"),
        }
    }
}

impl std::error::Error for MemError {}

pub struct Process {
    handle: isize,
    pub pid: u32,
}

impl Process {
    pub fn open(pid: u32) -> Result<Self, MemError> {
        // Asking only for read rights was a real bug: every write then failed with Win32 5 (access
        // denied) at a perfectly valid address, which reads as a privilege problem and sends the
        // investigation the wrong way. Writing needs VM_WRITE, and changing page protection needs
        // VM_OPERATION, so both are requested up front.
        let rights = PROCESS_VM_READ
            | PROCESS_VM_WRITE
            | PROCESS_VM_OPERATION
            | PROCESS_QUERY_LIMITED_INFORMATION;
        let handle = unsafe { OpenProcess(rights, 0, pid) };
        if handle == 0 {
            return Err(MemError::OpenFailed(unsafe { GetLastError() }));
        }
        Ok(Process { handle, pid })
    }

    /// True if the target process runs at a higher integrity level than this one.
    ///
    /// Writing across integrity levels is refused by Windows no matter which rights were requested,
    /// so a client whose game was started elevated can never be written to by an unelevated agent.
    /// Detecting that and saying so is far better than reporting "access denied" at a valid address.
    pub fn target_is_elevated(&self) -> bool {
        integrity_level(self.handle) > integrity_level(unsafe { GetCurrentProcess() })
    }

    pub fn current() -> Result<Self, MemError> {
        Self::open(unsafe { GetCurrentProcessId() })
    }

    /// Every committed, readable region inside `[start, start + size)`.
    pub fn regions(&self, start: usize, size: usize) -> Vec<MemoryBasicInformation> {
        let mut out = Vec::new();
        let mut addr = start;
        let end = start.saturating_add(size);
        while addr < end {
            let mut info = MemoryBasicInformation::default();
            let wrote = unsafe {
                VirtualQueryEx(
                    self.handle,
                    addr as *const u8,
                    &mut info,
                    std::mem::size_of::<MemoryBasicInformation>(),
                )
            };
            if wrote == 0 || info.region_size == 0 {
                break;
            }
            let readable = info.state == MEM_COMMIT
                && info.protect != PAGE_NOACCESS
                && info.protect & PAGE_GUARD == 0;
            if readable {
                out.push(info);
            }
            addr = info.base_address.saturating_add(info.region_size);
        }
        out
    }

    /// Base address and size of the loaded image, read from its own mapped PE header.
    pub fn image_range(&self, base: usize) -> Result<(usize, usize), MemError> {
        let dos = self.read(base, 0x40)?;
        let e_lfanew = u32::from_le_bytes([dos[0x3C], dos[0x3D], dos[0x3E], dos[0x3F]]) as usize;
        let nt = self.read(base + e_lfanew, 0x60)?;
        if &nt[0..4] != b"PE\0\0" {
            return Err(MemError::ModuleNotFound);
        }
        // Optional header begins after the 20-byte COFF header; SizeOfImage is at +0x38 in PE32+.
        let opt = 0x18;
        let magic = u16::from_le_bytes([nt[opt], nt[opt + 1]]);
        let at = if magic == 0x20b { opt + 0x38 } else { opt + 0x38 };
        let size = u32::from_le_bytes([nt[at], nt[at + 1], nt[at + 2], nt[at + 3]]) as usize;
        Ok((base, size))
    }

    /// Base address of the process's main module.
    ///
    /// Reads the PEB rather than using a Toolhelp/`EnumProcessModules` snapshot: those take the
    /// target's loader lock, which deadlocks against a process that is itself inside
    /// `LoadLibrary` (for example while our agent DLL is being mapped). A PEB read is pure
    /// `ReadProcessMemory`, so it cannot block the game.
    ///
    /// `name` is only used to sanity-check that the caller has the right kind of process; pass
    /// `""` to skip the check.
    pub fn module_base(&self, name: &str) -> Result<usize, MemError> {
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn NtQueryInformationProcess(
                handle: isize,
                class: u32,
                info: *mut u8,
                len: u32,
                ret_len: *mut u32,
            ) -> i32;
        }
        // PROCESS_BASIC_INFORMATION is 6 pointers on x64; only PebBaseAddress matters here.
        const PROCESS_BASIC_INFORMATION: u32 = 0;
        const PEB_IMAGE_BASE_ADDRESS: usize = 0x10;

        let mut info = [0u8; 48];
        let mut ret_len = 0u32;
        let status = unsafe {
            NtQueryInformationProcess(
                self.handle,
                PROCESS_BASIC_INFORMATION,
                info.as_mut_ptr(),
                48,
                &mut ret_len,
            )
        };
        if status < 0 {
            return Err(MemError::OpenFailed(status as u32));
        }
        let peb = usize::from_le_bytes(info[8..16].try_into().unwrap());
        if peb == 0 {
            return Err(MemError::ModuleNotFound);
        }
        let raw = self.read(peb + PEB_IMAGE_BASE_ADDRESS, 8)?;
        let base = usize::from_le_bytes(raw[..8].try_into().unwrap());
        if base == 0 {
            return Err(MemError::ModuleNotFound);
        }
        if !name.is_empty() {
            let expected = if name.to_ascii_lowercase().ends_with(".exe") {
                name.to_ascii_lowercase()
            } else {
                format!("{}.exe", name.to_ascii_lowercase())
            };
            if let Ok(path) = self.main_image_path() {
                let leaf = path
                    .rsplit(['\\', '/'])
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if leaf != expected {
                    return Err(MemError::ModuleNotFound);
                }
            }
        }
        Ok(base)
    }

    /// Full path of the main executable, read from the PEB. Empty when it cannot be resolved.
    fn main_image_path(&self) -> Result<String, MemError> {
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn NtQueryInformationProcess(
                handle: isize,
                class: u32,
                info: *mut u8,
                len: u32,
                ret_len: *mut u32,
            ) -> i32;
        }
        const PROCESS_BASIC_INFORMATION: u32 = 0;
        let mut info = [0u8; 48];
        let mut ret_len = 0u32;
        let status = unsafe {
            NtQueryInformationProcess(self.handle, PROCESS_BASIC_INFORMATION, info.as_mut_ptr(), 48, &mut ret_len)
        };
        if status < 0 {
            return Err(MemError::OpenFailed(status as u32));
        }
        let peb = usize::from_le_bytes(info[8..16].try_into().unwrap());
        // PEB.ProcessParameters -> RTL_USER_PROCESS_PARAMETERS.ImagePathName (UNICODE_STRING).
        let params = usize::from_le_bytes(self.read(peb + 0x20, 8)?.try_into().unwrap());
        if params == 0 {
            return Ok(String::new());
        }
        let header = self.read(params + 0x60, 16)?;
        let buf = usize::from_le_bytes(header[8..16].try_into().unwrap());
        let chars = u16::from_le_bytes(header[0..2].try_into().unwrap()) as usize / 2;
        if buf == 0 || chars == 0 || chars > 1024 {
            return Ok(String::new());
        }
        let raw = self.read(buf, chars * 2)?;
        let wide: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&wide))
    }

    pub fn read_into(&self, addr: usize, buf: &mut [u8]) -> Result<(), MemError> {
        let mut got: usize = 0;
        let ok = unsafe {
            ReadProcessMemory(
                self.handle,
                addr as *const u8,
                buf.as_mut_ptr(),
                buf.len(),
                &mut got,
            )
        };
        if ok == 0 || got != buf.len() {
            return Err(MemError::ReadFailed {
                addr,
                want: buf.len(),
                got,
                code: unsafe { GetLastError() },
            });
        }
        Ok(())
    }

    pub fn read(&self, addr: usize, len: usize) -> Result<Vec<u8>, MemError> {
        let mut buf = vec![0u8; len];
        self.read_into(addr, &mut buf)?;
        Ok(buf)
    }

    /// Read a car block only. Cheaper, but race control is unavailable from it.
    pub fn read_cars(&self, rsm: usize) -> Result<Vec<coop_proto::Car>, MemError> {
        let buf = self.read(rsm, coop_proto::layout::car_block_len())?;
        Ok(coop_proto::layout::cars(&buf))
    }

    /// Read the 22 cars with one small read per car instead of one 10 MiB read for all of them.
    ///
    /// `read_cars` reads `car_block_len()` = `CAR_BASE + 22*CAR_STRIDE` ≈ 10.07 MiB, because the car records are
    /// spread across a 0x10D8 stride of which only ~120 bytes are used. At the default 20 Hz that is **200 MiB/s
    /// copied out of the game process to derive a 2.43 KiB frame** (§21 of the audit). Nothing on the wire is
    /// wrong; the local copy is 4,000x more work than the problem needs.
    ///
    /// This reads just the used span of each record. It is *not* unambiguously better: the reads are no longer
    /// one contiguous range, so a value can be torn between two calls if the sim writes during the loop, and a
    /// torn snapshot is worse than a slow one. The mitigation is the caller's: the game thread applies host
    /// state at a single point (`hookmail::apply`), so a torn read shows up as one car jumping for one frame
    /// rather than as a corrupt write. `read_cars` stays as the reference implementation and the thing to
    /// compare against.
    ///
    /// Returns `None` for a car whose read fails, and the caller decides whether a partial field is acceptable -
    /// a shorter vector is a visible signal rather than a silently zeroed car.
    pub fn read_cars_sparse(&self, rsm: usize) -> Result<Vec<coop_proto::Car>, MemError> {
        let span = coop_proto::layout::car_record_used_len();
        let mut out = Vec::with_capacity(coop_proto::layout::CAR_MAX);
        let mut buf = vec![0u8; span];
        for i in 0..coop_proto::layout::CAR_MAX {
            let at = rsm + coop_proto::layout::car_start(i);
            if self.read_into(at, &mut buf).is_err() {
                // Skip this car rather than fabricate it. `cars()` on a short buffer yields fewer cars, which
                // `decode`'s car-count check rejects outright - so a partial field is loud, not silent.
                continue;
            }
            if let Some(car) = coop_proto::layout::car_from_record(&buf, 0, i as u8) {
                out.push(car);
            }
        }
        Ok(out)
    }

    /// Read the whole object: cars plus race control.
    pub fn read_object(&self, rsm: usize) -> Result<Vec<u8>, MemError> {
        self.read(rsm, coop_proto::layout::object_len())
    }

    /// Write raw bytes into the target.
    ///
    /// This is the write half the co-op design needs: the host is authoritative for all 22 cars and
    /// a client's simulation must be driven from host state, so there has to be a way to put car
    /// records into another process. Until this existed every "sync" report described a text file.
    ///
    /// The page is made writable first and restored afterwards. A race sim object can live in a
    /// read-only region (or a region whose protection the game changes at runtime), and a
    /// `WriteProcessMemory` into a `PAGE_READONLY` page fails with access denied rather than
    /// partially writing.
    pub fn write_into(&self, addr: usize, buf: &[u8]) -> Result<(), MemError> {
        if buf.is_empty() {
            return Ok(());
        }
        let mut old = 0u32;
        let changed = unsafe {
            VirtualProtectEx(
                self.handle,
                addr as *const u8,
                buf.len(),
                PAGE_READWRITE,
                &mut old,
            )
        };
        // A failed protection change is not fatal on its own: some regions are already writable and
        // VirtualProtectEx still succeeds on those, so a genuine failure shows up as a failed write.
        let mut wrote: usize = 0;
        let ok = unsafe {
            WriteProcessMemory(
                self.handle,
                addr as *mut u8,
                buf.as_ptr(),
                buf.len(),
                &mut wrote,
            )
        };
        if changed != 0 {
            let mut ignored = 0u32;
            unsafe {
                VirtualProtectEx(
                    self.handle,
                    addr as *const u8,
                    buf.len(),
                    old,
                    &mut ignored,
                );
            }
        }
        if ok == 0 || wrote != buf.len() {
            return Err(MemError::WriteFailed {
                addr,
                want: buf.len(),
                wrote,
                code: unsafe { GetLastError() },
            });
        }
        Ok(())
    }

    /// Write one car's replicable state into the target's car slot.
    ///
    /// Encodes into a scratch buffer at the car's real position in the object, then writes only the
    /// span that holds the touched fields - not the whole 0xA132F0 object, which would also rewrite
    /// the race-control bytes and every other car.
    ///
    /// **Gated per car.** Writing car state into another process is exactly the write `coop-auth` was built to
    /// police: in `Versus` each player owns one team, in `SharedTeam` a garage is shared but the two cars belong
    /// to different players, and a client must never overwrite a car it does not own. `coop-auth` existed with
    /// tests for precisely this and had no callers, so the rule was documented and unenforced.
    ///
    /// The car index is mapped to a team the only way this layer can: cars `0..1` are the player's own (the game
    /// fields one team, two cars), everything else is another team's. That is stated rather than assumed - if it
    /// is wrong, `may_write` refuses writes that should land, which is the safe direction, and a session shows
    /// it immediately as cars that do not move rather than as a corrupted rival.
    pub fn write_car(&self, rsm: usize, car: &coop_proto::Car) -> Result<(), MemError> {
        // Bounds first, and here rather than only in the caller. `write_cars` already skips ids past
        // `CAR_MAX`, but this function is `pub` and indexes `car_start(car.id)` without a check, so anything
        // that reaches it by another route computes an address up to `(255 - CAR_MAX) * CAR_STRIDE` past the
        // car table - roughly a megabyte beyond the sim object, i.e. into whatever the allocator put next.
        // The write would be "successful" and corrupt unrelated game memory. `car_at`, the read path, already
        // refuses `car >= CAR_MAX`; this is the same rule at the write chokepoint, so the guarantee does not
        // depend on every future caller remembering it.
        if car.id as usize >= coop_proto::layout::CAR_MAX {
            return Err(MemError::WriteFailed {
                addr: rsm.saturating_add(coop_proto::layout::car_start(car.id as usize)),
                want: 1,
                wrote: 0,
                code: 0,
            });
        }
        if let Some((mode, player)) = coop_auth::configured() {
            let scope = coop_auth::Scope::PerCar;
            let car_team = if (car.id as u32) < 2 { player.team } else { player.team.wrapping_add(1) };
            let key = coop_auth::RowKey {
                team: Some(car_team),
                loadout: Some(player.loadout),
                car: Some(car.id as u32),
            };
            if let Err(denied) = coop_auth::may_write(mode, &player, scope, &key) {
                // Report where it can be seen, then refuse. `eprintln!` rather than a new log helper: this crate
                // is used by command-line tools whose stderr is the operator's console, and adding a logging
                // path here would be more machinery than the one line is worth.
                eprintln!("[coop] refused car {}: {}", car.id, denied.as_str());
                return Err(MemError::WriteFailed {
                    addr: rsm + coop_proto::layout::car_start(car.id as usize),
                    want: 1,
                    wrote: 0,
                    code: 0,
                });
            }
        }
        // encode_car_motion, not encode_car: see its doc comment. Writing the full record made the
        // game dereference a track-graph index that did not belong to it and crash.
        let start = coop_proto::layout::car_start(car.id as usize);
        let mut scratch = vec![0u8; start + coop_proto::layout::CAR_STRIDE];
        let (a, b) = coop_proto::layout::encode_car_motion(&mut scratch, car).ok_or(
            MemError::WriteFailed {
                addr: rsm + start,
                want: 1,
                wrote: 0,
                code: 0,
            },
        )?;
        self.write_into(rsm + a, &scratch[a..b])
    }

    /// Write every car's replicable state in one pass.
    pub fn write_cars(
        &self,
        rsm: usize,
        cars: &[coop_proto::Car],
    ) -> Result<usize, MemError> {
        let mut n = 0;
        for c in cars {
            if c.id as usize >= coop_proto::layout::CAR_MAX {
                continue;
            }
            self.write_car(rsm, c)?;
            n += 1;
        }
        Ok(n)
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.handle != 0 {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

/// Reads the RaceSimManager address published by the in-process side.
///
/// The agent writes `RSM <hex address> <hex size> <unix time>`; anything older than `max_age_secs`
/// is refused, because a stale address points into freed memory and reading it would produce
/// convincing garbage.
/// Process ids whose executable name matches `name`, largest first.
///
/// The game runs as two processes with the same name: a small launcher and the real game holding the
/// world. Ordering by size lets a caller attach correctly without asking for a pid, and without ever
/// attaching to the launcher, which has nothing readable in it.
pub fn find_processes(name: &str) -> Vec<u32> {
    const TH32CS_SNAPPROCESS: u32 = 0x2;

    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> isize;
        fn Process32FirstW(snap: isize, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snap: isize, entry: *mut ProcessEntry32W) -> i32;
    }

    let mut pids = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap != 0 && snap != -1 {
            let mut entry: ProcessEntry32W = std::mem::zeroed();
            entry.dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut entry) != 0;
            while ok {
                let len = entry
                    .sz_exe_file
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.sz_exe_file.len());
                if String::from_utf16_lossy(&entry.sz_exe_file[..len]).eq_ignore_ascii_case(name) {
                    pids.push(entry.th32_process_id);
                }
                entry.dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
                ok = Process32NextW(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
        }
    }

    // Rank by image size, so the launcher sorts last.
    let mut sized: Vec<(usize, u32)> = pids
        .into_iter()
        .map(|pid| {
            let size = Process::open(pid)
                .and_then(|p| {
                    let base = p.module_base("").unwrap_or(0);
                    p.image_range(base).map(|(_, s)| s)
                })
                .unwrap_or(0);
            (size, pid)
        })
        .collect();
    sized.sort_by(|a, b| b.0.cmp(&a.0));
    sized.into_iter().map(|(_, pid)| pid).collect()
}

pub fn read_endpoint(path: &std::path::Path, max_age_secs: u64) -> Result<(usize, usize), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path:?}: {e}"))?;
    let line = text.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    if parts.next() != Some("RSM") {
        return Err(format!("{path:?}: expected a line starting with RSM"));
    }
    let addr = parts
        .next()
        .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .ok_or_else(|| format!("{path:?}: unreadable address"))?;
    let size = parts
        .next()
        .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .ok_or_else(|| format!("{path:?}: unreadable size"))?;
    let when: u64 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("{path:?}: unreadable timestamp"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now.saturating_sub(when) > max_age_secs {
        return Err(format!("{path:?}: endpoint is {}s old", now.saturating_sub(when)));
    }
    Ok((addr, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads memory out of our own process. This proves the FFI, the error paths and the layout
    /// decode together, without needing the game running.
    #[test]
    fn reads_its_own_memory() {
        let proc = Process::current().expect("must open own process");
        let marker: Vec<u8> = (0..64u8).collect();
        let got = proc.read(marker.as_ptr() as usize, marker.len()).expect("must read");
        assert_eq!(got, marker);
    }

    #[test]
    fn reports_unmapped_reads_instead_of_garbage() {
        let proc = Process::current().expect("must open own process");
        // address 1 is never mapped in a Windows process
        let err = proc.read(1, 16).expect_err("unmapped read must fail");
        match err {
            MemError::ReadFailed { addr, want, got, .. } => {
                assert_eq!(addr, 1);
                assert_eq!(want, 16);
                assert_eq!(got, 0);
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn decodes_a_car_block_from_real_memory() {
        let proc = Process::current().expect("must open own process");
        let mut buf = vec![0u8; coop_proto::layout::car_block_len()];
        // put a recognisable distance into car 7
        let o = coop_proto::layout::CAR_BASE + 7 * coop_proto::layout::CAR_STRIDE;
        buf[o + coop_proto::layout::off::DIST..o + coop_proto::layout::off::DIST + 4]
            .copy_from_slice(&1234.5f32.to_bits().to_le_bytes());
        let cars = {
            let raw = proc.read(buf.as_ptr() as usize, buf.len()).expect("must read");
            coop_proto::layout::cars(&raw)
        };
        assert_eq!(cars.len(), coop_proto::layout::CAR_MAX);
        assert_eq!(cars[7].dist, 1234.5);
    }
}
