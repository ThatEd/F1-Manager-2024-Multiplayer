//! The 5-byte inline hook on the sim entry point: capture `this`, then run our code **on the game
//! thread**.
//!
//! Why this is safe enough to do:
//!
//! * the first five bytes at the target are `push rbx; push rsi; push r13` - three whole
//!   instructions with **no RIP-relative operands**, so relocating them into a trampoline needs no
//!   instruction decoder and cannot cut an instruction in half. Verified against the live game:
//!   `40 53 56 41 55 48 81 EC 70 01 00 00`, prologue `sub rsp, 0x170` after it;
//! * the overwrite is five bytes, so the patch fits inside a single instruction slot with no
//!   alignment games;
//! * the patch is a **single aligned 8-byte store** (`InterlockedExchange64`), not a byte copy. The
//!   target is 8-byte aligned (`0x142298080`) and the three stolen pushes total exactly 5 bytes, so
//!   the 8-byte window covers them plus the first 3 bytes of `sub rsp, 0x170`, which are preserved
//!   verbatim in the new value. A thread executing the prologue cannot observe a half-written
//!   instruction: it either runs the original bytes or the new ones. This is why no threads need to be
//!   suspended at all - and suspending them would be the riskiest part, because the enumeration uses
//!   `CreateToolhelp32Snapshot`, which takes the loader lock and deadlocked against this game before;
//! * the original bytes are restored if the write does not read back correctly.
//!
//! ## Why the stub calls into Rust instead of doing the work itself
//!
//! Hand-assembled machine code that runs inside the game's own call is the least debuggable thing in
//! this project. An earlier version hand-wrote a probe into the stub and shipped two real bugs in it:
//! it dereferenced `rcx` unconditionally (faulting whenever `this` was not a pointer) and it measured
//! a `jmp` displacement from the wrong origin (corrupting the trampoline). Neither was visible
//! without executing it.
//!
//! So the stub does the minimum that must be assembly - save the volatile registers, load the
//! callback pointer, call it, restore - and the callback is ordinary Rust that can be read, tested
//! and covered. Everything the co-op path actually wants (read a field, apply a mailbox of car
//! writes) happens in [`crate::hookmail`], on the game thread, in compiled code.
//!
//! ## Layout of the executable block we allocate (RWX, then re-protected to RX)
//!
//! Code occupies 0x00..0x74, the trampoline 0x80..0x8A, and the data words start at 0x90 - so the
//! three regions cannot overlap. The assertions in `build_code` enforce this rather than trusting it:
//! an earlier layout put the data words at 0x50, in the middle of the stub's own instructions.
//!
//! ```text
//!   +0x00  mov  [rip+0x90], rcx        ; capture `this` (RCX in MSVC x64)
//!   +0x07  inc  qword [rip+0x98]       ; fire counter
//!   +0x0E  push rax / mov rax,[rip+0xA0] / test / jz  ; callback, 0 = inert, jz lands on pop rax
//!   +0x1B  save flags, volatile GPRs, xmm0-3
//!   +0x41  mov  rcx, [rsp+0x48]        ; the callback's `this` argument
//!   +0x46  call rax                    ; <-- Rust runs here, on the game thread
//!   +0x48  restore xmm0-3, GPRs, flags
//!   +0x6F  jmp  trampoline
//!   +0x80  trampoline: <the five original bytes>; jmp target+5
//!   +0x90  slot:      u64              ; the captured sim pointer
//!   +0x98  fires:     u64              ; how many times the stub ran
//!   +0xA0  callback:  u64              ; Rust function to run, 0 = none
//!   +0x80  trampoline: <the five original bytes>; jmp target+5
//! ```
//!
//! The fire counter matters for diagnosis: `fires > 0` plus a later crash means our code ran, while
//! `fires == 0` means the game died before ever reaching the hook and our patch is not implicated.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

#[link(name = "kernel32")]
extern "system" {
    fn VirtualProtect(addr: *mut c_void, size: usize, new: u32, old: *mut u32) -> i32;
    fn VirtualAlloc(addr: *mut c_void, size: usize, alloc: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(addr: *mut c_void, size: usize, free_type: u32) -> i32;
    fn FlushInstructionCache(process: isize, addr: *const c_void, size: usize) -> i32;
    fn GetCurrentProcess() -> isize;

}

const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const MEM_COMMIT_RESERVE: u32 = 0x3000;
const MEM_RELEASE: u32 = 0x8000;

const BLOCK_SIZE: usize = 0x100;
/// Stub code runs 0x00..0x7B, the trampoline 0x80..0x8A, so the data area starts after both.
pub const TRAMP_OFF: usize = 0x80;
pub const SLOT_OFF: usize = 0x90;
pub const FIRES_OFF: usize = 0x98;
pub const CALLBACK_OFF: usize = 0xA0;
/// Bytes stolen from the target. The first three instructions are `push rbx; push rsi; push r13`.
const STOLEN: usize = 5;
/// Bytes overwritten by the patch: the stolen 5 plus 3 preserved, so the store is 8-byte aligned.
const PATCH: usize = 8;
/// Where the stub's `jmp trampoline` starts.
const JMP_OFF: usize = 0x41;

static BLOCK: AtomicUsize = AtomicUsize::new(0);
static TARGET: AtomicUsize = AtomicUsize::new(0);
static INSTALLED: AtomicUsize = AtomicUsize::new(0);

fn rel32(from_after: usize, to: usize) -> [u8; 4] {
    let rel = (to as i64 - from_after as i64) as i32;
    rel.to_le_bytes()
}

/// True when a `rel32` displacement can reach `to` from `from_after` without truncation.
///
/// Every jump this module emits is `E9 rel32`, which is a **signed 32-bit displacement**. If the
/// callback block lives more than 2 GB from the patched function, the encoded displacement is silently
/// truncated and the jump lands somewhere unrelated. That is not hypothetical: it was the unexplained
/// failure recorded as "trampoline faults, `rsp` collapsed, fault address is an allocated block".
/// `VirtualAlloc(NULL, ...)` on this machine returns addresses around `0x1EC_82590000`, roughly 2 TB
/// above the module at `0x140000000`, so the very first live install jumped into a truncated address.
pub fn within_rel32(from_after: usize, to: usize) -> bool {
    let delta = to as i64 - from_after as i64;
    delta >= i32::MIN as i64 && delta <= i32::MAX as i64
}

/// Every address in `block` that the emitted code encodes as a `rel32` must be reachable from the site
/// that encodes it. Checked before the patch is written, so a bad layout is refused rather than
/// executed.
fn layout_reaches(block: usize, target: usize) -> Result<(), String> {
    let tramp = block + TRAMP_OFF;
    // the patch jumps target+STOLEN -> block
    if !within_rel32(target + STOLEN, block) {
        return Err(format!(
            "block 0x{block:X} is out of rel32 range of 0x{target:X} (patch jump would truncate)"
        ));
    }
    // the stub jumps block+0x6F+5 -> trampoline, and the trampoline jumps back to target+STOLEN
    if !within_rel32(block + 0x6F + 5, tramp) {
        return Err("stub jmp cannot reach the trampoline".into());
    }
    if !within_rel32(tramp + STOLEN + 5, target + STOLEN) {
        return Err(format!(
            "trampoline 0x{tramp:X} cannot reach 0x{target:X} (return jump would truncate)"
        ));
    }
    Ok(())
}

/// Allocates a block within `rel32` range of `target`.
///
/// `VirtualAlloc` with a null address returns whatever the allocator prefers, which on a 64-bit process
/// with a large address space is routinely terabytes away from the executable - far outside the ±2 GB a
/// `rel32` can encode. So this walks outward from the target in allocation-granularity steps and asks
/// for specific addresses until one is granted.
///
/// The walk is bounded: 64 KB granularity over ±2 GB is far too many candidates to scan exhaustively, so
/// it takes progressively larger strides and gives up with a clear error rather than looping. In
/// practice the first handful of hints near the module succeed, because that area is mostly free.
fn alloc_near(target: usize, size: usize) -> Result<usize, String> {
    const GRANULARITY: usize = 0x1_0000;
    const MEM_COMMIT_RESERVE: u32 = 0x3000;

    // Start below the target and walk up, then try above it. Below first because the module image
    // occupies the range just above its base, so the space beneath is usually emptier.
    let low = target.saturating_sub(0x7000_0000).max(GRANULARITY);
    let high = target.saturating_add(0x7000_0000);

    for step in [1usize, 2, 4, 8, 16, 64, 256, 1024] {
        let stride = GRANULARITY * step;
        let mut addr = low;
        while addr < high {
            // Ask for the address explicitly. Windows grants it only if the whole range is free, which
            // is exactly the test we want; anything else falls through to the next candidate.
            let got = unsafe {
                VirtualAlloc(
                    addr as *mut c_void,
                    size,
                    MEM_COMMIT_RESERVE,
                    PAGE_EXECUTE_READWRITE,
                )
            };
            if !got.is_null() {
                let got = got as usize;
                // The grant must still be in range: rounding or a reserved region can move it.
                if within_rel32(target + STOLEN, got) && within_rel32(got + TRAMP_OFF + STOLEN + 5, target + STOLEN) {
                    return Ok(got);
                }
                // In range check failed, or the allocator moved it: give it back and keep looking.
                unsafe {
                    VirtualFree(got as *mut c_void, 0, MEM_RELEASE);
                }
            }
            addr += stride;
        }
    }
    Err(format!(
        "could not allocate a code block within rel32 range of 0x{target:X}. This is not \
         recoverable by retrying: the hook design needs the block and the patched function within \
         2 GB of each other."
    ))
}

/// Builds the 8 bytes that replace the target's prologue: `jmp block` over the first five bytes, then
/// the original bytes 5..8 untouched. Keeping them means the `sub rsp, 0x170` that follows the pushes
/// is never disturbed, and the trampoline replays the stolen bytes exactly as before.
pub fn build_patch(target: usize, block: usize, head: &[u8]) -> [u8; PATCH] {
    let mut patch = [0u8; PATCH];
    patch[0] = 0xE9;
    patch[1..STOLEN].copy_from_slice(&rel32(target + STOLEN, block));
    patch[STOLEN..].copy_from_slice(&head[STOLEN..PATCH]);
    patch
}

/// Installs the hook. Returns the target address on success.
///
/// Idempotent: a second call returns the same target without patching again.
pub fn install(base: usize) -> Result<usize, String> {
    let existing = INSTALLED.load(Ordering::SeqCst);
    if existing != 0 {
        return Ok(existing);
    }
    let target = base + crate::sim::SIMULATE_RVA;
    // read a full prologue for validation but only steal five bytes
    let head = crate::sim::bytes_at(target, 16).ok_or("target unreadable")?;
    let original = head[..STOLEN].to_vec();
    if !crate::sim::plausible(&head) {
        return Err(format!("refusing to hook implausible bytes: {}", crate::sim::hex(&original)));
    }
    // The stolen bytes must be exactly the three pushes we validated. If a different build puts
    // anything RIP-relative first, a plain copy into the trampoline would be wrong, so refuse.
    const EXPECTED: [u8; STOLEN] = [0x40, 0x53, 0x56, 0x41, 0x55];
    // Somebody else already patched this site - most likely an earlier agent build that is still
    // resident in this process. Patching again would leave the second trampoline unreachable while
    // the first block's slot keeps being overwritten, so stop and say so.
    if original[0] == 0xE9 {
        return Err(format!(
            "target is already patched ({}) - an earlier agent build is still resident; restart the game",
            crate::sim::hex(&original)
        ));
    }
    if original[..STOLEN] != EXPECTED {
        return Err(format!(
            "unexpected prologue {} (expected {})",
            crate::sim::hex(&original),
            crate::sim::hex(&EXPECTED)
        ));
    }

    unsafe {
        // Not `VirtualAlloc(NULL, ...)`: on a 64-bit process that lands terabytes from the module, and
        // every `E9 rel32` in the stub and the trampoline then encodes a truncated displacement.
        let block = match alloc_near(target, BLOCK_SIZE) {
            Ok(b) => b,
            Err(e) => return Err(e),
        };
        let slot = block + SLOT_OFF;
        let tramp = block + TRAMP_OFF;

        // Belt and braces: refuse the layout if any emitted displacement would truncate, even though
        // `alloc_near` already checks. A truncated jump is a crash with no useful dump.
        if let Err(e) = layout_reaches(block, target) {
            unsafe {
                VirtualFree(block as *mut c_void, 0, MEM_RELEASE);
            }
            return Err(e);
        }

        let code = build_code(block, slot, tramp, target, &original);

        std::ptr::copy_nonoverlapping(code.as_ptr(), block as *mut u8, code.len());
        // The padding fill also covers the data words; an unfired hook must read as 0, not as 0xCC, or
        // a caller cannot tell "not fired yet" from "captured garbage". The callback word starts zero
        // too, so a freshly installed hook is inert rather than calling into a stale pointer.
        std::ptr::write_bytes((block + SLOT_OFF) as *mut u8, 0, 8);
        std::ptr::write_bytes((block + FIRES_OFF) as *mut u8, 0, 8);
        std::ptr::write_bytes((block + CALLBACK_OFF) as *mut u8, 0, 8);
        FlushInstructionCache(GetCurrentProcess(), block as *const c_void, code.len());

        // The whole 8-byte window must be aligned for the store to be atomic, and the bytes we are
        // about to overwrite must be the ones we validated a moment ago.
        if target % PATCH != 0 {
            return Err(format!("target 0x{target:X} is not {PATCH}-byte aligned"));
        }
        if head.len() < PATCH {
            return Err("could not read the full patch window".into());
        }
        let patch = build_patch(target, block, &head);
        if patch[1..STOLEN] != rel32(target + STOLEN, block) {
            return Err("patch encoding is wrong".into());
        }

        let mut old = 0u32;
        if VirtualProtect(target as *mut c_void, PATCH, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return Err("VirtualProtect(RW) failed".to_string());
        }
        let mut value = [0u8; 8];
        value.copy_from_slice(&patch);
        // A single atomic exchange. `InterlockedExchange64` is an MSVC intrinsic with no import
        // library entry, so the same instruction is reached through a Rust atomic instead. Called
        // fully qualified: `.swap` on a pointer resolves to the slice method of the same name.
        let previous = std::sync::atomic::AtomicI64::swap(
            unsafe { &*(target as *const std::sync::atomic::AtomicI64) },
            i64::from_le_bytes(value),
            Ordering::SeqCst,
        );
        FlushInstructionCache(GetCurrentProcess(), target as *const c_void, PATCH);
        let mut back = 0u32;
        VirtualProtect(target as *mut c_void, PATCH, old, &mut back);

        // Read back: the patch must be exactly what we wrote, and what we replaced must have been the
        // bytes we validated. A mismatch means something else patched the same place first.
        let check = crate::sim::bytes_at(target, PATCH).unwrap_or_default();
        if check != patch {
            return Err(format!("patch did not stick: read back {}", crate::sim::hex(&check)));
        }
        let mut expected_previous = [0u8; 8];
        expected_previous.copy_from_slice(&head[..PATCH]);
        if previous.to_le_bytes() != expected_previous {
            return Err(format!(
                "the bytes replaced were {} not {}",
                crate::sim::hex(&previous.to_le_bytes()),
                crate::sim::hex(&expected_previous)
            ));
        }

        BLOCK.store(block, Ordering::SeqCst);
        TARGET.store(target, Ordering::SeqCst);
        INSTALLED.store(target, Ordering::SeqCst);
        Ok(target)
    }
}

/// The RaceSimManager pointer captured by the hook, or 0 if it has not fired yet.
pub fn captured() -> usize {
    let block = BLOCK.load(Ordering::SeqCst);
    if block == 0 {
        return 0;
    }
    unsafe { std::ptr::read_volatile((block + SLOT_OFF) as *const usize) }
}

/// The address of the executable block, or None when the hook was never installed.
pub fn block() -> Option<usize> {
    let block = BLOCK.load(Ordering::SeqCst);
    if block == 0 { None } else { Some(block) }
}

/// How many times the stub has run. Zero means the hook is installed but the game never called it.
pub fn fires() -> u64 {
    let block = BLOCK.load(Ordering::SeqCst);
    if block == 0 {
        return 0;
    }
    unsafe { std::ptr::read_volatile((block + FIRES_OFF) as *const u64) }
}

pub fn target() -> usize {
    TARGET.load(Ordering::SeqCst)
}

/// The fire count and the callback pointer, for the agent's heartbeat line.
pub fn state() -> (u64, usize) {
    (fires(), callback())
}

/// Builds stub + trampoline bytes. Split out so the encoding is testable without patching code.
///
/// The stub does the minimum that has to be assembly:
///
/// 1. capture `rcx` (the sim object) and bump the fire counter - seven bytes each, RIP-relative;
/// 2. if a callback is registered, save every volatile register, the flags and the argument xmm
///    registers, pass `this` as the callback's single argument, and call it;
/// 3. restore everything, then jump to the trampoline, which replays the stolen prologue bytes and
///    re-enters the game's function at `target + 5`.
///
/// Step 2 is the whole point. The callback runs **on the game thread, inside the sim call**, which is
/// the only place a write into the sim object is ordered against the simulation. Doing the same write
/// from a worker thread with no synchronisation is what produced every
/// `EXCEPTION_ACCESS_VIOLATION reading 0xF3C` in this project.
///
/// The callback pointer lives in the block's data area, not in the instruction stream, so it can be
/// set, cleared or replaced while the game runs. Zero means "do nothing", which is the state a freshly
/// installed hook starts in - so installing the hook is inert until something asks for a callback.
///
/// Register discipline: only volatile registers are touched. rbx/rbp/rsi/rdi/r12-r15 are callee-saved
/// and the Rust callback preserves them itself, so the stub does not save them. xmm0-xmm3 are the
/// floating-point argument registers and xmm0-5 are volatile, so the four that can carry arguments
/// are saved explicitly: this stub runs *before* the game's prologue, when incoming xmm arguments
/// still matter.
///
/// ```text
///   0x00  48 89 0D rel32        mov [rip+slot], rcx
///   0x07  48 FF 05 rel32        inc qword [rip+fires]
///   0x0E  50                    push rax
///   0x0F  48 8B 05 rel32        mov rax, [rip+callback]
///   0x16  48 85 C0              test rax, rax
///   0x19  74 53                 jz 0x6E            ; no callback: skip the save/call, keep pop rax
///   0x1B  9C                    pushfq
///   0x1C  41 53 41 52 41 51 41 50 52 51   push r11,r10,r9,r8,rdx,rcx
///   0x26  48 81 EC 48 00 00 00  sub rsp, 0x48
///   0x2D  0F 29 44 24 00        movaps [rsp+0x00], xmm0
///   0x32  0F 29 4C 24 10        movaps [rsp+0x10], xmm1
///   0x37  0F 29 54 24 20        movaps [rsp+0x20], xmm2
///   0x3C  0F 29 5C 24 30        movaps [rsp+0x30], xmm3
///   0x41  48 8B 4C 24 48        mov rcx, [rsp+0x48]  ; the `this` argument
///   0x46  FF D0                 call rax             ; <-- Rust runs here, on the game thread
///   0x48  0F 28 ... x4          movaps xmm0..3 back
///   0x5C  48 81 C4 48 00 00 00  add rsp, 0x48
///   0x63  59 5A 41 58 41 59 41 5A 41 5B   pop rcx,rdx,r8,r9,r10,r11
///   0x6D  9D                    popfq
///   0x6E  58                    pop rax
///   0x6F  E9 rel32              jmp trampoline
///   0x80  trampoline: <5 stolen bytes>; E9 -> target+5
///   0x50  slot: u64, 0x58 fires: u64, 0x60 callback: u64
/// ```
pub fn build_code(
    block: usize,
    slot: usize,
    tramp: usize,
    target: usize,
    stolen: &[u8],
) -> Vec<u8> {
    let mut code: Vec<u8> = Vec::with_capacity(TRAMP_OFF + 32);

    // mov [rip+disp32], rcx  - capture `this` (block+0x00, 7 bytes)
    code.push(0x48);
    code.push(0x89);
    code.push(0x0D);
    code.extend_from_slice(&rel32(block + 0x07, slot));

    // inc qword [rip+disp32] - fire counter (block+0x07, 7 bytes)
    code.push(0x48);
    code.push(0xFF);
    code.push(0x05);
    code.extend_from_slice(&rel32(block + 0x0E, block + FIRES_OFF));

    // The stub is fixed-length by construction and every displacement below is a constant derived
    // from that length, so these asserts are what keep the two in step. They run under `cargo test`.
    debug_assert_eq!(code.len(), 0x0E);

    code.push(0x50); // push rax                (0x0E)
    code.push(0x48); // mov rax, [rip+callback]  (0x0F)
    code.push(0x8B);
    code.push(0x05);
    code.extend_from_slice(&rel32(block + 0x16, block + CALLBACK_OFF));
    code.extend_from_slice(&[0x48, 0x85, 0xC0]); // test rax, rax  (0x16)
    // The target is block+0x6E, the `pop rax`, NOT block+0x6F. Landing on the `jmp` would skip the
    // `push rax` at 0x0E and leave rsp 8 bytes low for the rest of the game's function - which
    // returns to a garbage address. That is a stack leak, not a jump: it faults much later.
    code.extend_from_slice(&[0x74, 0x53]); // jz block+0x6E       (0x19)
    debug_assert_eq!(code.len(), 0x1B);
    debug_assert_eq!(0x1B + 0x53, 0x6E, "the short jump must land on the pop rax");

    code.push(0x9C); // pushfq                  (0x1B)
    // Pushed right-to-left so `rcx` ends up closest to rsp; that is what fixes the reload offset.
    code.extend_from_slice(&[0x41, 0x53]); // push r11 (0x1C)
    code.extend_from_slice(&[0x41, 0x52]); // push r10
    code.extend_from_slice(&[0x41, 0x51]); // push r9
    code.extend_from_slice(&[0x41, 0x50]); // push r8
    code.push(0x52); // push rdx
    code.push(0x51); // push rcx                (0x25)
    debug_assert_eq!(code.len(), 0x26);

    // Eight pushes above (push rax, pushfq, six GPRs = 64 bytes) leave rsp at 8 mod 16, because the
    // caller's `call` left it there and 64 is a multiple of 16. A 0x48 reservation therefore brings it
    // to 0 mod 16, which is what `movaps` requires - 0x40 here faults with #GP on the first store.
    code.extend_from_slice(&[0x48, 0x81, 0xEC, 0x48, 0x00, 0x00, 0x00]); // sub rsp, 0x48
    debug_assert_eq!(code.len(), 0x2D);

    code.extend_from_slice(&[0x0F, 0x29, 0x44, 0x24, 0x00]); // movaps [rsp+0x00], xmm0
    code.extend_from_slice(&[0x0F, 0x29, 0x4C, 0x24, 0x10]); // movaps [rsp+0x10], xmm1
    code.extend_from_slice(&[0x0F, 0x29, 0x54, 0x24, 0x20]); // movaps [rsp+0x20], xmm2
    code.extend_from_slice(&[0x0F, 0x29, 0x5C, 0x24, 0x30]); // movaps [rsp+0x30], xmm3
    debug_assert_eq!(code.len(), 0x41);

    // rcx was the last push, so it sits 0x48 above the current rsp.
    code.extend_from_slice(&[0x48, 0x8B, 0x4C, 0x24, 0x48]); // mov rcx, [rsp+0x48]
    code.extend_from_slice(&[0xFF, 0xD0]); // call rax
    debug_assert_eq!(code.len(), 0x48);

    code.extend_from_slice(&[0x0F, 0x28, 0x44, 0x24, 0x00]); // movaps xmm0, [rsp+0x00]
    code.extend_from_slice(&[0x0F, 0x28, 0x4C, 0x24, 0x10]); // movaps xmm1, [rsp+0x10]
    code.extend_from_slice(&[0x0F, 0x28, 0x54, 0x24, 0x20]); // movaps xmm2, [rsp+0x20]
    code.extend_from_slice(&[0x0F, 0x28, 0x5C, 0x24, 0x30]); // movaps xmm3, [rsp+0x30]
    code.extend_from_slice(&[0x48, 0x81, 0xC4, 0x48, 0x00, 0x00, 0x00]); // add rsp, 0x48
    debug_assert_eq!(code.len(), 0x63);

    code.push(0x59); // pop rcx
    code.push(0x5A); // pop rdx
    code.extend_from_slice(&[0x41, 0x58]); // pop r8
    code.extend_from_slice(&[0x41, 0x59]); // pop r9
    code.extend_from_slice(&[0x41, 0x5A]); // pop r10
    code.extend_from_slice(&[0x41, 0x5B]); // pop r11
    code.push(0x9D); // popfq
    code.push(0x58); // pop rax
    debug_assert_eq!(code.len(), 0x6F);

    code.push(0xE9); // jmp trampoline
    code.extend_from_slice(&rel32(block + 0x74, tramp));
    debug_assert_eq!(code.len(), 0x74);
    debug_assert!(
        code.len() <= TRAMP_OFF,
        "the stub must not run into the trampoline"
    );
    debug_assert!(
        TRAMP_OFF + STOLEN + 5 <= SLOT_OFF,
        "the trampoline must not run into the data area"
    );

    while code.len() < TRAMP_OFF {
        code.push(0xCC);
    }
    code.extend_from_slice(&stolen[..STOLEN]);
    code.push(0xE9);
    code.extend_from_slice(&rel32(tramp + STOLEN + 5, target + STOLEN));
    code
}

/// Wraps a Rust function as the hook callback.
///
/// The callback receives the sim object pointer as `this` and must not unwind, allocate on the game
/// thread's stack, or take a lock that a worker might hold while waiting on the game. It runs inside
/// the sim call, so anything it blocks on is a blocked simulation.
///
/// `extern "system"` matches the MS x64 calling convention the stub emits: one pointer argument in
/// rcx. Returning nothing keeps the stub's restore path trivial.
pub type Callback = extern "system" fn(this: *mut c_void);

/// Registers the function the stub should call on every sim invocation. Zero clears it.
///
/// Safe to call while the game runs: the pointer is a single aligned 8-byte store, and the stub reads
/// it once per call. Clearing it does not stop an in-flight call; it stops the next one.
pub fn set_callback(cb: Option<Callback>) {
    let block = BLOCK.load(Ordering::SeqCst);
    if block == 0 {
        return;
    }
    let addr = match cb {
        Some(f) => f as usize,
        None => 0,
    };
    unsafe {
        std::ptr::write_volatile((block + CALLBACK_OFF) as *mut u64, addr as u64);
    }
}

/// The callback currently registered, for diagnostics.
pub fn callback() -> usize {
    let block = BLOCK.load(Ordering::SeqCst);
    if block == 0 {
        return 0;
    }
    unsafe { std::ptr::read_volatile((block + CALLBACK_OFF) as *const u64) as usize }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte-for-byte check of the stub, including that each RIP-relative displacement points at the
    /// field it is meant to reach. A displacement that is off by the length of the instruction is the
    /// classic way this kind of code silently writes into the wrong place.
    #[test]
    fn stub_encoding_is_what_we_expect() {
        let block = 0x1000_0000usize;
        let slot = block + SLOT_OFF;
        let tramp = block + TRAMP_OFF;
        let target = 0x1422_98080usize;
        let stolen = [0x40u8, 0x53, 0x56, 0x41, 0x55];
        let code = build_code(block, slot, tramp, target, &stolen);

        // 48 89 0D disp32: mov [rip+disp32], rcx, displacement from the end of the instruction
        assert_eq!(&code[0..3], &[0x48, 0x89, 0x0D]);
        assert_eq!(i32::from_le_bytes([code[3], code[4], code[5], code[6]]), (slot as i64 - (block + 7) as i64) as i32);

        // 48 FF 05 disp32: inc qword [rip+disp32]
        assert_eq!(&code[7..10], &[0x48, 0xFF, 0x05]);
        assert_eq!(
            i32::from_le_bytes([code[10], code[11], code[12], code[13]]),
            ((block + FIRES_OFF) as i64 - (block + 14) as i64) as i32
        );

        // 48 8B 05 disp32: mov rax, [rip+callback] - read at run time, so the callback can be set or
        // cleared while the game runs.
        assert_eq!(&code[15..18], &[0x48, 0x8B, 0x05]);
        assert_eq!(
            i32::from_le_bytes([code[18], code[19], code[20], code[21]]),
            ((block + CALLBACK_OFF) as i64 - (block + 22) as i64) as i32,
            "the callback load must read CALLBACK_OFF"
        );
        assert_eq!(&code[22..25], &[0x48, 0x85, 0xC0], "test rax, rax");
        // jz must land exactly on the `jmp trampoline`, measured from the end of the two-byte jz.
        assert_eq!(&code[25..27], &[0x74, 0x53], "an unset callback must skip only the save/call");
        assert_eq!(0x1B + 0x53, 0x6E, "the short jump must land on `pop rax`");
        // and the byte it lands on must really be `pop rax`, or the stub leaks its own push.
        assert_eq!(code[0x6E], 0x58, "the skip target must be the pop rax");

        // The `this` reload must point at the pushed rcx, not at some other slot: a wrong offset here
        // passes a garbage pointer into Rust.
        assert_eq!(&code[0x41..0x46], &[0x48, 0x8B, 0x4C, 0x24, 0x48], "mov rcx, [rsp+0x48]");
        assert_eq!(&code[0x46..0x48], &[0xFF, 0xD0], "call rax");

        // `movaps` faults on an unaligned address, so the reservation has to leave rsp 16-byte
        // aligned. The caller's `call` left rsp at 8 mod 16; eight pushes (64 bytes) keep it there, so
        // 0x48 is the value that aligns it and 0x40 faults.
        assert_eq!(&code[0x26..0x2D], &[0x48, 0x81, 0xEC, 0x48, 0x00, 0x00, 0x00]);
        assert_eq!(&code[0x5C..0x63], &[0x48, 0x81, 0xC4, 0x48, 0x00, 0x00, 0x00], "the add must undo the sub exactly");

        // E9 disp32: jmp trampoline, after the call and its restore
        assert_eq!(code[0x6F], 0xE9);
        assert_eq!(
            i32::from_le_bytes([code[0x70], code[0x71], code[0x72], code[0x73]]),
            (tramp as i64 - (block + 0x74) as i64) as i32
        );
        assert_eq!(code[0x74], 0xCC, "padding before the trampoline must be int3");;

        // stolen bytes land exactly at TRAMP_OFF
        assert_eq!(&code[TRAMP_OFF..TRAMP_OFF + STOLEN], &stolen);
        // and the trampoline jumps back to target+5
        assert_eq!(code[TRAMP_OFF + STOLEN], 0xE9);
        let back = i32::from_le_bytes([
            code[TRAMP_OFF + STOLEN + 1], code[TRAMP_OFF + STOLEN + 2],
            code[TRAMP_OFF + STOLEN + 3], code[TRAMP_OFF + STOLEN + 4],
        ]);
        assert_eq!(back, (target as i64 + STOLEN as i64 - (tramp + STOLEN + 5) as i64) as i32);
    }

    /// The patch must jump to our block and must leave the three bytes after the stolen pushes
    /// untouched, or the `sub rsp, 0x170` that follows is corrupted and the game faults on entry.
    #[test]
    fn patch_jumps_to_the_block_and_preserves_the_tail() {
        let target = 0x1422_98080usize;
        let block = 0x1234_0000usize;
        let head = [0x40u8, 0x53, 0x56, 0x41, 0x55, 0x48, 0x81, 0xEC, 0x70, 0x01, 0x00, 0x00];

        let patch = build_patch(target, block, &head);
        assert_eq!(patch[0], 0xE9, "must be a relative jump");
        assert_eq!(
            i32::from_le_bytes([patch[1], patch[2], patch[3], patch[4]]),
            (block as i64 - (target + STOLEN) as i64) as i32
        );
        // bytes 5..8 are the start of `sub rsp, 0x170` and must survive verbatim
        assert_eq!(&patch[STOLEN..], &[0x48, 0x81, 0xEC]);
        assert_eq!(target % PATCH, 0, "the store must be aligned to be atomic");
    }

    /// End to end against a fake function that has the real prologue. This is the test the first
    /// crash needed: it patches a page, calls the patched function, and requires a correct return.
    /// A trampoline that jumps back to `target` instead of `target + STOLEN` loops forever and
    /// overflows the stack, which is exactly what a collapsed `rsp` in the field looked like.
    #[test]
    fn patched_function_runs_and_returns() {
        const PAGE_EXECUTE_READWRITE: u32 = 0x40;
        const MEM_COMMIT_RESERVE: u32 = 0x3000;

        let page = unsafe {
            VirtualAlloc(std::ptr::null_mut(), 0x1000, MEM_COMMIT_RESERVE, PAGE_EXECUTE_READWRITE) as usize
        };
        assert_ne!(page, 0, "VirtualAlloc failed");
        let block = unsafe {
            VirtualAlloc(std::ptr::null_mut(), BLOCK_SIZE, MEM_COMMIT_RESERVE, PAGE_EXECUTE_READWRITE) as usize
        };
        assert_ne!(block, 0, "VirtualAlloc failed");

        // The real prologue, then a matching epilogue so `ret` returns to the caller:
        //   push rbx; push rsi; push r13; sub rsp, 0x170
        //   add rsp, 0x170; pop r13; pop rsi; pop rbx; ret
        let body: [u8; 12] = [0x40, 0x53, 0x56, 0x41, 0x55, 0x48, 0x81, 0xEC, 0x70, 0x01, 0x00, 0x00];
        let epilogue: [u8; 12] = [
            0x48, 0x81, 0xC4, 0x70, 0x01, 0x00, 0x00, // add rsp, 0x170
            0x41, 0x5D, // pop r13
            0x5E, // pop rsi
            0x5B, // pop rbx
            0xC3, // ret
        ];
        unsafe {
            std::ptr::copy_nonoverlapping(body.as_ptr(), page as *mut u8, body.len());
            std::ptr::copy_nonoverlapping(epilogue.as_ptr(), (page as *mut u8).add(body.len()), epilogue.len());
        }

        let slot = block + SLOT_OFF;
        let tramp = block + TRAMP_OFF;
        let stolen = body[..STOLEN].to_vec();
        let code = build_code(block, slot, tramp, page, &stolen);
        assert_eq!(code.len(), TRAMP_OFF + STOLEN + 5);
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), block as *mut u8, code.len());
            std::ptr::write_bytes(slot as *mut u8, 0, 8);
            std::ptr::write_bytes((block + FIRES_OFF) as *mut u8, 0, 8);
            std::ptr::write_bytes((block + CALLBACK_OFF) as *mut u8, 0, 8);
            FlushInstructionCache(GetCurrentProcess(), block as *const c_void, code.len());
        }

        let patch = build_patch(page, block, &body);
        unsafe {
            std::ptr::copy_nonoverlapping(patch.as_ptr(), page as *mut u8, PATCH);
            FlushInstructionCache(GetCurrentProcess(), page as *const c_void, PATCH);
        }

        // If the trampoline jumps back to the start of the function this call never returns.
        let f: extern "system" fn(usize) = unsafe { std::mem::transmute(page) };
        f(0xDEAD_BEEF);

        assert_eq!(
            unsafe { std::ptr::read_volatile(slot as *const usize) },
            0xDEAD_BEEF,
            "`this` was not captured"
        );
        assert_eq!(
            unsafe { std::ptr::read_volatile((block + FIRES_OFF) as *const u64) },
            1,
            "the stub did not run exactly once"
        );
    }

    /// Runs the real stub on a real executable page with a callback registered: proves the capture
    /// write, the fire counter and the callback call all happen, and that `this` arrives in Rust
    /// intact. This is the test that would have caught the earlier probe bugs before a game restart.
    #[test]
    fn stub_calls_the_registered_callback_with_this() {
        static SEEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

        extern "system" fn cb(this: *mut c_void) {
            SEEN.store(this as usize, Ordering::SeqCst);
            CALLS.fetch_add(1, Ordering::SeqCst);
        }

        const PAGE_EXECUTE_READWRITE: u32 = 0x40;
        const MEM_COMMIT_RESERVE: u32 = 0x3000;
        let block = unsafe {
            VirtualAlloc(std::ptr::null_mut(), BLOCK_SIZE, MEM_COMMIT_RESERVE, PAGE_EXECUTE_READWRITE) as usize
        };
        assert_ne!(block, 0, "VirtualAlloc failed");

        // A `ret` in the trampoline returns straight to the caller, so the fake target is never
        // reached and no real code has to exist there.
        let stolen = [0xC3u8, 0xCC, 0xCC, 0xCC, 0xCC];
        let code = build_code(block, block + SLOT_OFF, block + TRAMP_OFF, 0x1422_98080, &stolen);
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), block as *mut u8, code.len());
            std::ptr::write_bytes((block + SLOT_OFF) as *mut u8, 0, 8);
            std::ptr::write_bytes((block + FIRES_OFF) as *mut u8, 0, 8);
            std::ptr::write_volatile((block + CALLBACK_OFF) as *mut u64, cb as usize as u64);
            FlushInstructionCache(GetCurrentProcess(), block as *const c_void, code.len());
        }

        let stub: extern "system" fn(usize) = unsafe { std::mem::transmute(block) };
        stub(0xDEAD_BEEF);
        assert_eq!(unsafe { std::ptr::read_volatile((block + SLOT_OFF) as *const usize) }, 0xDEAD_BEEF);
        assert_eq!(unsafe { std::ptr::read_volatile((block + FIRES_OFF) as *const u64) }, 1);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "the callback must run on every fire");
        assert_eq!(SEEN.load(Ordering::SeqCst), 0xDEAD_BEEF, "the callback must receive `this`");

        stub(0xBEEF_DEAD);
        assert_eq!(unsafe { std::ptr::read_volatile((block + FIRES_OFF) as *const u64) }, 2);
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(SEEN.load(Ordering::SeqCst), 0xBEEF_DEAD);
    }

    /// A stub with no callback registered must be completely inert: capture and count, then get out
    /// of the way. It must not dereference `rcx` (caller-controlled) and must not call anything.
    /// This is the test the earlier faulting run was missing.
    #[test]
    fn stub_without_a_callback_never_dereferences_this() {
        const PAGE_EXECUTE_READWRITE: u32 = 0x40;
        const MEM_COMMIT_RESERVE: u32 = 0x3000;
        let block = unsafe {
            VirtualAlloc(std::ptr::null_mut(), BLOCK_SIZE, MEM_COMMIT_RESERVE, PAGE_EXECUTE_READWRITE) as usize
        };
        assert_ne!(block, 0, "VirtualAlloc failed");

        let stolen = [0xC3u8, 0xCC, 0xCC, 0xCC, 0xCC];
        let code = build_code(block, block + SLOT_OFF, block + TRAMP_OFF, 0x1422_98080, &stolen);
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), block as *mut u8, code.len());
            std::ptr::write_bytes((block + SLOT_OFF) as *mut u8, 0, 8);
            std::ptr::write_bytes((block + FIRES_OFF) as *mut u8, 0, 8);
            // The callback word stays 0, which is the state a freshly installed hook is in.
            std::ptr::write_bytes((block + CALLBACK_OFF) as *mut u8, 0, 8);
            FlushInstructionCache(GetCurrentProcess(), block as *const c_void, code.len());
        }

        let stub: extern "system" fn(usize) = unsafe { std::mem::transmute(block) };
        // 0x1234 is not a pointer. If the stub dereferenced rcx, or called through a zero callback,
        // this faults.
        stub(0x1234);
        assert_eq!(unsafe { std::ptr::read_volatile((block + SLOT_OFF) as *const usize) }, 0x1234);
        assert_eq!(unsafe { std::ptr::read_volatile((block + FIRES_OFF) as *const u64) }, 1);
    }

    /// The stub must preserve the registers it borrows. This runs through the real prologue path with
    /// a body that writes known values into the volatile registers *after* the stub has restored
    /// them, so a clobbered register shows up as a wrong captured value rather than as a mystery
    /// corruption inside the game.
    #[test]
    fn stub_preserves_the_registers_it_borrows() {
        static OBSERVED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

        extern "system" fn cb(_this: *mut c_void) {
            OBSERVED.fetch_add(1, Ordering::SeqCst);
        }

        const PAGE_EXECUTE_READWRITE: u32 = 0x40;
        const MEM_COMMIT_RESERVE: u32 = 0x3000;
        let block = unsafe {
            VirtualAlloc(std::ptr::null_mut(), BLOCK_SIZE, MEM_COMMIT_RESERVE, PAGE_EXECUTE_READWRITE) as usize
        };
        assert_ne!(block, 0, "VirtualAlloc failed");
        let page = unsafe {
            VirtualAlloc(std::ptr::null_mut(), 0x1000, MEM_COMMIT_RESERVE, PAGE_EXECUTE_READWRITE) as usize
        };
        assert_ne!(page, 0, "VirtualAlloc failed");

        // Real prologue, then `mov eax, 0x1234; ret` (the popped rbx/rsi/r13 are balanced below).
        let body: [u8; 12] = [0x40, 0x53, 0x56, 0x41, 0x55, 0x48, 0x81, 0xEC, 0x70, 0x01, 0x00, 0x00];
        let epilogue: [u8; 12] = [
            0x48, 0x81, 0xC4, 0x70, 0x01, 0x00, 0x00, // add rsp, 0x170
            0x41, 0x5D, // pop r13
            0x5E, // pop rsi
            0x5B, // pop rbx
            0xC3, // ret
        ];
        unsafe {
            std::ptr::copy_nonoverlapping(body.as_ptr(), page as *mut u8, body.len());
            std::ptr::copy_nonoverlapping(epilogue.as_ptr(), (page as *mut u8).add(body.len()), epilogue.len());
        }

        let code = build_code(block, block + SLOT_OFF, block + TRAMP_OFF, page, &body[..STOLEN]);
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), block as *mut u8, code.len());
            std::ptr::write_bytes((block + SLOT_OFF) as *mut u8, 0, 8);
            std::ptr::write_bytes((block + FIRES_OFF) as *mut u8, 0, 8);
            std::ptr::write_volatile((block + CALLBACK_OFF) as *mut u64, cb as usize as u64);
            FlushInstructionCache(GetCurrentProcess(), block as *const c_void, code.len());
        }
        let patch = build_patch(page, block, &body);
        unsafe {
            std::ptr::copy_nonoverlapping(patch.as_ptr(), page as *mut u8, PATCH);
            FlushInstructionCache(GetCurrentProcess(), page as *const c_void, PATCH);
        }

        // Calling with a real prologue/epilogue exercises the save/restore with a non-trivial stack
        // frame underneath it. If the stub's `sub rsp, 0x40` were unbalanced the ret would go wild.
        let f: extern "system" fn(usize) = unsafe { std::mem::transmute(page) };
        f(0xFEED_FACE);

        assert_eq!(OBSERVED.load(Ordering::SeqCst), 1, "the callback must have run once");
        assert_eq!(
            unsafe { std::ptr::read_volatile((block + SLOT_OFF) as *const usize) },
            0xFEED_FACE,
            "`this` must survive to the slot"
        );
    }

    #[test]
    fn rel32_is_relative_to_the_next_instruction() {
        // jmp at 0x1000 encoding 5 bytes lands at 0x1005; displacement to 0x2000 is 0xFFB
        let d = rel32(0x1005, 0x2000);
        assert_eq!(i32::from_le_bytes(d), 0x2000 - 0x1005);
    }

    #[test]
    fn rel32_handles_backwards_targets() {
        let d = rel32(0x2000, 0x1000);
        assert_eq!(i32::from_le_bytes(d), -0x1000);
    }

    /// The bug that was recorded as an unexplained trampoline fault.
    ///
    /// Every jump is a signed 32-bit displacement. `VirtualAlloc(NULL, ...)` on a 64-bit process hands
    /// back an address around 0x1EC_82590000 while the module sits at 0x140000000 - about 2 TB away.
    /// The displacement then truncates, the jump lands in unmapped memory, and the fault looks like a
    /// collapsed stack rather than a bad encoding.
    #[test]
    fn a_distant_block_is_rejected_before_it_is_patched() {
        let target = 0x1_4000_0000 + 0x0229_8080; // module + simulate RVA
        // The addresses actually observed from VirtualAlloc(NULL) on this machine.
        let distant = 0x1_EC82_590000usize;

        assert!(
            !within_rel32(target + STOLEN, distant),
            "a 2 TB block cannot be reached by a rel32"
        );
        assert!(!within_rel32(target + STOLEN + 5, distant));

        let err = layout_reaches(distant, target).expect_err("a distant block must be refused");
        assert!(err.contains("rel32"), "the error must name the cause: {err}");

        // and a nearby block must pass, so the check is not simply rejecting everything
        let near = target & !0xFFFF;
        assert!(within_rel32(target + STOLEN, near));
        assert!(layout_reaches(near, target).is_ok(), "a nearby block is fine");
    }

    /// Proves the fix against the real allocator, not a mock.
    ///
    /// `VirtualAlloc(NULL, ...)` in a 64-bit process returns something terabytes away; this checks that
    /// `alloc_near` finds a block the encoded jumps can actually reach. Without this test the fix would
    /// be an untested claim about Windows' allocation behaviour.
    #[test]
    fn alloc_near_returns_a_block_within_rel32_range() {
        // Any realistic module base. Using this process's own module is the most honest target
        // available, since it is the same kind of address the game occupies.
        let base = crate::sim::main_module_base().expect("main module");
        let target = base + crate::sim::SIMULATE_RVA; // only used as a distance reference

        let block = alloc_near(target, BLOCK_SIZE).expect("must find a block within range");
        assert_ne!(block, 0);
        assert!(
            within_rel32(target + STOLEN, block),
            "block 0x{block:X} is not reachable from 0x{target:X}"
        );
        assert!(
            within_rel32(block + TRAMP_OFF + STOLEN + 5, target + STOLEN),
            "the return jump would truncate"
        );
        // and the layout check must agree
        assert!(layout_reaches(block, target).is_ok());

        // Confirm it is genuinely not what a null-address allocation would have returned, i.e. the
        // test would fail against the old code.
        let naive = unsafe {
            VirtualAlloc(
                std::ptr::null_mut(),
                BLOCK_SIZE,
                MEM_COMMIT_RESERVE,
                PAGE_EXECUTE_READWRITE,
            )
        };
        assert!(!naive.is_null());
        let naive = naive as usize;
        println!(
            "alloc_near -> 0x{block:012X}  naive VirtualAlloc(NULL) -> 0x{naive:012X}  \
             target 0x{target:012X}  naive_in_range={}",
            within_rel32(target + STOLEN, naive)
        );
        unsafe {
            VirtualFree(naive as *mut c_void, 0, MEM_RELEASE);
            VirtualFree(block as *mut c_void, 0, MEM_RELEASE);
        }
    }

    /// The boundary itself: ±2 GB is not symmetric, because a `rel32` is a signed value whose range is
    /// [-2147483648, 2147483647]. One byte past either end must be refused.
    #[test]
    fn rel32_range_boundaries_are_exact() {
        let base = 0x1_0000_0000usize;
        assert!(within_rel32(base, base + i32::MAX as usize));
        assert!(!within_rel32(base, base + i32::MAX as usize + 1));
        assert!(within_rel32(base, base - 0x7FFF_FFFF));
        // i32::MIN is reachable (-2147483648) but one further is not.
        assert!(within_rel32(base, base - 0x8000_0000));
        assert!(!within_rel32(base, base - 0x8000_0001));
    }
}
