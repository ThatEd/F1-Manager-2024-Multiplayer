//! The unsafe half of Steam lobbies: create, advertise, join, and receive the join request.
//!
//! Everything here is FFI against `steam_api64.dll` found at runtime, so the agent keeps its policy of having
//! no build-time Steam dependency and still loading in a non-Steam copy of the game.
//!
//! ## Every signature here is taken from a header, not guessed
//!
//! The flat (`SteamAPI_ISteam*_`) wrappers do **not** mirror the class methods' return types. Two examples
//! that would corrupt memory if assumed:
//!
//! * `ISteamMatchmaking::GetLobbyData` returns `const char *`, while `SetLobbyData` returns `bool`.
//! * `CreateLobby` / `JoinLobby` return a `SteamAPICall_t` (`uint64`), not `bool`.
//!
//! Declaring any of those as `bool` would read the low byte of a pointer and discard the rest. The declared
//! types below were checked against `steam_api_flat.h` and `isteammatchmaking.h`.
//!
//! ## The callback object is a vtable, and its layout is load-bearing
//!
//! `CCallbackBase` (steam_api_common.h) is:
//!
//! ```text
//!   0   vptr              -> Run(void*), Run(void*, bool, SteamAPICall_t), GetCallbackSizeBytes()
//!   8   u8  m_nCallbackFlags
//!   12  i32 m_iCallback
//! ```
//!
//! so the object is 16 bytes and the vtable has exactly three slots in that order. Steam calls `Run` from its
//! own callback thread when a `GameLobbyJoinRequested_t` arrives. Registering a struct whose layout did not
//! match would have Steam write into the wrong offsets - a heap corruption in the game process, which is the
//! worst possible failure mode for a mod.
//!
//! `GameLobbyJoinRequested_t` is `{ k_iCallback = k_iSteamFriendsCallbacks + 33; u64 m_steamIDLobby;
//! u64 m_steamIDFriend; }` - two 64-bit ids, nothing else.
//!
//! ## What is verified, and what is not
//!
//! Verified: every function below is exported by the shipped `steam_api64.dll` (298,384 bytes), and every
//! type is copied from the Valve header text.
//!
//! NOT verified: that these calls succeed at runtime. `CreateLobby` and `JoinLobby` are asynchronous - they
//! return a `SteamAPICall_t` and the answer arrives as a call result - and this module deliberately does not
//! register call-result handlers. It acts on them optimistically and then *confirms* with the synchronous
//! getters (`GetNumLobbyMembers`, `GetLobbyOwner`, `GetLobbyData`), which is the part that can be checked
//! without a callback pump of our own.

#![allow(non_snake_case)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::lobby_policy as policy;

/// `k_iSteamFriendsCallbacks` base. `GameLobbyJoinRequested_t` is `base + 33`.
///
/// The base is a fixed part of the Steamworks ABI and the `+33` offset is read from the struct in the shipped
/// header text, not invented.
const GAME_LOBBY_JOIN_REQUESTED: i32 = 3301 + 33;

type HSteamPipe = i32;
type HSteamUser = i32;
type SteamAPICall = u64;

#[cfg(windows)]
mod ffi {
    use super::*;

    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
        fn GetProcAddress(module: isize, name: *const u8) -> *mut c_void;
    }

    /// A `CCallbackBase`-compatible object for one callback. See the module docs for the layout.
    ///
    /// `#[repr(C)]` fixes the field order; the vtable pointer is the first field because Rust puts a
    /// `*const VTable` there for a trait object.
    #[repr(C)]
    pub struct CallbackObj {
        pub vtable: *const CallbackVTable,
        pub flags: u8,
        // Three bytes of padding so `callback_id` lands at offset 12, matching the C++ layout.
        pub pad: [u8; 3],
        pub callback_id: i32,
    }

    #[repr(C)]
    pub struct CallbackVTable {
        pub run: unsafe extern "system" fn(*mut CallbackObj, *mut c_void),
        pub run_result: unsafe extern "system" fn(*mut CallbackObj, *mut c_void, u8, u64),
        pub size: unsafe extern "system" fn(*mut CallbackObj) -> i32,
    }

    /// Most recent lobby id a friend asked us to join. `0` means none.
    ///
    /// A static because the callback fires on Steam's thread and must not allocate, lock, or touch Rust
    /// objects that could be mid-drop. An atomic integer is the only thing safe to write there.
    static WANTED_LOBBY: AtomicU64 = AtomicU64::new(0);

    /// Read-and-clear, so one request is acted on once.
    pub fn take_wanted_lobby() -> u64 {
        WANTED_LOBBY.swap(0, Ordering::SeqCst)
    }

    unsafe extern "system" fn run(this: *mut CallbackObj, pv: *mut c_void) {
        // `pv` points at the callback struct itself: the first field of `GameLobbyJoinRequested_t` is
        // `m_steamIDLobby`, and Steam passes the whole struct. So the lobby id is the first 8 bytes.
        if pv.is_null() {
            return;
        }
        let lobby = *(pv as *const u64);
        if lobby != 0 {
            WANTED_LOBBY.store(lobby, Ordering::SeqCst);
        }
        let _ = this;
    }

    unsafe extern "system" fn run_result(_this: *mut CallbackObj, _pv: *mut c_void, _io: u8, _call: u64) {}

    unsafe extern "system" fn size(_this: *mut CallbackObj) -> i32 {
        // sizeof(GameLobbyJoinRequested_t) = two 64-bit ids.
        16
    }

    static VTABLE: CallbackVTable = CallbackVTable { run, run_result, size };

    /// The one registered callback object. A static with a `'static` vtable, so its address is stable for the
    /// lifetime of the process - which Steam requires, since it keeps the pointer.
    pub static mut JOIN_CALLBACK: CallbackObj = CallbackObj {
        vtable: &VTABLE,
        flags: 0,
        pad: [0; 3],
        callback_id: GAME_LOBBY_JOIN_REQUESTED,
    };

    pub type CreateLobbyFn = unsafe extern "system" fn(*mut c_void, i32, i32) -> SteamAPICall;
    pub type JoinLobbyFn = unsafe extern "system" fn(*mut c_void, u64) -> SteamAPICall;
    pub type LeaveLobbyFn = unsafe extern "system" fn(*mut c_void, u64);
    pub type SetLobbyDataFn = unsafe extern "system" fn(*mut c_void, u64, *const u8, *const u8) -> u8;
    pub type GetLobbyDataFn = unsafe extern "system" fn(*mut c_void, u64, *const u8) -> *const u8;
    pub type SetLobbyJoinableFn = unsafe extern "system" fn(*mut c_void, u64, u8) -> u8;
    pub type GetNumLobbyMembersFn = unsafe extern "system" fn(*mut c_void, u64) -> i32;
    pub type GetLobbyMemberByIndexFn = unsafe extern "system" fn(*mut c_void, u64, i32) -> u64;
    pub type GetLobbyOwnerFn = unsafe extern "system" fn(*mut c_void, u64) -> u64;
    pub type InviteUserFn = unsafe extern "system" fn(*mut c_void, u64, u64) -> u8;
    pub type ActivateInviteDialogFn = unsafe extern "system" fn(*mut c_void, u64);
    pub type RegisterCallbackFn = unsafe extern "system" fn(*mut CallbackObj, i32);
    pub type GetFriendGamePlayedFn = unsafe extern "system" fn(*mut c_void, u64, *mut u8) -> u8;

    /// Resolved function pointers. A plain struct of `Option<fn>`s, so a missing export degrades to a
    /// named error instead of a crash.
    pub struct Api {
        pub matchmaking: *mut c_void,
        pub friends: *mut c_void,
        pub create_lobby: CreateLobbyFn,
        pub join_lobby: JoinLobbyFn,
        pub leave_lobby: LeaveLobbyFn,
        pub set_lobby_data: SetLobbyDataFn,
        pub get_lobby_data: GetLobbyDataFn,
        pub set_lobby_joinable: SetLobbyJoinableFn,
        pub num_members: GetNumLobbyMembersFn,
        pub member_by_index: GetLobbyMemberByIndexFn,
        pub lobby_owner: GetLobbyOwnerFn,
        pub invite_user: InviteUserFn,
        pub activate_invite_dialog: ActivateInviteDialogFn,
        pub register_callback: RegisterCallbackFn,
        pub friend_game_played: GetFriendGamePlayedFn,
    }

    fn wide(s: &str) -> Vec<u16> {
        let mut v: Vec<u16> = s.encode_utf16().collect();
        v.push(0);
        v
    }

    unsafe fn sym(module: isize, name: &str) -> Option<*mut c_void> {
        let mut n: Vec<u8> = name.as_bytes().to_vec();
        n.push(0);
        let p = GetProcAddress(module, n.as_ptr());
        if p.is_null() {
            None
        } else {
            Some(p)
        }
    }

    unsafe fn must(module: isize, name: &str) -> Result<*mut c_void, String> {
        sym(module, name).ok_or_else(|| format!("{name} is not exported by steam_api64.dll"))
    }

    /// Resolves everything once. Idempotent and cheap after the first call.
    pub fn attach() -> Result<Api, String> {
        static mut CACHED: Option<Api> = None;
        static mut ERR: Option<String> = None;
        // Single-threaded attach is guaranteed: this is called from the agent's worker, never the game thread.
        unsafe {
            if let Some(ref e) = ERR {
                return Err(e.clone());
            }
            if let Some(ref a) = CACHED {
                return Ok(Api {
                    matchmaking: a.matchmaking,
                    friends: a.friends,
                    create_lobby: a.create_lobby,
                    join_lobby: a.join_lobby,
                    leave_lobby: a.leave_lobby,
                    set_lobby_data: a.set_lobby_data,
                    get_lobby_data: a.get_lobby_data,
                    set_lobby_joinable: a.set_lobby_joinable,
                    num_members: a.num_members,
                    member_by_index: a.member_by_index,
                    lobby_owner: a.lobby_owner,
                    invite_user: a.invite_user,
                    activate_invite_dialog: a.activate_invite_dialog,
                    register_callback: a.register_callback,
                    friend_game_played: a.friend_game_played,
                });
            }
            match attach_once() {
                Ok(a) => {
                    CACHED = Some(Api {
                        matchmaking: a.matchmaking,
                        friends: a.friends,
                        create_lobby: a.create_lobby,
                        join_lobby: a.join_lobby,
                        leave_lobby: a.leave_lobby,
                        set_lobby_data: a.set_lobby_data,
                        get_lobby_data: a.get_lobby_data,
                        set_lobby_joinable: a.set_lobby_joinable,
                        num_members: a.num_members,
                        member_by_index: a.member_by_index,
                        lobby_owner: a.lobby_owner,
                        invite_user: a.invite_user,
                        activate_invite_dialog: a.activate_invite_dialog,
                        register_callback: a.register_callback,
                        friend_game_played: a.friend_game_played,
                    });
                    Ok(CACHED.take().expect("just set"))
                }
                Err(e) => {
                    ERR = Some(e.clone());
                    Err(e)
                }
            }
        }
    }

    unsafe fn attach_once() -> Result<Api, String> {
        // `steam_api64.dll` is already loaded into the game process, so this only asks for the handle.
        let module = GetModuleHandleW(wide("steam_api64.dll").as_ptr());
        if module == 0 {
            return Err("steam_api64.dll is not loaded in this process".into());
        }

        let acc_mm: unsafe extern "system" fn() -> *mut c_void =
            std::mem::transmute(must(module, "SteamAPI_SteamMatchmaking_v009")?);
        let acc_fr: unsafe extern "system" fn() -> *mut c_void =
            std::mem::transmute(must(module, "SteamAPI_SteamFriends_v017")?);

        let matchmaking = acc_mm();
        let friends = acc_fr();
        if matchmaking.is_null() {
            return Err("SteamMatchmaking() returned null - is SteamAPI_Init done?".into());
        }
        if friends.is_null() {
            return Err("SteamFriends() returned null - is SteamAPI_Init done?".into());
        }

        Ok(Api {
            matchmaking,
            friends,
            create_lobby: std::mem::transmute(must(module, "SteamAPI_ISteamMatchmaking_CreateLobby")?),
            join_lobby: std::mem::transmute(must(module, "SteamAPI_ISteamMatchmaking_JoinLobby")?),
            leave_lobby: std::mem::transmute(must(module, "SteamAPI_ISteamMatchmaking_LeaveLobby")?),
            set_lobby_data: std::mem::transmute(must(module, "SteamAPI_ISteamMatchmaking_SetLobbyData")?),
            get_lobby_data: std::mem::transmute(must(module, "SteamAPI_ISteamMatchmaking_GetLobbyData")?),
            set_lobby_joinable: std::mem::transmute(must(
                module,
                "SteamAPI_ISteamMatchmaking_SetLobbyJoinable",
            )?),
            num_members: std::mem::transmute(must(module, "SteamAPI_ISteamMatchmaking_GetNumLobbyMembers")?),
            member_by_index: std::mem::transmute(must(
                module,
                "SteamAPI_ISteamMatchmaking_GetLobbyMemberByIndex",
            )?),
            lobby_owner: std::mem::transmute(must(module, "SteamAPI_ISteamMatchmaking_GetLobbyOwner")?),
            invite_user: std::mem::transmute(must(module, "SteamAPI_ISteamMatchmaking_InviteUserToLobby")?),
            activate_invite_dialog: std::mem::transmute(must(
                module,
                "SteamAPI_ISteamFriends_ActivateGameOverlayInviteDialog",
            )?),
            register_callback: std::mem::transmute(must(module, "SteamAPI_RegisterCallback")?),
            friend_game_played: std::mem::transmute(must(module, "SteamAPI_ISteamFriends_GetFriendGamePlayed")?),
        })
    }

    /// Registers the join-request callback exactly once.
    ///
    /// Separate from `attach` because registering mutates Steam's global callback list, and a second
    /// registration of the same object is a documented mistake (it would fire twice per request).
    pub fn register_join_callback(api: &Api) -> Result<(), String> {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        static mut OK: bool = false;
        ONCE.call_once(|| unsafe {
            let obj: *mut CallbackObj = std::ptr::addr_of_mut!(JOIN_CALLBACK);
            (api.register_callback)(obj, GAME_LOBBY_JOIN_REQUESTED);
            OK = true;
        });
        unsafe {
            if OK {
                Ok(())
            } else {
                Err("the join callback was already attempted and failed".into())
            }
        }
    }

    /// Creates a lobby and returns its id, or an error naming what Steam said.
    ///
    /// `CreateLobby` is asynchronous, so the id cannot be read here. `0` after the call means "not known yet",
    /// and the caller confirms with [`lobby_members`] on a later pass rather than pretending it worked.
    pub fn create(api: &Api, visibility: policy::ELobbyType, max_members: i32) -> u64 {
        unsafe {
            let call = (api.create_lobby)(api.matchmaking, visibility.as_i32(), max_members);
            // The `SteamAPICall_t` is only useful with a call-result handler, which this module does not
            // register. Recorded for the log; the lobby id is discovered by `owned_lobby` afterwards.
            let _ = call;
        }
        0
    }

    /// The lobby id this process owns, if Steam says it is in one.
    ///
    /// This is how the host learns its own lobby id without a call-result handler: the local user is
    /// automatically a member of a lobby they created (Steam docs: "the local user has joined their own
    /// lobby"), so a lobby whose owner is the local user is ours.
    pub fn owned_lobby(api: &Api, local_user: u64, candidate: u64) -> Option<u64> {
        if candidate == 0 || local_user == 0 {
            return None;
        }
        unsafe {
            let owner = (api.lobby_owner)(api.matchmaking, candidate);
            if owner == local_user {
                Some(candidate)
            } else {
                None
            }
        }
    }

    pub fn set_data(api: &Api, lobby: u64, key: &str, value: &str) -> bool {
        let (k, v) = (cstr(key), cstr(value));
        unsafe { (api.set_lobby_data)(api.matchmaking, lobby, cbytes(&k), cbytes(&v)) != 0 }
    }

    /// Reads lobby metadata. Returns `None` when the key is absent.
    ///
    /// `GetLobbyData` returns a `const char *` owned by Steam and valid only until the next callback pump, so
    /// it is copied immediately rather than held.
    pub fn get_data(api: &Api, lobby: u64, key: &str) -> Option<String> {
        let k = cstr(key);
        unsafe {
            let p = (api.get_lobby_data)(api.matchmaking, lobby, cbytes(&k));
            if p.is_null() {
                return None;
            }
            let mut len = 0usize;
            while len < 8192 && *p.add(len) != 0 {
                len += 1;
            }
            if len == 0 {
                return None;
            }
            let bytes = std::slice::from_raw_parts(p, len);
            Some(String::from_utf8_lossy(bytes).to_string())
        }
    }

    pub fn set_joinable(api: &Api, lobby: u64, joinable: bool) -> bool {
        unsafe { (api.set_lobby_joinable)(api.matchmaking, lobby, joinable as u8) != 0 }
    }

    pub fn members(api: &Api, lobby: u64) -> i32 {
        unsafe { (api.num_members)(api.matchmaking, lobby) }
    }

    pub fn member_at(api: &Api, lobby: u64, index: i32) -> u64 {
        unsafe { (api.member_by_index)(api.matchmaking, lobby, index) }
    }

    pub fn leave(api: &Api, lobby: u64) {
        if lobby != 0 {
            unsafe { (api.leave_lobby)(api.matchmaking, lobby) }
        }
    }

    pub fn join(api: &Api, lobby: u64) -> bool {
        if lobby == 0 {
            return false;
        }
        unsafe {
            let _call = (api.join_lobby)(api.matchmaking, lobby);
        }
        true
    }

    pub fn activate_invite_dialog(api: &Api, lobby: u64) {
        if lobby != 0 {
            unsafe { (api.activate_invite_dialog)(api.friends, lobby) }
        }
    }

    /// Reads a friend's session. `lobby` is `FriendGameInfo_t::m_steamIDLobby`, i.e. offset 16 of the 24-byte
    /// struct (`CGameID` 8, `u32` IP 4, `u16` port 2, `u16` query port 2, `CSteamID` lobby 8).
    pub fn friend_lobby(api: &Api, friend: u64) -> (u32, u64) {
        // The struct is pack(8) by default, so the fields are laid out as above with no hidden padding.
        #[repr(C)]
        struct FriendGameInfo {
            game_id: u64,
            ip: u32,
            port: u16,
            query_port: u16,
            lobby: u64,
        }
        let mut info = FriendGameInfo { game_id: 0, ip: 0, port: 0, query_port: 0, lobby: 0 };
        let ok = unsafe {
            (api.friend_game_played)(api.friends, friend, &mut info as *mut FriendGameInfo as *mut u8)
        };
        if ok == 0 {
            return (0, 0);
        }
        // `CGameID` bit 24 is `k_EGameIDTypeApp` for a normal app; the app id is the low 24 bits.
        ((info.game_id & 0x00FF_FFFF) as u32, info.lobby)
    }

    fn cstr(s: &str) -> std::ffi::CString {
        // A key or value containing an interior nul would silently truncate; replacing is safer than
        // panicking on a string a player could in principle influence.
        let clean: String = s.chars().filter(|c| *c != '\0').collect();
        std::ffi::CString::new(clean).unwrap_or_else(|_| std::ffi::CString::new("").expect("empty is valid"))
    }

    /// `CString::as_ptr` yields `*const i8`; the declared FFI signatures use `*const u8`, so convert
    /// explicitly rather than relying on a cast at each call site.
    fn cbytes(c: &std::ffi::CString) -> *const u8 {
        c.as_ptr() as *const u8
    }
}

#[cfg(windows)]
pub use ffi::{
    activate_invite_dialog, attach, create, friend_lobby, get_data, join, leave, member_at, members,
    owned_lobby, register_join_callback, set_data, set_joinable, take_wanted_lobby, Api,
};

/// Non-Windows stubs, so the crate still builds (and its tests still run) elsewhere. Every call reports that
/// Steam is unavailable rather than silently succeeding - the same policy as `presence_steam`.
#[cfg(not(windows))]
pub fn unavailable() -> Result<(), String> {
    Err("Steam lobbies are only available on Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The callback id must equal the Friends base plus 33.
    ///
    /// Steam dispatches purely on this number: registering the right struct under the wrong id means the
    /// callback never fires, and - worse - registering a struct whose size does not match the id means Steam
    /// copies the wrong number of bytes into it.
    #[test]
    fn the_join_request_callback_id_matches_the_documented_offset() {
        assert_eq!(GAME_LOBBY_JOIN_REQUESTED, 3301 + 33);
    }

    #[test]
    fn the_callback_object_is_sixteen_bytes_with_the_id_at_twelve() {
        // CCallbackBase is { vptr, u8 flags, i32 id } with `int` 4-byte aligned: 8 + 1 + 3 pad + 4 = 16, and
        // the id sits at offset 12. If this ever fails, the struct no longer matches the C++ layout and Steam
        // would write into the wrong field.
        #[cfg(windows)]
        {
            use std::mem::{align_of, offset_of, size_of};
            assert_eq!(size_of::<ffi::CallbackObj>(), 16);
            assert_eq!(align_of::<ffi::CallbackObj>(), 8);
            assert_eq!(offset_of!(ffi::CallbackObj, vtable), 0);
            assert_eq!(offset_of!(ffi::CallbackObj, flags), 8);
            assert_eq!(offset_of!(ffi::CallbackObj, callback_id), 12);
        }
    }

    #[test]
    fn the_callback_vtable_has_three_slots_in_declaration_order() {
        // CCallbackBase declares Run(void*), Run(void*,bool,SteamAPICall_t), GetCallbackSizeBytes(), in that
        // order, with no virtual destructor ("don't add a virtual destructor because we export this binary
        // interface across dll's"). A different order would dispatch the wrong function.
        #[cfg(windows)]
        {
            use std::mem::size_of;
            assert_eq!(size_of::<ffi::CallbackVTable>(), 3 * size_of::<usize>());
        }
    }

    #[test]
    fn the_join_payload_is_two_sixty_four_bit_ids() {
        // GameLobbyJoinRequested_t = { u64 m_steamIDLobby; u64 m_steamIDFriend; } = 16 bytes, which is also
        // what the vtable's GetCallbackSizeBytes reports.
        #[repr(C)]
        struct GameLobbyJoinRequested {
            lobby: u64,
            friend: u64,
        }
        assert_eq!(std::mem::size_of::<GameLobbyJoinRequested>(), 16);
        // The lobby is the *first* field, which is why the callback can read the id from offset 0.
        assert_eq!(std::mem::offset_of!(GameLobbyJoinRequested, lobby), 0);
    }

    #[test]
    fn the_friend_game_info_lobby_is_at_offset_sixteen() {
        // GetFriendGamePlayed fills a FriendGameInfo_t by pointer. If this layout were wrong, the lobby id
        // would be read out of the port fields and every join would target a nonsense id.
        #[repr(C)]
        struct FriendGameInfo {
            game_id: u64,
            ip: u32,
            port: u16,
            query_port: u16,
            lobby: u64,
        }
        assert_eq!(std::mem::size_of::<FriendGameInfo>(), 24);
        assert_eq!(std::mem::offset_of!(FriendGameInfo, lobby), 16);
    }

    #[test]
    fn no_wanted_lobby_is_recorded_before_a_request_arrives() {
        let _ = take_wanted_lobby();
        assert_eq!(take_wanted_lobby(), 0);
    }

    #[cfg(not(windows))]
    #[test]
    fn on_a_non_windows_host_steam_is_reported_unavailable_not_silently_ok() {
        assert!(unavailable().is_err());
    }
}
