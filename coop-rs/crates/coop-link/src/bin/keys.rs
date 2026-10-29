//! A global hotkey that joins the host session, for the client machine.
//!
//! Why not a Lua/UE4SS keybind: the client install has no UE4SS, adding it means deploying a whole
//! loader into a second machine's game, and the earlier Lua mods froze the client when they did work
//! inside the game's own threads. A `RegisterHotKey` window needs none of that — it is a normal
//! desktop process that receives a keystroke from Windows no matter which window has focus, and it
//! does its work in its own address space. The game does not know it exists.
//!
//! Default key: **Ctrl+Alt+J** (J for join). It launches `coop-link --role client` as a child process,
//! so the link can be restarted by pressing the key again, and the link's output is shown in a console
//! window the player can read.

use std::ffi::c_void;
use std::process::{Child, Command};

#[link(name = "user32")]
extern "system" {
    fn RegisterHotKey(hwnd: isize, id: i32, modifiers: u32, vk: u32) -> i32;
    fn UnregisterHotKey(hwnd: isize, id: i32) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: isize, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
}

#[repr(C)]
struct Msg {
    hwnd: isize,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point_x: i32,
    point_y: i32,
}

const WM_HOTKEY: u32 = 0x0312;
const MOD_ALT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0002;
const MOD_SHIFT: u32 = 0x0004;
const MOD_NOREPEAT: u32 = 0x4000;
const VK_J: u32 = 0x4A;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn main() {
    // Ctrl+Alt+J, or Ctrl+Alt+Shift+J if the simple one is already taken by another program.
    let combos: [(u32, &str); 2] = [
        (MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, "Ctrl+Alt+J"),
        (MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT, "Ctrl+Alt+Shift+J"),
    ];

    let mut registered: Option<(u32, &str)> = None;
    for (mods, label) in combos {
        if unsafe { RegisterHotKey(0, 1, mods, VK_J) } != 0 {
            registered = Some((mods, label));
            break;
        }
    }

    let Some((_, label)) = registered else {
        let text = "Could not register a hotkey: Ctrl+Alt+J and Ctrl+Alt+Shift+J are both taken by \
                    another program. Close that program, or run coop-link manually.";
        unsafe {
            MessageBoxW(
                0,
                wide(text).as_ptr(),
                wide("coop-keys").as_ptr(),
                0x30,
            );
        }
        std::process::exit(1);
    };

    // Tell the player what to press and how to stop it. A console window is the only UI available
    // without adding one, and a hidden process the player cannot kill is worse than a visible one.
    println!("coop-keys: press {label} to join the host session.");
    println!("The link launches as a child process and stays up until it fails or you close it.");
    println!("Close this window to stop listening.\n");

    let mut link: Option<Child> = None;
    let mut msg: Msg = unsafe { std::mem::zeroed() };

    loop {
        let got = unsafe { GetMessageW(&mut msg, 0, 0, 0) };
        if got == 0 || got == -1 {
            break;
        }
        if msg.message == WM_HOTKEY {
            // Restart rather than double up: pressing twice should give one working link, not two
            // processes fighting over the same save file.
            if let Some(child) = link.as_mut() {
                if child.try_wait().ok().flatten().is_none() {
                    let _ = child.kill();
                    println!("[{label}] stopped the previous link; starting a fresh one");
                }
            }
            match spawn_link() {
                Ok(child) => {
                    println!("[{label}] link started (pid {})", child.id());
                    link = Some(child);
                }
                Err(e) => {
                    println!("[{label}] could not start the link: {e}");
                    link = None;
                }
            }
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        UnregisterHotKey(0, 1);
    }
}

/// Starts `coop-link --role client` beside this executable, in its own console window.
///
/// A separate console matters: the link prints the session offer, the hash check result and any
/// refusal, and that is exactly the text a player needs when a join does not work.
fn spawn_link() -> Result<Child, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot locate this executable: {e}"))?
        .with_file_name("coop-link.exe");
    if !exe.exists() {
        return Err(format!("{} is missing", exe.display()));
    }
    // The supervisor address comes from coop_supervisor.txt, which coop-link reads itself.
    let mut cmd = Command::new(&exe);
    cmd.args(["--role", "client"]);
    if let Some(addr) = supervisor_address() {
        cmd.args(["--supervisor", &addr]);
    }
    cmd.current_dir(exe.parent().unwrap_or(std::path::Path::new(".")));
    cmd.spawn()
        .map_err(|e| format!("could not launch {}: {e}", exe.display()))
}

/// Reads `coop_supervisor.txt` next to the executable, the same file the in-process agent uses.
fn supervisor_address() -> Option<String> {
    let path = std::env::current_exe().ok()?.with_file_name("coop_supervisor.txt");
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().next()?.trim().to_string();
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

// `c_void` is used by the FFI signatures above; keep the import meaningful for that.
const _: Option<*mut c_void> = None;
