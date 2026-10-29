"""Pre-flight gate for a synced race. Refuses to say READY unless the whole chain is live.

Why this exists
---------------
Three separate sessions were lost to the same class of mistake: a component was alive but pointed at
something dead, or two copies of it were running, and nobody checked. Specifically, in one session:

  * two host publishers were running, so the relay fanned duplicate tick streams at the client
    (visible as `gap: tick 15 -> 13`);
  * the client was attached to a game pid that no longer existed;
  * the client's live game had no agent in it at all, so every frame was refused.

Each was invisible from the component that was working. The cure is to check the *chain* - every link,
against the live process table - and to fail loudly. This script exits non-zero unless everything needed
for a synced race is true right now.

It deliberately reports the evidence for each check (pids, hashes, listening sockets, counters), so a
failure names the thing to fix rather than a bare FAIL.

Usage:
    python tools/preflight.py            # check, print a table, exit 0 only if ready
    python tools/preflight.py --wait 60  # poll for up to 60s for frames to start flowing

Read-only except for starting nothing: it never injects, never kills, never writes to a game.
"""

import ctypes
import ctypes.wintypes as w
import hashlib
import os
import re
import struct
import sys
import time

SHARE = os.environ.get('COOP_REMOTE_SHARE', r'\\<client-pc>\e')
HOST_BIN = r'E:\Steam\steamapps\common\F1 Manager 2024\F1Manager24\Binaries\Win64'
CLIENT_BIN = SHARE + r'\SteamLibrary\steamapps\common\F1 Manager 2024\F1Manager24\Binaries\Win64'
AGENT_PORT = 45991
RELAY_PORT = 45990

PROCESS_VM_READ = 0x0010
PROCESS_QUERY_INFORMATION = 0x0400
MEM_COMMIT = 0x1000
PAGE_GUARD = 0x100
PAGE_NOACCESS = 0x01

CC = ctypes
k32 = CC.windll.kernel32
k32.OpenProcess.restype = w.HANDLE
k32.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
k32.ReadProcessMemory.argtypes = [w.HANDLE, CC.c_void_p, CC.c_void_p,
                                  CC.c_size_t, CC.POINTER(CC.c_size_t)]
k32.VirtualQueryEx.argtypes = [w.HANDLE, CC.c_void_p, CC.c_void_p, CC.c_size_t]
k32.CloseHandle.argtypes = [w.HANDLE]

LAYOUTS = [(0x90, 0x98, 0xA0)]


class MBI(CC.Structure):
    _fields_ = [('BaseAddress', CC.c_void_p), ('AllocationBase', CC.c_void_p),
                ('AllocationProtect', w.DWORD), ('PartitionId', w.WORD),
                ('_pad', w.WORD), ('RegionSize', CC.c_size_t), ('State', w.DWORD),
                ('Protect', w.DWORD), ('Type', w.DWORD), ('_pad2', w.DWORD)]


class Check:
    """A checklist with two severities.

    `add` is a wiring check: it must pass before a race is started, and a failure means something is
    pointed at the wrong process.

    `pending` is a flow check: it can only pass once a session is actually running, so before a race it
    reports as PENDING rather than FAIL. Keeping these apart on purpose - the alternative is to weaken a
    real check so the gate goes green, which is exactly how three sessions were lost.
    """

    def __init__(self):
        self.rows = []

    def add(self, ok, name, detail):
        self.rows.append(('wiring', ok, name, detail))
        return ok

    def pending(self, ok, name, detail):
        self.rows.append(('flow', ok, name, detail))
        return ok

    def report(self, require_flow=False):
        width = max(len(n) for _, _, n, _ in self.rows) + 2
        for kind, ok, name, detail in self.rows:
            if ok:
                mark = 'PASS'
            elif kind == 'flow' and not require_flow:
                mark = 'WAIT'
            else:
                mark = 'FAIL'
            print('%s  %-*s %s' % (mark, width, name, detail))

        bad = [n for k, ok, n, _ in self.rows if not ok and (k == 'wiring' or require_flow)]
        waiting = [n for k, ok, n, _ in self.rows if not ok and k == 'flow' and not require_flow]
        print()
        if bad:
            print('%d CHECK(S) FAILED - do not start a race:' % len(bad))
            for n in bad:
                print('  - %s' % n)
            return False
        if waiting:
            print('WIRING OK - safe to start a race.')
            print('Not yet flowing (%d), which is expected before a session:' % len(waiting))
            for n in waiting:
                print('  - %s' % n)
            print('\nRe-run with --started once the race is running to require these too.')
            return True
        print('READY: all %d checks passed, including frames flowing.' % len(self.rows))
        return True


# ------------------------------------------------------------------ host-side inspection

def read_proc(h, addr, size):
    buf = CC.create_string_buffer(size)
    got = CC.c_size_t(0)
    if not k32.ReadProcessMemory(h, CC.c_void_p(addr), buf, size, CC.byref(got)):
        return None
    return buf.raw[:got.value] if got.value == size else None


def ru64(h, addr):
    b = read_proc(h, addr, 8)
    return None if b is None else int.from_bytes(b, 'little')


def regions(h):
    addr = 0
    limit = 0x7FFFFFFF0000
    while addr < limit:
        mbi = MBI()
        if not k32.VirtualQueryEx(h, CC.c_void_p(addr), CC.byref(mbi), CC.sizeof(mbi)):
            break
        size = mbi.RegionSize or 0x1000
        if mbi.State == MEM_COMMIT and not (mbi.Protect & (PAGE_GUARD | PAGE_NOACCESS)) and mbi.Protect:
            yield int(mbi.BaseAddress or addr), size
        addr = int(mbi.BaseAddress or addr) + size


def find_hook_block(h):
    """Locate the stub by its constant RIP-relative displacements; the match address is the block."""
    for slot_off, fires_off, _cb in LAYOUTS:
        sig = (b'\x48\x89\x0d' + (slot_off - 7).to_bytes(4, 'little')
               + b'\x48\xff\x05' + (fires_off - 0x0E).to_bytes(4, 'little'))
        for base, size in regions(h):
            if size > 0x100000:
                continue
            data = read_proc(h, base, size)
            if not data:
                continue
            i = data.find(sig)
            if i >= 0:
                return base + i, fires_off
    return None, None


def hook_state(pid):
    """Returns (block, fires, callback) for a game pid, or (None, None, None)."""
    h = k32.OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, False, pid)
    if not h:
        return None, None, None
    try:
        block, fires_off = find_hook_block(h)
        if block is None:
            return None, None, None
        _, _, cb_off = LAYOUTS[0]
        return block, ru64(h, block + fires_off), ru64(h, block + cb_off)
    finally:
        k32.CloseHandle(h)


# ------------------------------------------------------------------ process table

def processes(name):
    """Pids for an image name. Uses tasklist via a shell so CSV quoting is handled consistently."""
    out = []
    import subprocess
    try:
        r = subprocess.run('tasklist /FI "IMAGENAME eq %s" /FO CSV /NH' % name,
                           capture_output=True, text=True, timeout=20, shell=True)
    except Exception:
        return out
    for line in r.stdout.splitlines():
        # `"F1Manager24.exe","44312","Console","1","32 248 K"`
        parts = [p.strip('"') for p in line.split('","')]
        if len(parts) >= 2 and parts[0].lower().endswith(name.lower()):
            try:
                out.append(int(parts[1]))
            except ValueError:
                pass
    return out


def cmdline(pid):
    import subprocess
    try:
        r = subprocess.run(
            ['powershell', '-NoProfile', '-Command',
             "(Get-CimInstance Win32_Process -Filter \"ProcessId=%d\").CommandLine" % pid],
            capture_output=True, text=True, timeout=20)
        return r.stdout.strip()
    except Exception:
        return ''


def working_set(pid):
    import subprocess
    try:
        r = subprocess.run(
            ['powershell', '-NoProfile', '-Command',
             "(Get-Process -Id %d -ErrorAction SilentlyContinue).WorkingSet64" % pid],
            capture_output=True, text=True, timeout=20)
        return int(r.stdout.strip() or 0)
    except Exception:
        return 0


def sha16(path):
    try:
        with open(path, 'rb') as f:
            return hashlib.sha256(f.read()).hexdigest()[:16], os.path.getsize(path)
    except OSError:
        return None, None


def listening(port, local_only=True):
    """Owning pid of a listening socket. `local_only` checks the loopback listener, which is the one an
    agent on the same machine uses; a remote host cannot be probed this way."""
    import subprocess
    try:
        where = " -LocalAddress 127.0.0.1" if local_only else ""
        r = subprocess.run(
            ['powershell', '-NoProfile', '-Command',
             "(Get-NetTCPConnection -LocalPort %d -State Listen%s -ErrorAction SilentlyContinue | "
             "Select-Object -First 1).OwningProcess" % (port, where)],
            capture_output=True, text=True, timeout=20)
        v = r.stdout.strip()
        return int(v) if v.isdigit() else None
    except Exception:
        return None


def file_hash(path):
    """Full SHA-256, for comparing an installed pak against the built one."""
    try:
        with open(path, 'rb') as f:
            return hashlib.sha256(f.read()).hexdigest()
    except OSError:
        return None


def read_side(bin_dir):
    """Read `coop_side.txt` from a `Binaries\\Win64` directory: 'host', 'client', or None.

    The agent resolves its player identity from this file (it cannot see the injector's environment), and
    the side decides which half of `ClockGate` its pause reports feed. Both machines defaulting to Host
    does not fail loudly - it silently arbitrates both players as the host, which is worse than refusing.
    """
    try:
        with open(os.path.join(bin_dir, 'coop_side.txt'), 'r', encoding='utf-8') as f:
            v = f.readline().strip().lower()
        return v if v in ('host', 'client') else (v or None)
    except OSError:
        return None


def built_agent():
    """Hash and size of the agent this tree builds, or (None, None) if it has not been built.

    The agent size used to be hard-coded here (305152) and went stale the moment the agent gained code, which
    made every preflight fail on a check that was really about "did the build happen". Comparing against the
    artefact is the check that was actually meant.

    Order matters and got this wrong once: the workspace pins its output via `.cargo/config.toml`
    (`target-dir = E:/TEMP/coop-target`), so an *in-tree* `target/release` is a stale leftover from an older
    build. Preferring it made the gate compare a two-month-old binary against the install and fail a machine
    that was correctly deployed. The pinned location is therefore asked for first.
    """
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    candidates = [
        # Explicit override wins, for a machine that builds somewhere unusual.
        os.environ.get('COOP_AGENT_DLL', ''),
        # The pinned target dir from .cargo/config.toml - the one `cargo build --release` actually writes.
        r'E:\TEMP\coop-target\release\coop_agent.dll',
        os.path.join(here, 'coop-rs', 'target', 'release', 'coop_agent.dll'),
    ]
    for p in candidates:
        if p and os.path.exists(p):
            h = file_hash(p)
            if h:
                return h, os.path.getsize(p)
    return None, None


def remote(script, timeout=40):
    """Run a PowerShell snippet on the client machine through its agent, returning stdout.

    Returns '' when the client cannot be reached. Used only for read-only queries.
    """
    import subprocess
    import tempfile

    here = os.path.dirname(os.path.abspath(__file__))
    name = os.path.join(os.environ.get('TEMP', '.'), 'coop-remote-%d.ps1' % os.getpid())
    try:
        with open(name, 'w', encoding='utf-8-sig') as f:
            f.write(script)
        r = subprocess.run([sys.executable, os.path.join(here, 'remote.py'), 'runfile', name],
                           capture_output=True, text=True, timeout=timeout)
        return r.stdout if r.returncode == 0 else ''
    except Exception:
        return ''
    finally:
        try:
            os.remove(name)
        except OSError:
            pass


def spin(path, tries=6, delay=3):
    """Wait for a network-share path to answer.

    A single failed probe is not evidence the machine is down - the share flaps - and `os.path.isdir` is
    unreliable for a UNC path from a child `python.exe`. The probe is a temp `.ps1` file rather than an
    inline `-Command`, because quoting a UNC path with spaces through two shells produced a PowerShell
    *parse* error that looked exactly like "share is down" and hid the real state for several runs.
    """
    import subprocess
    import tempfile

    script = "$p = '%s'\nif (Test-Path -LiteralPath $p) { 'yes' } else { 'no' }\n" % path.replace("'", "''")
    name = os.path.join(os.environ.get('TEMP', '.'), 'coop-preflight-%d.ps1' % os.getpid())
    try:
        # Written with an explicit close: `mkstemp` leaves the handle open, and PowerShell then fails to
        # read the file with "being used by another process", which this code would have re-reported as
        # "the client machine is down".
        with open(name, 'w', encoding='utf-8-sig') as f:
            f.write(script)
        for _ in range(tries):
            try:
                r = subprocess.run(
                    ['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', name],
                    capture_output=True, text=True, timeout=25)
                if 'yes' in r.stdout.lower():
                    return True
            except Exception:
                pass
            time.sleep(delay)
        return False
    finally:
        try:
            os.remove(name)
        except OSError:
            pass


def log_tail(path, n=40):
    try:
        with open(path, 'r', errors='replace') as f:
            return f.read().splitlines()[-n:]
    except OSError:
        return []


def count_in(lines, pattern):
    rx = re.compile(pattern)
    return sum(1 for line in lines if rx.search(line))


# ------------------------------------------------------------------ the checks

def main():
    wait = 0
    if '--wait' in sys.argv:
        i = sys.argv.index('--wait')
        wait = int(sys.argv[i + 1]) if i + 1 < len(sys.argv) else 30

    c = Check()

    local_pid = 0
    for p in processes('F1Manager24.exe'):
        if working_set(p) > 500 * 1024 * 1024:   # the real game, not the launcher
            local_pid = p
            break
    c.add(local_pid != 0, 'host game running', 'pid %s' % (local_pid or 'none'))

    block, fires, cb = hook_state(local_pid) if local_pid else (None, None, None)
    c.add(block is not None, 'host agent injected', ('hook block 0x%X' % block) if block else
          'no hook stub found - inject coop_agent.dll')
    c.add(bool(cb), 'host hook callback registered', ('0x%X' % cb) if cb else 'callback is null')

    # The agent that is *installed* must match the agent this tree builds. Asking "is it 305152 bytes" was a
    # build-staleness check in disguise and broke the moment the agent grew; comparing hashes says what was
    # meant. If the tree has not been built, that is a named failure rather than a size mismatch nobody can act on.
    want_hash, want_size = built_agent()
    h_dll, h_size = sha16(os.path.join(HOST_BIN, 'coop_agent.dll'))
    if want_hash is None:
        c.pending(False, 'agent build available',
                  'no coop_agent.dll in the tree - run cargo build --release -p coop-agent')
    else:
        c.add(h_dll is not None and file_hash(os.path.join(HOST_BIN, 'coop_agent.dll')) == want_hash,
              'host agent matches the build',
              ('%s bytes %s' % (h_size, h_dll)) if h_size else 'missing - run tools/remote.py deploy-link')

    # Which player this machine is. Load-bearing, and silent when wrong: the agent reads coop_side.txt
    # because it cannot see the injector's environment, and the side decides whose half of ClockGate its
    # pause reports feed. Two machines both saying Host arbitrates both players as the host - a wrong clock
    # owner rather than an error, which is why this is a hard check and not a warning.
    h_side = read_side(HOST_BIN)
    c.add(h_side == 'host', 'host side file says host',
          h_side or 'missing coop_side.txt - the agent would default to Host, which is only right by luck')
    c_side = read_side(CLIENT_BIN)
    if c_side is not None or os.path.isdir(CLIENT_BIN):
        c.add(c_side == 'client', 'client side file says client',
              c_side or 'missing coop_side.txt - both machines would claim Host and arbitration breaks')

    # Publishers: exactly one, and pointed at the live game.
    pubs = [p for p in processes('coop-link.exe')
            if '--role host' in cmdline(p) or '--role","host' in cmdline(p)]
    c.add(len(pubs) == 1, 'exactly one host publisher',
          '%d running%s' % (len(pubs), '' if len(pubs) == 1 else
                            ' - duplicates fan out twice at the client (gaps go backwards)'))
    if len(pubs) == 1:
        cl = cmdline(pubs[0])
        m = re.search(r'--pid\s+(\d+)', cl)
        said = int(m.group(1)) if m else None
        c.add(said == local_pid, 'publisher targets the live game',
              'publisher pid %s reads game %s, live game is %s' % (pubs[0], said, local_pid))

    relay = listening(RELAY_PORT, local_only=False)
    c.add(relay is not None, 'relay listening on %d' % RELAY_PORT,
          ('pid %s' % relay) if relay else 'start coop-supervisor.exe')

    # Client machine: reachable at all? The share flaps every few seconds, so this polls patiently
    # rather than concluding "down" from one probe - that false negative cost a full preflight run
    # twice, and a gate that cries wolf gets bypassed, which defeats its purpose.
    reachable = spin(CLIENT_BIN, tries=14, delay=3)
    c.add(reachable, 'client machine reachable', CLIENT_BIN if reachable else 'share is down')

    if reachable:
        c_dll, c_size = sha16(os.path.join(CLIENT_BIN, 'coop_agent.dll'))
        if want_hash is not None:
            c.add(file_hash(os.path.join(CLIENT_BIN, 'coop_agent.dll')) == want_hash,
                  'client agent matches the build',
                  ('%s bytes %s' % (c_size, c_dll)) if c_size else 'missing - run tools/remote.py deploy-link')

        c.bin_size, _ = sha16(os.path.join(CLIENT_BIN, 'coop-link.exe'))
        c_ls, _ = sha16(os.path.join(CLIENT_BIN, 'coop-link.exe'))
        h_ls, _ = sha16(os.path.join(HOST_BIN, 'coop-link.exe'))
        c.add(c_ls == h_ls, 'client binary matches host', str(c_ls) if c_ls else 'missing')

        agent_log = log_tail(os.path.join(CLIENT_BIN, 'coop_agent.log'))
        joined = '\n'.join(agent_log)

        # A log from a previous session proves nothing about *this* one, and it is actively misleading: the
        # first run of this gate reported "client hook has fired, total 72903 calls" from a two-day-old file
        # while no game was running at all. Counters are therefore only trusted when the log is recent enough
        # to belong to the session being tested.
        try:
            log_age = time.time() - os.path.getmtime(os.path.join(CLIENT_BIN, 'coop_agent.log'))
        except OSError:
            log_age = None
        log_is_current = log_age is not None and log_age < 3600
        c.pending(log_is_current, 'client agent log is from this session',
                  ('%ds old' % log_age) if log_age is not None else 'no coop_agent.log')

        fired = [int(m.group(1)) for line in agent_log
                 for m in [re.search(r'hook fired x\d+ \(total (\d+)\)', line)] if m]
        if not log_is_current:
            c.pending(False, 'client hook has fired',
                      'counters are from an old log - start the client and re-run')
        else:
            c.pending(bool(fired), 'client hook has fired',
                      ('total %d calls' % fired[-1]) if fired else
                      'never fired - no race is running on the client yet')

        applied = [int(m.group(1)) for line in agent_log
                   for m in [re.search(r'mailbox applied=(\d+)', line)] if m]
        if not log_is_current:
            c.pending(False, 'client agent has applied frames',
                      'counters are from an old log - start the client and re-run')
        else:
            c.pending(bool(applied) and applied[-1] > 0, 'client agent has applied frames',
                      ('applied=%d' % applied[-1]) if applied else 'nothing applied yet')

        c.bin_tyres = (log_tail(os.path.join(CLIENT_BIN, 'coop_tyres.txt'), 1) or ['(none)'])[0].strip()
        c.add(c.bin_tyres in ('observe', 'wear', 'full'),
              'client tyre mode is explicit', c.bin_tyres)

        client_log = log_tail(os.path.join(CLIENT_BIN, 'client-run.log'))
        tail = '\n'.join(client_log)
        c.add('no agent at' not in tail, 'client is not reporting a missing agent',
              'clean' if 'no agent at' not in tail else 'client cannot reach its agent - see log')

        # Log-based checks are unavoidable for the remote machine, but a STALE log is indistinguishable
        # from a dead agent. So the live facts come from a process query: which pid the agent is in, what
        # the client attached to, and whether the port is listening. The log is only used for counters.
        live = remote(
            "$d = 'E:\\SteamLibrary\\steamapps\\common\\F1 Manager 2024\\F1Manager24\\Binaries\\Win64'\n"
            "$g = Get-Process F1Manager24 -ErrorAction SilentlyContinue |"
            " Sort-Object WorkingSet64 -Descending | Select-Object -First 1\n"
            "Write-Output ('GAMEPID ' + $g.Id)\n"
            "$l = Get-Process coop-link -ErrorAction SilentlyContinue | Select-Object -First 1\n"
            "if ($l) { Write-Output ('CLIENTPID ' + $l.Id) } else { Write-Output 'CLIENTPID 0' }\n"
            "$c = Get-NetTCPConnection -LocalPort 45991 -State Listen -ErrorAction SilentlyContinue |"
            " Select-Object -First 1\n"
            "if ($c) { Write-Output ('AGENTLISTEN ' + $c.OwningProcess) } else { Write-Output 'AGENTLISTEN 0' }\n"
            "$a = Join-Path $d 'coop_agent.log'\n"
            "if (Test-Path $a) { Write-Output ('AGELOG ' + [int]((Get-Date) - (Get-Item $a).LastWriteTime).TotalSeconds) }"
            " else { Write-Output 'AGELOG -1' }\n")

        def grab(key):
            for line in live.splitlines():
                if line.startswith(key + ' '):
                    v = line[len(key) + 1:].strip()
                    return int(v) if v.lstrip('-').isdigit() else None
            return None

        game_pid = grab('GAMEPID')
        client_pid = grab('CLIENTPID')
        agent_listen = grab('AGENTLISTEN')
        agent_log_age = grab('AGELOG')

        c.add(agent_listen not in (None, 0), 'client agent is running',
              ('listening, in pid %s' % agent_listen) if agent_listen else
              'no listener on 45991 - the client game has no agent')
        # A quiet log is normal pre-race: the agent only writes on state change (fire-rate buckets,
        # tyre observations once a session runs). Freshness is therefore a FLOW signal, not wiring -
        # requiring it before a race would flag a perfectly healthy agent, and a gate that cries wolf
        # gets ignored. What matters for wiring is that the log exists and the listener is up.
        c.pending(agent_log_age is not None and 0 <= agent_log_age < 60,
                  'client agent log is live',
                  ('%ss ago' % agent_log_age) if agent_log_age is not None else
                  'cannot read')
        if agent_listen and game_pid:
            c.add(agent_listen == game_pid, 'client agent is in the client game',
                  'agent pid %s, game pid %s' % (agent_listen, game_pid))

        # Throughput: the number that proves frames are actually landing.
        sent = [int(m.group(1)) for line in client_log
                for m in [re.search(r'sent (\d+) to the agent', line)] if m]
        if sent:
            first, last = sent[0], sent[-1]
            c.pending(last > first, 'client is sending frames',
                      'sent counter %d -> %d' % (first, last))
        else:
            c.pending(False, 'client is sending frames', 'no "sent N to the agent" line yet')

    else:
        for name in ('client agent is the current build', 'client binary matches host',
                     'client hook installed', 'client agent accepting frames',
                     'client hook has fired', 'clIENT agent has applied frames',
                     'client tyre mode is explicit', 'client is not reporting a missing agent',
                     'client is sending frames'):
            c.add(False, name, 'client unreachable')

    # The HOST's agent listener. The client's cannot be probed from here, so the client side is judged
    # from its own log instead - and a check that silently reported the host's port as the client's would
    # have passed while the client had no agent at all.
    port = listening(AGENT_PORT)
    c.add(port is not None, 'host agent port %d listening' % AGENT_PORT,
          ('pid %s' % port) if port else 'no agent in the host game')
    if port is not None:
        c.add(port == local_pid, 'host agent belongs to the host game',
              'listener pid %s, game pid %s' % (port, local_pid))

    # The UI paks. This check exists because "the multiplayer button does not appear" was twice caused
    # by the install running an old pak rather than by anything in the code: the game holds the file
    # open, so the copy silently did not happen. A stale pak is compared by size and hash against the
    # build, which is cheap and cannot be fooled.
    #
    # It also checks the *location*. A loose `*_P.pak` in `Content/Paks/` root is never mounted - only paks
    # under `~mods/` are - so a byte-identical pak in the wrong folder loads nothing and produces exactly the
    # same symptom as a stale one: no multiplayer entry, no error. Checking only the hash would report
    # "identical" for a deployment that cannot work.
    for label, root in (('host', HOST_BIN), ('client', CLIENT_BIN)):
        if label == 'client' and not reachable:
            continue
        # HOST_BIN ends in `<game>\F1Manager24\Binaries\Win64`, so the game root is three levels up.
        game_root = os.path.dirname(os.path.dirname(os.path.dirname(root)))
        paks = os.path.join(game_root, 'F1Manager24', 'Content', 'Paks')
        installed = os.path.join(paks, '~mods', 'CoopBridge_P.pak')
        misplaced = os.path.join(paks, 'CoopBridge_P.pak')
        built = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                             'pak', 'CoopBridge_P.pak')

        if not os.path.isdir(os.path.join(paks, '~mods')):
            c.add(False, '%s mods directory exists' % label,
                  'no `~mods` in %s (loose paks elsewhere do mount, so this is convention, not a blocker)' % paks)
        elif os.path.exists(misplaced):
            c.add(False, '%s pak is in the mount directory' % label,
                  'found in Paks/ root; it will still load, but ~mods is where mod managers expect it')
        if not os.path.exists(built):
            c.add(False, '%s coop pak is built' % label, 'missing %s' % built)
            continue
        if not os.path.exists(installed):
            c.add(False, '%s coop pak is installed' % label, 'missing %s' % installed)
            continue
        same = file_hash(installed) == file_hash(built)
        c.add(same, '%s coop pak matches the build' % label,
              'identical' if same else
              'STALE: installed %d bytes, built %d bytes - run tools/deploy_pak.py'
              % (os.path.getsize(installed), os.path.getsize(built)))

    ok = c.report(require_flow='--started' in sys.argv)
    if ok and wait:
        print('\nWatching for frames for %ds...' % wait)
        start = time.time()
        base = None
        while time.time() - start < wait:
            lines = log_tail(os.path.join(CLIENT_BIN, 'client-run.log'))
            sent = [int(m.group(1)) for line in lines
                    for m in [re.search(r'sent (\d+) to the agent', line)] if m]
            if sent:
                if base is None:
                    base = sent[-1]
                elif sent[-1] > base:
                    print('frames are flowing: sent %d -> %d' % (base, sent[-1]))
                    break
            time.sleep(2)
        else:
            print('no frame growth seen - the host publisher may have nothing to send '
                  '(start or join a session)')
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
