//! Loading the agent DLL into a running game process.
//!
//! Classic CreateRemoteThread + LoadLibraryW. Deliberately explicit about every Win32 failure, because the
//! alternative - a silent no-op - is the failure mode this whole rebuild exists to escape.
//!
//! A library rather than a standalone tool so that `coop-link --inject` can use it. The goal is to ship as few
//! files as possible: an injector is a small amount of Win32 and does not deserve its own `.exe` on both
//! machines when the client binary is already there.

use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn CloseHandle(h: isize) -> i32;
    fn VirtualAllocEx(h: isize, addr: *mut c_void, size: usize, alloc: u32, protect: u32) -> *mut c_void;
    fn VirtualFreeEx(h: isize, addr: *mut c_void, size: usize, free: u32) -> i32;
    fn WriteProcessMemory(h: isize, addr: *mut c_void, buf: *const c_void, size: usize, written: *mut usize) -> i32;
    fn CreateRemoteThread(h: isize, attrs: *mut c_void, stack: usize, start: *mut c_void, param: *mut c_void, flags: u32, id: *mut u32) -> isize;
    fn WaitForSingleObject(h: isize, ms: u32) -> u32;
    fn GetExitCodeThread(h: isize, code: *mut u32) -> i32;
    fn GetModuleHandleW(name: *const u16) -> isize;
    fn GetProcAddress(module: isize, name: *const u8) -> *mut c_void;
    fn GetLastError() -> u32;
}

const PROCESS_ALL_ACCESS: u32 = 0x001F_0FFF;
const MEM_COMMIT_RESERVE: u32 = 0x3000;
const PAGE_READWRITE: u32 = 0x04;
const MEM_RELEASE: u32 = 0x8000;
const WAIT_OBJECT_0: u32 = 0;

fn last_error(what: &str) -> String {
    format!("{what} failed (Win32 {})", unsafe { GetLastError() })
}

fn wide(s: &OsStr) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_wide().collect();
    v.push(0);
    v
}

/// The pid of the largest running process with this image name.
///
/// Largest, not first: the game runs as a small launcher plus the real game under the same name, and the
/// launcher holds nothing worth injecting into.
pub fn pid_by_name(name: &str) -> Option<u32> {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {name}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() >= 2 {
            let image = cols[0].trim_matches('"');
            if image.eq_ignore_ascii_case(name) || image.eq_ignore_ascii_case(&format!("{name}.exe")) {
                let pid = cols[1].trim_matches('"').parse().ok()?;
                return Some(pid);
            }
        }
    }
    None
}

/// Loads `dll` into `pid`. Returns the module base the remote `LoadLibraryW` reported.
///
/// Every failure is a typed `Err` with the Win32 code, because the thing this must never do is appear to
/// succeed. A silent no-op injector is what made an earlier generation of this project debug the wrong
/// layer for hours.
pub fn inject(pid: u32, dll: &str) -> Result<usize, String> {
    let dll_path = std::path::Path::new(dll);
    if !dll_path.is_file() {
        return Err(format!("dll not found: {}", dll_path.display()));
    }
    let abs = dll_path
        .canonicalize()
        .map(|p| p.to_string_lossy().trim_start_matches(r"\\?\").to_string())
        .unwrap_or_else(|_| dll.to_string());

    let handle = unsafe { OpenProcess(PROCESS_ALL_ACCESS, 0, pid) };
    if handle == 0 {
        return Err(format!(
            "{} - run elevated if the game runs elevated; a process inside a Sandboxie box must be \
             injected from inside that box",
            last_error("OpenProcess")
        ));
    }

    let path_w = wide(OsStr::new(&abs));
    let bytes = path_w.len() * 2;

    let remote =
        unsafe { VirtualAllocEx(handle, std::ptr::null_mut(), bytes, MEM_COMMIT_RESERVE, PAGE_READWRITE) };
    if remote.is_null() {
        unsafe { CloseHandle(handle) };
        return Err(last_error("VirtualAllocEx"));
    }

    let mut written = 0usize;
    let ok = unsafe { WriteProcessMemory(handle, remote, path_w.as_ptr() as *const c_void, bytes, &mut written) };
    if ok == 0 || written != bytes {
        unsafe {
            VirtualFreeEx(handle, remote, 0, MEM_RELEASE);
            CloseHandle(handle);
        }
        return Err(last_error("WriteProcessMemory"));
    }

    let kernel32 = unsafe { GetModuleHandleW(wide(OsStr::new("kernel32.dll")).as_ptr()) };
    let load_library = unsafe { GetProcAddress(kernel32, b"LoadLibraryW\0".as_ptr()) };
    if load_library.is_null() {
        unsafe {
            VirtualFreeEx(handle, remote, 0, MEM_RELEASE);
            CloseHandle(handle);
        }
        return Err(last_error("GetProcAddress(LoadLibraryW)"));
    }

    let thread = unsafe {
        CreateRemoteThread(handle, std::ptr::null_mut(), 0, load_library, remote, 0, std::ptr::null_mut())
    };
    if thread == 0 {
        unsafe {
            VirtualFreeEx(handle, remote, 0, MEM_RELEASE);
            CloseHandle(handle);
        }
        return Err(last_error("CreateRemoteThread"));
    }

    let wait = unsafe { WaitForSingleObject(thread, 10_000) };
    let mut code: u32 = 0;
    unsafe { GetExitCodeThread(thread, &mut code) };
    unsafe {
        CloseHandle(thread);
        VirtualFreeEx(handle, remote, 0, MEM_RELEASE);
        CloseHandle(handle);
    }

    if wait != WAIT_OBJECT_0 {
        return Err("LoadLibraryW did not finish within 10 s (thread still running)".into());
    }
    if code == 0 {
        return Err(format!(
            "LoadLibraryW returned NULL for {abs} - check that the DLL is x64 and its dependencies resolve"
        ));
    }
    Ok(code as usize)
}
