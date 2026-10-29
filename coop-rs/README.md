# coop-rs

Out-of-process half of F1 Manager 24 co-op. The game keeps its own simulation; this owns the wire.

```
crates/coop-proto        wire format: fixed layout, length-prefixed, checksummed, versioned
crates/coop-supervisor   relay: two game instances connect over loopback, frames are validated and forwarded
```

## Run

```sh
cargo test --offline          # 10 protocol tests
cargo run --offline --bin coop-supervisor -- --selftest   # end-to-end relay check
cargo run --offline --bin coop-supervisor -- --port 45990
```

The selftest starts a relay, connects a Host and a Client in-process, sends a 22-car snapshot and
requires the client to receive it byte-identical:

```
[supervisor] 127.0.0.1:64819 says it is Host, owns 11 car(s)
[supervisor] 127.0.0.1:64820 says it is Client, owns 11 car(s)
[supervisor] selftest: client received snapshot tick=7 race_time=12.5 cars=22
[supervisor] selftest OK: frames_in=1 frames_out=1 bytes_in=2528 bad=0
```

## Wire format

```
 0  magic     4   b"F1CP"
 4  version   2   little endian, mismatched peers are refused
 6  len       4   payload length
10  checksum  4   FNV-1a 32 of the payload
14  payload       tag + body
```

Cost: 2528 bytes per 22-car snapshot ≈ 100 KB/s at 40 Hz. A non-problem.

Design choices, each one a bug the Lua transport hit:

| Choice | Bug it prevents |
|---|---|
| length prefix + checksum | torn reads. Lua needed a trailing `END` marker and a text format to detect them |
| fixed field layout, no self-description | schema drift between two independently hot-reloaded files |
| explicit version, refused if different | a v77 client silently misreading a v78 host |
| bounds-checked decode returning `Result` | a short frame panicking inside the process that owns the sockets |
| frame splitter that waits for a whole frame | acting on half a snapshot mid-write |
| `no_delay` on every socket | 40 ms Nagle stalls masquerading as "the client is behind" |

## Deliberately not implemented yet

- **Peer transport.** Loopback only. Real remote play needs UDP+sequencing or QUIC, plus NAT handling
  (Steam P2P is the obvious candidate).
- **Ownership arbitration.** The protocol carries `Hello { side, cars_owned }` so the supervisor can
  refuse a snapshot for a car the sender does not own, but the rule itself is not enforced yet. This
  is the contested-car bug from the Lua v73 demo.
- **Plausibility validation.** The supervisor can already see every car every frame; checking that a
  distance advanced by a sane amount is cheap and would catch injection errors on the wire rather
  than in the game.
- **The in-process agent.** Still Lua. It should shrink to "read these offsets, send this frame" and
  the breadcrumb/minidump work should land next, because a hang currently leaves no evidence.


## In-process agent (`crates/coop-agent`) and injector (`crates/coop-inject`)

The agent is a standalone DLL. It exists because writes must happen on the game thread, so that half
cannot move out of process. This increment deliberately does **nothing dangerous**: no hooks, no
memory access, no writes. It proves injection, thread startup and IPC only.

```sh
# 1. supervisor
coop-supervisor --port 45990

# 2. inject (the agent connects to 127.0.0.1:45990 by default)
coop-inject --pid <pid> --dll coop_agent.dll
coop-inject --name F1Manager24 --dll coop_agent.dll
```

Verified against a live race (pid 44316):

```
injected ...coop_agent_v2.dll into pid 44316 (module base 0xEE620000)
[agent] agent started in pid 44316, supervisor 127.0.0.1:45990
[agent] connected to supervisor
[agent] heartbeat started
[supervisor] agent in pid 44316, rsm=0x0
```

`DllMain` does exactly one thing - `CreateThread` and return. No allocation, no logging, no locks,
because the loader lock makes anything else a deadlock risk.

### Bugs found and fixed on the way

- The first agent called `TcpStream::connect` inside its loop, so it reconnected every second and the
  supervisor logged a connect/close pair each time. Now the socket is held and only `write` failing
  triggers a reconnect.
- The supervisor logged `unknown message tag 6` until the binary was rebuilt: it had been locked by
  the running process during the previous build, so the stale exe was decoding the new message. A
  release build can silently no-op when the target is in use - always check the build actually
  replaced the binary.
- `--discover` hung with no output at all for minutes. It was not the reader: that argument arm never
  advanced the parser index, so the loop rematched it forever. `cargo build` now compiles
  `coop-agent`, whose DLL the game has open, so a workspace build fails with LNK1104 while a session
  is live - build `-p coop-supervisor -p coop-mem -p coop-inject` instead.
- `CreateToolhelp32Snapshot` deadlocked inside the game: it takes the target's loader lock, and the
  target was itself inside `LoadLibrary` mapping our DLL. Replaced with a PEB read.
- The agent reported `rsm=0xCCCCCCCCCCCCCCCC`: the hook slot is filled with `0xCC` and the agent
  published it unvalidated. Any address is now read before it is trusted.

## Crash and hang evidence

The first attempt to install the sim hook crashed the game with **no Windows Error Reporting event
and no dump**, so the only fact left was "it crashed" and a second attempt would have produced the
same silence. `crates/coop-agent/src/crash.rs` fixes that before any hook is retried.

Three artefacts, all next to the game's exe:

| file | written by | what it gives you |
|---|---|---|
| `coop_agent.trace` | `WriteFile` on a handle opened at load | one line per stage; the last line is where it died |
| `coop_dump_<pid>_<tick>_<reason>.dmp` | `MiniDumpWriteDump` | every thread's stack and registers, the module list, system info |
| `coop_agent.log` | the agent | the human-readable narrative |

The trace is written with a raw `WriteFile` on a pre-opened handle: no allocation and no `PathBuf`,
so it still works when the heap is corrupt. A crash cannot lose lines already written, because they
are in the file rather than in our address space.

A **first-chance** vectored handler also logs access violations as they happen, with the faulting
`rip`, and marks whether that `rip` is inside our hook block or the patched function. That is how a
fault that the game silently swallows becomes visible.

### Capturing a freeze

A freeze raises no exception, so nothing fires on its own. Create an empty file named
`coop_dump_now` next to the game's exe and the agent writes a dump within a second, then removes the
marker. Creating a file works from a shell, a script or a shortcut, which is what makes it usable
while the game is hanging. The dump briefly suspends every thread, so it is never automatic.

Measured on the live game (pid 45836): two requested dumps, 553 KB each, 147 threads, 15 streams,
`MDMP` signature, game still responding afterwards.

### Four things that looked like they should work and did not

Each of these cost a round of debugging, so they are recorded rather than rediscovered:

1. **`MiniDumpWriteDump` from the exception dispatcher on the faulting thread** fails with
   `ERROR_NOACCESS` (998) and writes a zero-byte file. The same call from a second thread succeeds,
   so the unhandled filter now hands the work to a thread of its own and waits on an event, bounded
   at 15 s.
2. **An `EXCEPTION_POINTERS` argument** makes the call fail the same way, with `ClientPointers` TRUE
   or FALSE, from either thread. `dbghelp.dll` forwards `MiniDumpWriteDump` to `dbgcore.dll`, and the
   fault lands inside dbgcore. No exception record is passed; the code and the faulting address come
   from the trace instead, and every thread's registers are in the dump anyway.
3. **A `MINIDUMP_USER_STREAM_INFORMATION` comment stream** faults in exactly the same place. The
   reason is in the dump's file name instead.
4. **`MINIDUMP_WITH_DATA_SEGS`** adds the writable data section of every loaded module and took a
   dump from 553 KB to 90 MB. Thread stacks, thread info and the module list are what identify a
   hang, so it is off.

Two process-level lessons came out of the same work. `LoadLibraryW` is now called at install time,
never from a handler, and the resolved function pointer is cached. And a `const` holding an `Atomic`
inside a function is a fresh temporary at every use, so a "cache" written that way never caches
anything ? the compiler said so and it was a real bug in `locate_rsm`.

### Verifying this without crashing the game

`cargo test --offline -p coop-agent` includes a test that **really kills a child process**: it
re-executes the test binary with `COOP_CRASH_SELFTEST=1`, installs the handler in the child,
breadcrumbs, dereferences a bad pointer, then the parent asserts that the trace kept the last stage,
that the unhandled filter ran, and that the dump parses with the `MDMP` signature and a non-empty
thread and module list. A size threshold is not used: the dump type changed once already and a byte
count would have passed on a file that had lost its thread list.

`cargo run --offline -p coop-agent --example dumptest` writes a dump from a healthy process, which is
how the four failures above were narrowed down.

## The hook, second attempt

The first attempt crashed the game with no evidence. Retried with evidence capture in place, and the
patch itself was made safer first: the target sits at `0x142298080`, which is 8-byte aligned, and the
three stolen `push` instructions total exactly 5 bytes, so the patch is now a **single aligned 8-byte
atomic exchange** that preserves the 3 bytes of `sub rsp, 0x170` that follow. A thread in the prologue
runs either the original bytes or the new ones, never a mixture, so no threads are suspended. That
removed the riskiest part of the old installer, which enumerated threads with
`CreateToolhelp32Snapshot` ? the call that deadlocked against this game once already.

### What the retry showed

The hook installed, the game **died on the first sim call**, and this time there was a trace:

```
first-chance code=0xc0000005 at=0x1afd70000 rip=0x1afd70000 rsp=0xf7f3f8
```

Two facts came out of it. The fault address is an allocated block rather than the game module, and
`rsp=0xf7f3f8` is a collapsed stack, which is the signature of unbounded recursion in the trampoline.

That reading also exposed a hole in the tests: the stub test used `ret` as the stolen bytes, so it
**never executed the jump back into the function** and could not have caught a trampoline that loops.
There is now a test that patches a fake function carrying the real prologue and a matching epilogue,
calls it through the patch, and requires a correct return. It passes, so the encoding is right and
the cause is elsewhere.

### A patch outlives the process that made it, but not a rebuild

The retry then refused to install with:

```
hook NOT installed: unexpected prologue E9 7B 7F 39 50 (expected 40 53 56 41 55)
```

`E9` is *our own* jump ? the site was already patched by the build whose install crashed. The
trampoline block belonged to a DLL that is still mapped, while the game survived. Patching again would
have pointed the site at a second block and left the first one's capture slot being overwritten
forever, so the installer now reports "already patched; restart the game" instead of proceeding.

The practical rule: **a killed process does not undo its patch.** A process that survived a failed
hook attempt is not clean, and re-injecting into it tests nothing. Restart the game between attempts.

### Done to make the next attempt diagnosable

- the stub now carries a fire counter, so after a crash `fires > 0` proves our code ran and
  `fires == 0` proves the game never reached the hook;
- the first-chance handler records `rip`, and marks whether it is inside the hook block or the
  patched function;
- `coop_agent.trace` is opened with `FILE_APPEND_DATA`. Several agent builds stay resident in one
  process, and a plain `GENERIC_WRITE` handle writes from offset 0, so they were overwriting each
  other's lines ? which is what produced interleaved, truncated trace entries in the field.

### Known noise

Every build of the agent injected into a process stays resident until that process exits. Several
old ones are left in the current game, and they report `rsm=0x0` once a second forever. The
supervisor therefore treats a zero address as "still looking" and logs it once per process, and
reports a real address once per change, so the handshake is not buried. A game restart clears them.

## Locating the RaceSimManager without hard-coded addresses

The object moves every session, so an address pasted from `UE4SS.log` is only good for that run. It
is now found from scratch, from inside the process (agent) or outside it (supervisor, probe):

1. **Main module base.** `GUObjectArray` is at a fixed RVA, `0x7821450`, from the exe base. The
   base comes from the PEB (`NtQueryInformationProcess` -> `PEB.ImageBaseAddress`), *not* from a
   Toolhelp/`EnumProcessModules` snapshot: those take the target's loader lock and deadlock against
   a process that is itself inside `LoadLibrary` ? which is exactly what happens while our agent DLL
   is being mapped. A PEB read is pure `ReadProcessMemory` and cannot block the game.
2. **Array layout**, derived rather than assumed: probe `0x10/0x18/0x20` for the data pointer and
   `8/16/24` for the item stride, then score the candidates against readable object pointers. The
   live game resolves to `array_offset=0x10, item_size=24` (score 29/32). `0x180000 / 24 = 65536`
   items per chunk, which is UE's own chunking, so the layout cannot be a coincidence.
3. **The instance.** Exactly two objects carry the `RaceSimManager` class size `0xA132F0`: the class
   default object and the live instance. They are told apart by content, never by order:

   | object | car 0 distance | sim clock | verdict |
   |---|---|---|---|
   | `0xFE920000` | 0.0 | 0.0 | class default object |
   | `0x173530000` | 15136.5 | 302.0 | live instance |

   If no candidate looks live (menu, nothing loaded) discovery returns an error instead of picking
   one, because a wrong object is a 10 MB window into unrelated memory.

Measured: full discovery is a ~100 000-object scan and takes a few seconds; it runs only when the
cached address stops being readable, so a running session costs one 4-byte validation read per pass.

## The read/write split
```
┌───────────────┐   ReadProcessMemory   ┌──────────────────────────┐
│ F1Manager24   │◄──────────────────────│ Rust: coop-mem + probe    │
│  (own thread) │                       │  reads, protocol, session │
│               │                       └──────────────────────────┘
│               │   shared memory / TCP ┌──────────────────────────┐
│  C++ plugin   │◄──────────────────────│ commands to apply         │
│  (game thread)│                       └──────────────────────────┘
└───────────────┘
```

**Reads in Rust** (`crates/coop-mem`): `OpenProcess` + `ReadProcessMemory`, raw FFI, no crates.
Reading cannot stall the game thread and any failure is a Win32 code with a name. One read of the
object covers everything, so a 40 Hz snapshot costs a single syscall per tick rather than ~700.

**Writes in C++** (not written yet): writes must land on the game thread at a point the game treats
as safe, which is exactly why the Lua version froze the client. A native plugin also gives real stack
traces and SEH, so a bad write stops being an unexplained freeze.

### Read cost, measured by the tests

- car block only: `car_block_len()` = 9 907 856 bytes
- with race control: `object_len()` = 10 564 336 bytes, because `RF_STATE` sits at `0xA13289`

A reader that takes only the car block gets a correct snapshot with race control unavailable —
`rc()` returns `None` rather than zeroes, so a cheap read cannot be mistaken for "no flags".

## Try the read half

```sh
# against the live game, once the agent publishes its endpoint
cargo run --offline --bin coop-probe -- --endpoint ..\coop_rsm.txt --frames 40

# or point it at an address by hand
cargo run --offline --bin coop-probe -- --pid 12345 --rsm 0x196FA0000 --frames 40
```

The agent writes `RSM <hex addr> <hex size> <unix>`; an endpoint older than 5 s is refused, because a
stale address points into freed memory and would produce convincing garbage.

## Prerequisites for the C++ half

| Need | Status |
|---|---|
| MSVC | present (VS 2022 Community), but only via `vcvars` — not on PATH |
| cmake | installed: `local/tools/cmake-3.29.6-windows-x86_64` (3.x on purpose; 4.x removed compatibility with older projects) |
| UE4SS C++ mod SDK (matching 3.0.1 / git `d935b5b`) | **impossible to build from source — see below** |
| hot reload | unavailable for native plugins, so every write change means a game restart |

### Why the UE4SS C++ route is closed

`git@github.com:Re-UE4SS/UEPseudo.git` is a required submodule of `deps/first/Unreal`, and that
repository returns **404** — it is private to the UE4SS organisation. Checked on both `main` and
`v3.0.1`: identical `.gitmodules`, identical private submodule. The only public copies are
third-party mirrors, which are not an acceptable supply-chain source.

Consequence: a UE4SS C++ mod cannot be built against the installed `UE4SS.dll` (3.0.1 Beta #0,
SHA `d935b5b`), because the toolchain that would produce a matching build is not obtainable.

The installed UE4SS is a release binary and ships no headers. So "writes inside UE4SS" is out;
the viable native route is a **standalone DLL** that owns its own hook and injection, with no
UE4SS dependency and therefore no ABI coupling.

Note for later: the engine build is pinned, and FormationLap documents the sim entry points —
`simulate` at RVA `0x142298080` and the flags-ring append at `0x142298126` — which is where a
standalone hook would attach to get a per-tick game-thread callback.

## The network link (`crates/coop-link`)

The relay existed but nothing used it: the supervisor forwarded `Host` frames to every `Client`, and
no process ever sent or consumed a `Snapshot`. `coop-link` is the missing half on each side.

```
# host machine: relay, then a source of snapshots
coop-supervisor.exe --bind 0.0.0.0
coop-link.exe --role host --supervisor 127.0.0.1:45990 --pid <game pid> --hz 20

# client machine: the far end
coop-link.exe --role client --supervisor <host-ip>:45990
```

`--bind` defaults to `0.0.0.0` so a client on the LAN can reach the relay; it was loopback-only, which
made a two-machine test impossible. The injected agent reads its supervisor address from
`coop_supervisor.txt` next to the game's exe, because an injected DLL cannot see the injector's
environment and rebuilding to change an address constant is not a usable test loop.

`--role host --fake` publishes synthetic moving snapshots. That separates the two halves of the
problem: if `--fake` reaches the other machine, the transport, relay, framing and client all work and
anything still broken is in the game side.

Measured locally over the relay, fake source at 20 Hz: **2488-byte frames, 22 cars, 37-40 snapshots
per two seconds, worst arrival gap 75-89 ms**, and the client's `coop_shadow.txt` listed 22 cars with
correctly rising distances.

## Session transfer

The host can send its save to a client over the same link, so a co-op session needs no file copying at
any point.

```
coop-link --role host --supervisor 127.0.0.1:45990 --pid <pid> --send-save "<save>"
```

`SessionOffer` / `SessionChunk` / `SessionReady` (tags 7-9), 32 KB chunks, re-offered every 10 s so a
client that joins late still receives it. The relay needed no changes: it already re-frames and forwards
any valid message.

The receiver holds the payload in memory and writes only once the hash matches, because a truncated
`.sav` in the game's save folder would be offered by the game's own load menu. Measured:

| save | bytes | transfer | result |
|---|---|---|---|
| `save168.sav` | 2 936 598 | 42 ms | byte-identical, SHA-256 match |
| `autosave.sav` | 11 575 739 | 84 ms | byte-identical, SHA-256 match |

Sending does **not** require a live race sim. It used to: host startup aborted when discovery failed,
so the transfer never ran - which is backwards, since the whole point is to bring a client to the host's
state while the host sits in menus.

The payload is never parsed. The save is compressed by frontier's own codec (`Oodle`/`zstd` are static in
the exe; the file is 7.99-8.00 bits/byte from 64 KB on, with no SQLite, CREATE TABLE or gzip marker), so
carrying the game's own bytes is the only correct handling. The merge for team ownership is a separate
problem and is planned in `../SAVE-MERGE-PLAN.md`: it needs the **live DB in memory**, not this file.

## Next steps

1. **Agent publishes its endpoint** — a one-line change to the Lua mod: write `RSM <addr> <size> <unix>`
   once proven. That alone lets `coop-probe` read the live race and validates the whole split.
   *(Already validated by hand: 22/22 cars, race control, best 1.53 ms, mean 3.19 ms per full object.)*
2. **Breadcrumb + minidump** on the agent so a freeze names its last operation.
3. **Standalone native write DLL**, since the UE4SS C++ SDK is unobtainable.
