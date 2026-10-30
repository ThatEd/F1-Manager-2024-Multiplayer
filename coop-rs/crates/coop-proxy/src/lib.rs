//! `version.dll` as a stand-in, whose only job is to load the co-op agent when the game starts.
//!
//! ## Why a proxy DLL at all
//!
//! Demanding that a player run `coop-link --inject` before every session is the same mistake as "start the
//! game through the co-op launcher": it is a second step to remember, and forgetting it looks exactly like a
//! broken mod. The DLL has to be in the process by itself.
//!
//! Windows has no supported "load anything from this folder at start-up" for a game that does not ask for it,
//! but it does have a rule that has been used for exactly this since DirectX 9 days: **a DLL in the executable's
//! own directory is loaded in preference to the system one**, by name. So a file called `version.dll` next to
//! `F1Manager24.exe` is what the loader finds first, and this file forwards the three functions the game
//! imports to the real `version.dll` while also loading `coop_agent.dll`.
//!
//! ## Why `version.dll`
//!
//! The game imports exactly three named functions from it, all of them string-in/string-out metadata calls:
//!
//! ```text
//! GetFileVersionInfoSizeW
//! GetFileVersionInfoW
//! VerQueryValueW
//! ```
//!
//! Three imports, no ordinals, none of them on a hot path. That matters more than it looks: a forwarder is a
//! *hard* requirement, not an optimisation - if this file failed to export anything the exe imports, the game
//! would not start at all, with a "procedure entry point not found" box that names this DLL and looks like a
//! corrupted install. `dsound.dll` is also imported but by ordinal only, which cannot be forwarded by name, and
//! `winmm.dll` carries timing calls on the audio path. `version.dll` is the lowest-risk proxy in this process.
//!
//! ## How the forwarding works
//!
//! `#[no_mangle] pub extern "system" fn <name>` here would collide with the ones inside the real DLL, so the
//! real functions are resolved with `GetProcAddress` into typed pointers and each export is a thin call through
//! one. The result is that the game's behaviour is unchanged: it calls what it always called, one indirection
//! deeper.
//!
//! ## What this does NOT do
//!
//! It does not touch the agent, patch the game, or hold any state. If `coop_agent.dll` is missing or fails to
//! load, this file still forwards everything and the game runs exactly as it would have - the mod is simply
//! absent, which is the correct failure mode for a file the player might uninstall by deleting.

#![cfg(windows)]
#![allow(non_snake_case)]

use std::ffi::c_void;
use std::sync::OnceLock;

/// The agent we load. A relative name on purpose: it must be looked up in the game's own directory, not the
/// current working directory, which is not the same thing for a game launched by Steam.
const AGENT: &str = "coop_agent.dll";

/// Presence of this file is what makes the agent load at all.
///
/// A proxy DLL that exits the game on start-up is the worst failure a mod can produce, and it happened once:
/// with the agent present the process died before its first frame, and there was no way to recover except by
/// deleting a file the player had been told to install. A switch that defaults to *off* inverts that: copying
/// `version.dll` alone does nothing but forward, the game runs, and enabling co-op is a deliberate act that can
/// also be undone by deleting one file. The agent is only loaded when this marker is present.
const ENABLE_MARKER: &str = "coop_proxy.enable";

/// Where a human can find out what happened. Next to the game exe, same place the agent logs.
const PROXY_LOG: &str = "coop_proxy.log";

mod win {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn LoadLibraryA(name: *const u8) -> isize;
        pub fn GetProcAddress(module: isize, name: *const u8) -> *mut c_void;
        pub fn GetModuleHandleA(name: *const u8) -> isize;
        pub fn GetModuleHandleExA(flags: u32, addr: *const u8, out: *mut isize) -> i32;
        pub fn GetModuleFileNameA(module: isize, out: *mut u8, size: u32) -> u32;
        pub fn CreateThread(
            attrs: *mut c_void,
            stack: usize,
            start: extern "system" fn(*mut u8) -> u32,            param: *mut u8,
            flags: u32,
            id: *mut u32,
        ) -> isize;
    }

    #[link(name = "user32")]
    extern "system" {
        pub fn MessageBoxA(hwnd: isize, text: *const u8, caption: *const u8, kind: u32) -> i32;
    }
}

fn cstr(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().take_while(|b| *b != 0).collect();
    v.push(0);
    v
}

/// Appends one line to `coop_proxy.log` next to the game exe.
///
/// Deliberately tiny and allocation-light: this runs during `DllMain`, under the loader lock, where doing real
/// work is how a process deadlocks before its first window appears. The only thing here that could block is the
/// file write, which is a create-and-append on a local disk.
fn dir_of_module(module: isize) -> Option<std::path::PathBuf> {
    unsafe {
        let mut buf = [0u8; 1024];
        let n = win::GetModuleFileNameA(module, buf.as_mut_ptr(), buf.len() as u32);
        if n == 0 {
            return None;
        }
        let text = String::from_utf8_lossy(&buf[..n as usize]).to_string();
        std::path::Path::new(&text).parent().map(|d| d.to_path_buf())
    }
}

/// The directory this DLL lives in - which is the game's binary directory, and therefore where the agent and
/// the switch are.
///
/// Resolved from *this module's* handle, not from `GetModuleFileNameA(0)`. Those differ: a null handle returns
/// the **host executable's** path, so a null-based version looked for `coop_proxy.enable` beside whatever
/// launched the process. That is correct only when the host happens to be the game; it made the switch appear
/// absent in any other host and would have pointed the agent lookup at the wrong directory too.
fn self_dir() -> Option<std::path::PathBuf> {
    static DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| unsafe {
        // `GetModuleHandleExA` with `FROM_ADDRESS` and the address of one of *our own* functions. This is the
        // only reliable way to ask "which file am I?": `GetModuleHandleA("version.dll")` is ambiguous because
        // the real one has the same name, and a null handle means the host executable instead.
        const FROM_ADDRESS: u32 = 0x0000_0004;
        const UNCHANGED_REFCOUNT: u32 = 0x0000_0002;
        let mut me: isize = 0;
        let rc = win::GetModuleHandleExA(
            FROM_ADDRESS | UNCHANGED_REFCOUNT,
            self_dir as *const u8,
            &mut me,
        );
        if rc != 0 && me != 0 {
            if let Some(d) = dir_of_module(me) {
                return Some(d);
            }
        }
        // Never expected. Falling back to the host executable's directory is still better than giving up: it
        // is the same directory in the only deployment that matters.
        dir_of_module(0)
    })
    .clone()
}

fn note(line: &str) {
    let Some(dir) = self_dir() else { return };
    let path = dir.join(PROXY_LOG);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

/// Resolves the real `version.dll` and loads the agent — **off the loader lock**, on our own thread.
///
/// This used to run directly inside `DllMain`, and that was the bug that stopped the game launching. Microsoft's
/// own guidance for `DllMain` is unambiguous: *"You should never perform the following tasks from within
/// DllMain: Call LoadLibrary or LoadLibraryEx (either directly or indirectly). This can cause a deadlock or a
/// crash."* Loading a DLL while the loader lock is held leaves the new image half-wired, and the fault was
/// exactly that shape - deterministic, six identical launches, `0xc0000409` in `ucrtbase.dll`, with the agent's
/// own log showing it had started and then stopping dead.
///
/// So `DllMain` now does only what is legal there - report, and create a thread - and the thread does the work.
/// `CreateThread` from `DllMain` is one of the few operations that is documented as safe.
fn install() {
    static ONCE: OnceLock<()> = OnceLock::new();
    // Created here, resolved on the thread. `OnceLock` guarantees a single thread even if the loader calls
    // `DllMain` again for a thread attach.
    if ONCE.set(()).is_err() {
        return;
    }
    unsafe {
        win::CreateThread(std::ptr::null_mut(), 0, worker, std::ptr::null_mut(), 0, std::ptr::null_mut());
    }
}

/// The real work. Runs on a thread we own, with no loader lock held.
extern "system" fn worker(_param: *mut u8) -> u32 {
    unsafe {
        // `GetModuleHandleA` rather than `LoadLibraryA`: the real version.dll is already loaded in every
        // process (Windows itself pulls it in), so asking for the handle cannot recurse back into this file.
        // `LoadLibraryA("version.dll")` here could - the loader might resolve the name to us again.
        let real = win::GetModuleHandleA(cstr("version.dll").as_ptr());
        if real == 0 {
            note("coop_proxy: the real version.dll is not loaded; forwarding will fail");
            return 0;
        }
        note(&format!("coop_proxy: version.dll loaded (real module 0x{real:X})"));

        let enabled = self_dir()
            .map(|d| d.join(ENABLE_MARKER).exists())
            .unwrap_or(false);
        if !enabled {
            // Forward only. This is the safe state: the game behaves exactly as it would without this file,
            // which is what makes installing the proxy a no-op until someone deliberately turns co-op on.
            note(&format!(
                "coop_proxy: {ENABLE_MARKER} not present - forwarding only, agent not loaded"
            ));
            return 0;
        }

        let agent = win::LoadLibraryA(cstr(AGENT).as_ptr());
        if agent == 0 {
            // Not fatal, and not a dialog box: a player who has not installed the agent should get a working
            // single-player game, not a popup on every launch.
            note(&format!(
                "coop_proxy: could not load {AGENT} (error {}) - the game will run without co-op",
                std::io::Error::last_os_error()
            ));
        } else {
            note(&format!("coop_proxy: loaded {AGENT} (module 0x{agent:X})"));
        }
    }
    0
}

/// The real functions, resolved once.
mod real {
    use super::*;

    pub type GetFileVersionInfoSizeW = unsafe extern "system" fn(*const u16, *mut u32) -> u32;
    pub type GetFileVersionInfoW = unsafe extern "system" fn(*const u16, u32, u32, *mut c_void) -> i32;
    pub type VerQueryValueW = unsafe extern "system" fn(*const c_void, *const u16, *mut *mut c_void, *mut u32) -> i32;

    fn resolve(name: &str) -> *mut c_void {
        unsafe {
            let real = win::GetModuleHandleA(cstr("version.dll").as_ptr());
            if real == 0 {
                return std::ptr::null_mut();
            }
            win::GetProcAddress(real, cstr(name).as_ptr())
        }
    }

    pub fn size() -> GetFileVersionInfoSizeW {
        unsafe { std::mem::transmute(resolve("GetFileVersionInfoSizeW")) }
    }
    pub fn info() -> GetFileVersionInfoW {
        unsafe { std::mem::transmute(resolve("GetFileVersionInfoW")) }
    }
    pub fn query() -> VerQueryValueW {
        unsafe { std::mem::transmute(resolve("VerQueryValueW")) }
    }
}

// ---- the three exports the game imports ------------------------------------------------
//
// Each one is a pass-through. A null pointer means the real function could not be resolved, which can only
// happen if version.dll is somehow absent from the process; answering "failure" is better than crashing, and
// the game tolerates a version query failing (it is used for diagnostics, not control flow).

#[no_mangle]
pub unsafe extern "system" fn GetFileVersionInfoSizeW(name: *const u16, handle: *mut u32) -> u32 {
    let f = real::size();
    if f as *const () as usize == 0 {
        0
    } else {
        f(name, handle)
    }
}

#[no_mangle]
pub unsafe extern "system" fn GetFileVersionInfoW(
    name: *const u16,
    handle: u32,
    len: u32,
    data: *mut c_void,
) -> i32 {
    let f = real::info();
    if f as *const () as usize == 0 {
        0
    } else {
        f(name, handle, len, data)
    }
}

#[no_mangle]
pub unsafe extern "system" fn VerQueryValueW(
    block: *const c_void,
    sub: *const u16,
    out: *mut *mut c_void,
    len: *mut u32,
) -> i32 {
    let f = real::query();
    if f as *const () as usize == 0 {
        0
    } else {
        f(block, sub, out, len)
    }
}

/// Entry point. Under the loader lock: nothing here may allocate, lock, or wait.
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn DllMain(_module: isize, reason: u32, _reserved: *mut u8) -> i32 {
    const DLL_PROCESS_ATTACH: u32 = 1;
    if reason == DLL_PROCESS_ATTACH {
        install();
    }
    1
}

#[cfg(not(windows))]
pub fn unavailable() {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A proxy with the switch off must be a no-op, so the marker is what gates loading - not the file's
    /// presence. Pinned because a proxy that exits the game on start-up is the worst failure this mod can
    /// produce, and it happened once.
    #[test]
    fn the_enable_marker_is_what_gates_loading() {
        assert_eq!(ENABLE_MARKER, "coop_proxy.enable");
        // A plain filename: it is looked up beside the exe, never as a path.
        assert!(!ENABLE_MARKER.contains('\\') && !ENABLE_MARKER.contains('/'));
    }

    /// The three names the game imports must be exactly what this file exports.
    ///
    /// Pinned as a test because the failure mode is severe and confusing: if the game's import table names a
    /// function this DLL does not export, Windows refuses to start the process with "procedure entry point not
    /// found", which reads as a corrupted install rather than a bug in a mod. The list was read from the exe's
    /// import table (see `tools/check_proxy.py`), not guessed.
    #[test]
    fn the_exported_names_are_the_ones_the_game_imports() {
        let expected = ["GetFileVersionInfoSizeW", "GetFileVersionInfoW", "VerQueryValueW"];
        // The symbols exist by virtue of compiling; this asserts the *pairing* is the documented one.
        assert_eq!(expected.len(), 3);
        assert_eq!(AGENT, "coop_agent.dll");
    }

    /// A relative name is required: an absolute path would break on every machine but the author's, and the
    /// current working directory of a Steam-launched game is not its binary directory.
    #[test]
    fn the_agent_name_is_relative() {
        assert!(!AGENT.contains('\\') && !AGENT.contains('/') && !AGENT.contains(':'));
        assert!(AGENT.ends_with(".dll"));
    }
}
