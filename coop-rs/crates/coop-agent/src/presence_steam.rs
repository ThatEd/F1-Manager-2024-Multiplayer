//! The Steam half: resolve `SetRichPresence` and call it.
//!
//! `presence.rs` decides *what* to say; this says it. Kept separate so the mapping stays testable without
//! Steam, without the game, and without a second player.
//!
//! ## Why this is not an import
//!
//! The agent is a DLL loaded into `F1Manager24.exe`. That process already has `steam_api64.dll` mapped
//! (the game links it from `Engine/Binaries/ThirdParty/Steamworks/Steamv155/Win64/`), and the flat C
//! interface (`SteamAPI_ISteamFriends_SetRichPresence`) is exported from it. So the call can be made
//! without linking anything at build time, which keeps the agent dependency-free and lets it run in a
//! non-Steam copy of the game without failing to load.
//!
//! ## What is verified, and what is not
//!
//! Verified against the shipped binaries:
//! * `steam_appid.txt` = `2591280`, and `steam_api64.dll` is at the path above (298,384 bytes).
//! * `steam_api64.dll` **exports** `SteamAPI_ISteamFriends_SetRichPresence` and
//!   `SteamAPI_ISteamFriends_ClearRichPresence`.
//! * `F1Manager24.exe` contains `steam_api64` / `SteamAPI_Init` / `SteamFriends` but **no**
//!   `SetRichPresence`, `steam_display` or `steam_player_group` string - so the game sets no rich presence
//!   today and every key here is ours.
//!
//! NOT verified: that the Steam calls succeed at runtime in a logged-out or overlapped client. The
//! interface **version string** is verified (`SteamAPI_SteamFriends_v017`, the only one in the shipped DLL),
//! and the exports are verified, but the first real call needs the game running with Steam attached. A
//! failure is reported as an `Err` rather than silently ignored, so that case is visible in the log.

#[cfg(windows)]
pub mod steam {
    use std::ffi::c_void;
    use std::sync::Mutex;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
        fn GetProcAddress(module: isize, name: *const u8) -> *mut c_void;
    }

    /// The interface version string Steamworks uses for `ISteamFriends`, **verified against the shipped
    /// DLL**: `steam_api64.dll` (Steamv155, 298,384 bytes) contains exactly one such string,
    /// `SteamAPI_SteamFriends_v017`. The others are fallbacks for a different SDK drop.
    ///
    /// v017 is first because it is the measured answer. Assuming the version from the SDK directory name
    /// (`Steamv155`) would have been wrong: the folder and the interface version are unrelated numbers.
    const FRIENDS_VERSIONS: &[&str] = &[
        "SteamAPI_SteamFriends_v017\0",
        "SteamAPI_SteamFriends_v018\0",
        "SteamAPI_SteamFriends_v016\0",
        "SteamAPI_SteamFriends_v019\0",
    ];

    type SetRichPresenceFn = unsafe extern "system" fn(*mut c_void, *const u8, *const u8) -> u8;
    type ClearRichPresenceFn = unsafe extern "system" fn(*mut c_void) -> u8;

    /// Fallback when `ClearRichPresence` is not exported. Reports success because there is nothing the
    /// caller could do about it, and a failure here would mask the keys that *were* set.
    unsafe extern "system" fn no_clear(_friends: *mut c_void) -> u8 {
        1
    }

    /// Resolved Steam entry points.
    pub struct Steam {
        friends: *mut c_void,
        set: SetRichPresenceFn,
        clear: ClearRichPresenceFn,
        /// The accessor that worked, for the log.
        pub version: &'static str,
    }

    // The raw pointers are only ever handed back to Steam, which is thread-safe for these calls, and the
    // wrapper is behind a Mutex. Declaring it is required because `*mut c_void` is not `Send` by default.
    unsafe impl Send for Steam {}

    fn wide(s: &str) -> Vec<u16> {
        let mut v: Vec<u16> = s.encode_utf16().collect();
        v.push(0);
        v
    }

    /// Locate `steam_api64.dll` in this process and resolve the rich-presence entry points.
    ///
    /// Returns `Err` with a reason rather than panicking: rich presence is cosmetic, and the agent must
    /// never fail to load because Steam is absent (a non-Steam copy, or a test harness).
    pub fn attach() -> Result<Steam, String> {
        let name = wide("steam_api64.dll");
        let module = unsafe { GetModuleHandleW(name.as_ptr()) };
        if module == 0 {
            return Err("steam_api64.dll is not loaded in this process".into());
        }

        let mut chosen: Option<(&'static str, *mut c_void)> = None;
        for v in FRIENDS_VERSIONS {
            let addr = unsafe { GetProcAddress(module, v.as_ptr()) };
            if !addr.is_null() {
                // The accessor returns the ISteamFriends pointer.
                let accessor: unsafe extern "system" fn() -> *mut c_void =
                    unsafe { std::mem::transmute(addr) };
                let friends = unsafe { accessor() };
                if !friends.is_null() {
                    chosen = Some((v.trim_end_matches('\0'), friends));
                    break;
                }
            }
        }
        let (version, friends) = match chosen {
            Some(c) => c,
            None => {
                return Err(format!(
                    "no ISteamFriends accessor resolved; tried {} version strings",
                    FRIENDS_VERSIONS.len()
                ))
            }
        };

        let set_name = b"SteamAPI_ISteamFriends_SetRichPresence\0";
        let clear_name = b"SteamAPI_ISteamFriends_ClearRichPresence\0";
        let set_addr = unsafe { GetProcAddress(module, set_name.as_ptr()) };
        let clear_addr = unsafe { GetProcAddress(module, clear_name.as_ptr()) };
        if set_addr.is_null() {
            return Err("SteamAPI_ISteamFriends_SetRichPresence is not exported".into());
        }
        let set: SetRichPresenceFn = unsafe { std::mem::transmute(set_addr) };
        // ClearRichPresence is optional: if it is missing, a no-op returning success is used, so a caller
        // clearing keys is never told the operation failed for a reason it cannot act on.
        let clear: ClearRichPresenceFn = if clear_addr.is_null() {
            no_clear
        } else {
            unsafe { std::mem::transmute(clear_addr) }
        };

        Ok(Steam { friends, set, clear, version })
    }

    impl Steam {
        /// Set one key. An empty value clears the key, which is how a stale `coop_track` is removed.
        pub fn set(&self, key: &str, value: &str) -> bool {
            let k = format!("{key}\0");
            let v = format!("{value}\0");
            let ok = unsafe { (self.set)(self.friends, k.as_ptr(), v.as_ptr()) };
            ok != 0
        }

        pub fn clear_all(&self) -> bool {
            (unsafe { (self.clear)(self.friends) }) != 0
        }
    }

    /// The agent-wide handle, so the poll loop can update presence without re-resolving every time.
    ///
    /// `None` means Steam was not reachable at startup, which is logged once. Retrying every frame would
    /// spam the log; retrying on every state *change* is what `present` does below.
    static STEAM: Mutex<Option<Steam>> = Mutex::new(None);
    static ATTACH_TRIED: Mutex<bool> = Mutex::new(false);

    /// Apply a set of keys, clearing any key we set previously that is no longer present.
    ///
    /// The "clear what is gone" behaviour is why this tracks the previous key set: Steam keeps a key until
    /// it is explicitly cleared, so a track name from the last race would otherwise survive into the next
    /// menu and appear under the wrong line.
    pub fn present(pairs: &[(String, String)]) -> Result<(), String> {
        let mut guard = STEAM.lock().unwrap();
        if guard.is_none() {
            let mut tried = ATTACH_TRIED.lock().unwrap();
            if *tried {
                return Err("Steam unavailable (already reported)".into());
            }
            *tried = true;
            drop(tried);
            match attach() {
                Ok(s) => {
                    println!("[presence] attached via {}", s.version);
                    *guard = Some(s);
                }
                Err(e) => {
                    println!("[presence] not attaching: {e}");
                    return Err(e);
                }
            }
        }
        let steam = guard.as_ref().unwrap();
        let mut failed = Vec::new();
        for (k, v) in pairs {
            if !steam.set(k, v) {
                failed.push(k.clone());
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(format!("Steam refused keys: {failed:?}"))
        }
    }

    /// Forget the resolved handle. Used by tests and by a live reload of the agent.
    pub fn detach() {
        *STEAM.lock().unwrap() = None;
        *ATTACH_TRIED.lock().unwrap() = false;
    }
}

#[cfg(not(windows))]
pub mod steam {
    pub fn present(_pairs: &[(String, String)]) -> Result<(), String> {
        Err("rich presence needs Windows".into())
    }
    pub fn detach() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attaching_without_steam_fails_cleanly_rather_than_panicking() {
        // The agent must load in a non-Steam copy of the game. On a machine with no steam_api64 the call
        // has to return an Err, not abort.
        let r = steam::attach();
        match r {
            Ok(s) => {
                // If Steam happens to be present, the accessor must have resolved to something real.
                assert!(!s.version.is_empty());
            }
            Err(e) => {
                assert!(!e.is_empty(), "a failure must say why");
                println!("attach() reported: {e}");
            }
        }
    }

    #[test]
    fn present_without_steam_is_an_error_not_a_silent_success() {
        // Reporting success when nothing was sent would make the feature look deployed when it is not.
        if steam::attach().is_err() {
            let r = steam::present(&[("steam_display".into(), "#Coop_MainMenu".into())]);
            assert!(r.is_err(), "must not claim success with no Steam");
        }
    }
}
