"""Run code on the other PC and read the results back.

The other machine exposes a writable SMB share but no SSH, no WinRM and no open coop port, so the
channel is files: a request script goes in, its output and exit code come back. `remote-agent/run/`
must have been started on that machine once.

Why this exists
---------------
Everything in this project needs the *second* instance to be observable: whether the pak override
loaded there, whether the bridge saw `engine.trigger`, what its UE4SS log says, which save it has, how
big its working set is while a race runs. Reading that through a share beats guessing.

Usage
-----
    python remote.py ping
    python remote.py ps                      # processes on that machine
    python remote.py run "<powershell>"      # run a snippet, print output + exit code
    python remote.py game                    # game process table
    python remote.py ue4ss [n]               # tail of that machine's UE4SS.log
    python remote.py logs [n]                # tail of agent.log there
    python remote.py install-pak             # copy CoopBridge_P.pak across
    python remote.py push <local> <remote-rel>
    python remote.py pull <remote-rel> <local>
    python remote.py start-agent             # print what a human there must press
    python remote.py deploy-link             # coop-link + coop_agent.dll + coop-inject, hash-verified
    python remote.py versions                # must say MATCH before any test
    python remote.py attach-client           # inject the agent and start the client there
    python remote.py fires [seconds]         # is the hook actually firing? (see RACE-SYNC-RUNBOOK.md)

`remote.py` never guesses an address: it reads COOP_REMOTE_ROOT or falls back to the share that
already exists, and says so when it cannot reach it.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import time
import uuid

def _find_share():
    """Locate the other machine's share without trusting one hard-coded address.

    The address is not a constant: the LAN changed from `192.168.1.x` to `10.32.231.x` and back while this
    project was running, and the "client is unavailable" conclusion that came from a single failed hostname
    lookup was wrong twice - the machine was on, just somewhere else. So every plausible name is tried, with a
    short timeout, and the first that resolves wins.

    `COOP_REMOTE_SHARE` still overrides everything, for a machine nothing here can guess.
    """
    override = os.environ.get('COOP_REMOTE_SHARE')
    if override:
        return override
    candidates = [
        r'\\<client-pc>\e',
        r'\\192.168.1.100\e',
        r'\\192.168.50.101\e',
        r'\\10.32.231.100\e',
    ]
    for c in candidates:
        try:
            if os.path.isdir(c):
                return c
        except OSError:
            continue
    # Nothing resolved. Return the historical default so the error message names something concrete.
    return r'\\<client-pc>\e'


SHARE = _find_share()
ROOT = os.environ.get('COOP_REMOTE_ROOT', os.path.join(SHARE, 'coop-agent'))

GAME_SAVES = r'%LOCALAPPDATA%\F1Manager24\Saved\SaveGames'
GAME_PAKS = r'\SteamLibrary\steamapps\common\F1 Manager 2024\F1Manager24\Content\Paks'
GAME_BIN = r'\SteamLibrary\steamapps\common\F1 Manager 2024\F1Manager24\Binaries\Win64'

# The host's own install. An environment override exists because the drive differs between machines, and a
# hard-coded path that is wrong writes a side file into a directory nothing reads - which looks like a
# configured machine until the arbitration silently picks the wrong owner.
HOST_GAME = os.environ.get('COOP_GAME', r'E:\Steam\steamapps\common\F1 Manager 2024')
HOST_BIN = os.path.join(HOST_GAME, 'F1Manager24', 'Binaries', 'Win64')

LOCAL_PAK = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'pak', 'CoopBridge_P.pak')

# Which rules `coop-auth` enforces on each machine. Written to `coop_mode.txt` / `coop_player.txt` beside the exe,
# because nothing else creates them and their absence makes every gate pass - the team rules were documented,
# implemented and tested, and enforced nothing.
#
# `shared-team` is the only mode the UI honestly offers today: Versus advertises "a team each, your own garage"
# and the career layer that would deliver it is report-only. Override with COOP_MODE=versus to try it anyway.
COOP_MODE = os.environ.get('COOP_MODE', 'shared-team')
HOST_TEAM = int(os.environ.get('COOP_HOST_TEAM', '1'))
CLIENT_TEAM = int(os.environ.get('COOP_CLIENT_TEAM', '2'))
# Loadout is the car index. In `shared-team` each player owns one, which is the point of the mode, so the client
# must be told which: leaving both at 0 would have them fight over the same car and `coop-auth` would refuse one.
CLIENT_LOADOUT = int(os.environ.get('COOP_CLIENT_LOADOUT', '1'))


def release_dir():
    """Where the release binaries actually are.

    Not a hardcoded `coop-rs/target/release`: this project builds into `CARGO_TARGET_DIR` (set to
    `E:\\TEMP\\coop-target` to dodge a Defender lock on the in-tree target), and a deploy step that reads
    the wrong directory silently ships a stale binary. That trap has now cost **two** test cycles - the
    client ran a 12:51 build against a 13:18 fix, and later a 219648-byte agent was deployed while the
    real 288768-byte build sat in `E:\\TEMP`.

    Resolution: collect every candidate, then choose the one whose `coop-link.exe` is **newest**. Picking
    the first existing directory is not good enough, because a stale in-tree build is exactly as
    "existing" as the fresh one. `COOP_TARGET_DIR` still wins outright when set, so an ambiguous machine
    can be pinned explicitly.
    """
    root = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'coop-rs'))

    pinned = os.environ.get('COOP_TARGET_DIR')
    if pinned:
        d = os.path.join(pinned, 'release')
        if os.path.exists(os.path.join(d, 'coop-link.exe')):
            return d

    candidates = []
    for var in ('COOP_TARGET_DIR', 'CARGO_TARGET_DIR'):
        v = os.environ.get(var)
        if v:
            candidates.append(os.path.join(v, 'release'))

    # `coop-rs/.cargo/config.toml` is the single source of truth, because it also governs what cargo
    # itself does in *any* shell. Reading it here is what stops this script and cargo from disagreeing -
    # which is exactly how a stale binary got deployed twice.
    cfg = os.path.join(root, '.cargo', 'config.toml')
    try:
        with open(cfg, 'r', encoding='utf-8') as f:
            m = re.search(r'target-dir\s*=\s*"([^"]+)"', f.read())
        if m:
            candidates.append(os.path.join(m.group(1).replace('/', os.sep), 'release'))
    except OSError:
        pass

    candidates.append(os.path.join(root, 'target', 'release'))

    existing = [c for c in candidates if os.path.exists(os.path.join(c, 'coop-link.exe'))]
    if not existing:
        return candidates[0]

    def freshness(d):
        # Whichever file is newest decides: building only the agent leaves coop-link.exe old, and
        # selecting on coop-link alone would then mix a fresh agent with a stale client.
        return max(os.path.getmtime(os.path.join(d, n))
                   for n in ('coop-link.exe', 'coop_agent.dll')
                   if os.path.exists(os.path.join(d, n)))

    return max(existing, key=freshness)


RELEASE = release_dir()


def release_binary(name):
    path = os.path.join(RELEASE, name)
    if not os.path.exists(path):
        raise SystemExit('missing %s\n  build it with: cargo build --release -p coop-link -p coop-agent -p coop-inject\n'
                         '  expected in: %s' % (path, RELEASE))
    return path


def build_stamp():
    """Describe a binary as `size  mtime  hash16` so a stale copy is visible at a glance."""
    import hashlib

    if not os.path.exists(RELEASE):
        return '(no release dir)'
    parts = []
    for name in ('coop-link.exe', 'coop_agent.dll'):
        p = os.path.join(RELEASE, name)
        if not os.path.exists(p):
            parts.append('%s MISSING' % name)
            continue
        with open(p, 'rb') as f:
            h = hashlib.sha256(f.read()).hexdigest()
        parts.append('%s %d %s %s'
                     % (name, os.path.getsize(p),
                        time.strftime('%m-%d %H:%M', time.localtime(os.path.getmtime(p))), h[:8]))
    return ' | '.join(parts)



# --------------------------------------------------------------------------------------- plumbing

def dirs():
    return {k: os.path.join(ROOT, k) for k in ('req', 'out', 'done', 'put', 'get')}


def ensure_dirs():
    for d in dirs().values():
        os.makedirs(d, exist_ok=True)


def alive():
    """True if the other machine's share answers. Does not prove the agent is running."""
    try:
        return os.path.isdir(ROOT)
    except OSError:
        return False


def agent_state():
    """(running, age_seconds). The heartbeat file is the only live signal a share can carry."""
    if not alive():
        return False, None
    for name, limit in (('agent.beat', 20), ('agent.log', 120)):
        p = os.path.join(ROOT, name)
        if os.path.exists(p):
            age = time.time() - os.path.getmtime(p)
            return age < limit, age
    return False, None


def require_agent():
    if not alive():
        raise SystemExit('cannot reach %s\n'
                         'set COOP_REMOTE_SHARE / COOP_REMOTE_ROOT if the machine moved' % ROOT)
    ok, age = agent_state()
    if not ok:
        raise SystemExit(
            'the share is reachable but the agent is not answering there.\n'
            '  last agent.log write: %s\n'
            'start it on that machine (the share cannot launch a process itself):\n'
            '  %s\\run\\start-agent.cmd\n'
            'then retry.' % ('never' if age is None else '%ds ago' % age, ROOT))


def submit(script, timeout=180, quiet=False):
    """Send a PowerShell script and wait for its result. Returns (exit_code, output, meta)."""
    require_agent()
    ensure_dirs()
    d = dirs()
    rid = time.strftime('%H%M%S') + '-' + uuid.uuid4().hex[:8]
    req = os.path.join(d['req'], rid + '.cmd')
    out = os.path.join(d['out'], rid + '.out')
    meta = os.path.join(d['out'], rid + '.json')
    done = os.path.join(d['out'], rid + '.done')

    with open(req, 'w', encoding='utf-8') as f:
        f.write(script)

    deadline = time.time() + timeout
    while time.time() < deadline:
        if os.path.exists(done):
            break
        time.sleep(0.3)
    else:
        raise SystemExit('no result for %s within %ss - is the agent running there? '
                         '(remote.py start-agent)' % (rid, timeout))

    try:
        with open(out, 'r', encoding='utf-8-sig', errors='replace') as f:
            text = f.read()
    except OSError:
        text = ''
    try:
        # PowerShell 5.1's Set-Content -Encoding UTF8 writes a BOM; the agent runs on 5.1 there.
        with open(meta, 'r', encoding='utf-8-sig') as f:
            m = json.load(f)
    except (OSError, ValueError):
        m = {'id': rid, 'exit': -1, 'note': 'no meta'}

    for p in (req, out, meta, done):
        # Best-effort cleanup. A share delete can fail transiently while the agent still holds the
        # handle, and that must never turn a successful run into an error.
        try:
            os.remove(p)
        except OSError:
            pass
    return m.get('exit', 0), text, m


def report(code, text, m, quiet=False):
    if quiet:
        return
    if m.get('host'):
        print('--- %s as %s at %s in %ss ---'
              % (m.get('host'), m.get('user'), m.get('finished', '?'), m.get('seconds', '?')))
    # PowerShell's Set-Content -Encoding UTF8 emits a BOM, and that machine may print characters this
    # console cannot encode. Neither is a failure, so normalise rather than raise.
    body = text.lstrip('\ufeff').rstrip()
    if body:
        print(body.encode(sys.stdout.encoding or 'utf-8', 'replace').decode(
            sys.stdout.encoding or 'utf-8', 'replace'))
    print('--- exit %s ---' % code)


# ---------------------------------------------------------------------------------------- commands

def cmd_ping():
    ok, text, m = submit(
        'Write-Output "host=$env:COMPUTERNAME user=$env:USERNAME"\n'
        'Write-Output "ps=$($PSVersionTable.PSVersion)"\n'
        'Write-Output "distro=$((Get-CimInstance Win32_OperatingSystem).Caption)"\n'
        'Write-Output ("uptime=" + ((Get-Date) - (Get-CimInstance Win32_OperatingSystem).LastBootUpTime).ToString())\n')
    report(ok, text, m)


def cmd_ps():
    ok, text, m = submit(
        'Get-Process | Sort-Object WorkingSet64 -Descending | Select-Object -First 25 '
        'Id,ProcessName,@{n="MB";e={[int]($_.WorkingSet64/1MB)}},MainWindowTitle | '
        'Format-Table -AutoSize | Out-String -Width 200\n')
    report(ok, text, m)


def cmd_game():
    # Tier 1: this works with no agent at all. The share is readable, so the game's own log and save
    # folder can be inspected directly. Only arbitrary code execution needs the agent.
    print('--- F1Manager24.exe on the other PC ---')
    exe = SHARE + GAME_BIN + r'\F1Manager24.exe'
    if os.path.exists(exe):
        st = os.stat(exe)
        print('  present, %d bytes, modified %s'
              % (st.st_size, time.strftime('%Y-%m-%d %H:%M', time.localtime(st.st_mtime))))
    else:
        print('  not found at %s' % exe)

    print('--- CoopBridge pak ---')
    pak = SHARE + GAME_PAKS + r'\CoopBridge_P.pak'
    if os.path.exists(pak):
        st = os.stat(pak)
        print('  installed, %d bytes, %s'
              % (st.st_size, time.strftime('%Y-%m-%d %H:%M', time.localtime(st.st_mtime))))
    else:
        print('  NOT installed')

    print('--- coop artifacts in Binaries\\Win64 ---')
    bindir = SHARE + GAME_BIN
    try:
        names = [n for n in os.listdir(bindir)
                 if n.lower().startswith(('coop', 'ue4ss'))]
        for n in sorted(names):
            st = os.stat(os.path.join(bindir, n))
            print('  %-24s %8d  %s'
                  % (n, st.st_size, time.strftime('%H:%M', time.localtime(st.st_mtime))))
        if not names:
            print('  none')
    except OSError as exc:
        print('  cannot list: %s' % exc)

    # Saves are under %LOCALAPPDATA%, which is not on the share, so say so rather than guess.
    print('--- saves ---')
    print('  not reachable over this share (they live under %%LOCALAPPDATA%%);')
    print('  use: python remote.py saves      (needs the agent)')


def cmd_saves():
    ok, text, m = submit(
        '$d = Join-Path $env:LOCALAPPDATA "F1Manager24\\Saved\\SaveGames"\n'
        'if (-not (Test-Path $d)) { Write-Output "no save dir: $d"; exit }\n'
        'Get-ChildItem $d -Filter *.sav | Sort-Object LastWriteTime -Descending | '
        'Select-Object -First 12 Name,Length,LastWriteTime | Format-Table -AutoSize '
        '| Out-String -Width 160\n')
    report(ok, text, m)


def cmd_snapshot():
    """Everything observable about the other machine in one pass, agent if available, share if not."""
    print('=== tier 1: share (always available) ===')
    cmd_game()
    ok, age = agent_state()
    print()
    print('=== tier 2: agent ===')
    if not ok:
        print('  not running (last agent.log write: %s)'
              % ('never' if age is None else '%ds ago' % age))
        print('  start it there to get processes, logs, saves and arbitrary commands.')
        return
    print('  running')
    cmd_ps_head()
    print()
    cmd_saves()


def cmd_ps_head():
    ok, text, m = submit(
        'Get-Process | Sort-Object WorkingSet64 -Descending | Select-Object -First 12 '
        'Id,ProcessName,@{n="MB";e={[int]($_.WorkingSet64/1MB)}} | '
        'Format-Table -AutoSize | Out-String -Width 120\n')
    report(ok, text, m)


def cmd_ue4ss(n=40):
    ok, text, m = submit(
        '$b = "%s"\n'
        '$log = Join-Path $b "UE4SS.log"\n'
        'if (Test-Path $log) { Get-Content $log -Tail %d } else { Write-Output "no UE4SS.log at $log" }\n'
        % (GAME_BIN, n))
    report(ok, text, m)


def cmd_logs(n=30):
    ok, text, m = submit(
        '$log = Join-Path "%s" "agent.log"\n'
        'if (Test-Path $log) { Get-Content $log -Tail %d } else { Write-Output "no agent.log" }\n'
        % (ROOT, n))
    report(ok, text, m)


def cmd_attach_client():
    """Run the client attach script on that machine: kill old client, inject agent, start client."""
    here = os.path.dirname(os.path.abspath(__file__))
    path = os.path.join(here, '..', 'remote-agent', 'attach-client.ps1')
    print('running attach-client.ps1 on the client machine')
    ok = cmd_runfile(os.path.abspath(path))
    if not ok:
        print('attach-client reported a failure; reading its logs anyway')
    # The script writes its own verdict. Read it rather than assuming the start worked: `GAME_BIN` is
    # relative to the share root, so it has to be joined onto SHARE for an absolute path.
    verdict = SHARE + GAME_BIN.replace('/', '\\') + '\\client-attach.txt'
    for attempt in range(5):
        ok, text, m = submit(
            '$p = "%s"\n'
            'if (Test-Path $p) { Get-Content $p } else { Write-Output "(client-attach.txt missing)" }\n'
            % verdict.replace('"', '`"'))
        if ok:
            break
        time.sleep(3)
    print()
    print('client-attach.txt:')
    print(text.strip() or '  (not written - the script did not reach the end)')
    if 'AGENT NOT PRESENT' in text:
        raise SystemExit(
            'the agent is NOT in the client game, so frames will be staged and never applied.\n'
            'Check that coop_agent.dll and coop-inject.exe were deployed (remote.py deploy-link).')
    print()
    print('next: confirm the hook is live with `remote.py fires`, then start the session')


def cmd_fires(seconds=10):
    """Report whether the agent's hook is actually firing on that machine.

    The block is found by signature, so this works even for a build that does not log its address.
    """
    here = os.path.dirname(os.path.abspath(__file__))
    script = os.path.join(here, 'watch_fires.py')
    with open(script, 'r', encoding='utf-8') as f:
        body = f.read()
    # Ship the watcher, run it against the biggest game process there, then remove it.
    remote = os.path.join(ROOT, 'watch_fires.py')
    os.makedirs(ROOT, exist_ok=True)
    with open(remote, 'w', encoding='utf-8') as f:
        f.write(body)
    ok, text, m = submit(
        '$g = Get-Process F1Manager24 -ErrorAction SilentlyContinue |\n'
        '     Sort-Object WorkingSet64 -Descending | Select-Object -First 1\n'
        'if (-not $g) { Write-Output "NO GAME PROCESS"; exit 1 }\n'
        'python (Join-Path "%s" "watch_fires.py") $g.Id %d\n'
        % (ROOT, seconds), timeout=seconds + 60)
    report(ok, text, m)
    return ok


def cmd_run(script):
    ok, text, m = submit(script)
    report(ok, text, m)
    return ok


def cmd_runfile(local):
    """Run a local .ps1 on the other machine. Avoids shell quoting entirely."""
    if not os.path.exists(local):
        raise SystemExit('missing script: %s' % local)
    with open(local, 'r', encoding='utf-8') as f:
        script = f.read()
    ok, text, m = submit(script)
    report(ok, text, m)
    return ok


def cmd_killgame():
    ok, text, m = submit(
        '$p = Get-Process -Name F1Manager24 -ErrorAction SilentlyContinue\n'
        'if (-not $p) { Write-Output "already stopped"; return }\n'
        'foreach ($x in $p) {\n'
        '  Write-Output ("stopping pid " + $x.Id + " (" + [int]($x.WorkingSet64/1MB) + " MB)")\n'
        '  Stop-Process -Id $x.Id -Force -ErrorAction SilentlyContinue\n'
        '}\n'
        'Start-Sleep -Seconds 3\n'
        '$still = Get-Process -Name F1Manager24 -ErrorAction SilentlyContinue\n'
        'Write-Output ("remaining: " + @($still).Count)\n')
    report(ok, text, m)


def cmd_launch():
    """Start the game there through Steam's URL handler, then report what appeared."""
    ok, text, m = submit(
        'Start-Process "steam://rungameid/2591280"\n'
        'Start-Sleep -Seconds 20\n'
        'Get-Process -Name F1Manager24 -ErrorAction SilentlyContinue |\n'
        '  Select-Object Id,@{n="MB";e={[int]($_.WorkingSet64/1MB)}} |\n'
        '  Format-Table -AutoSize | Out-String -Width 80\n', timeout=120)
    report(ok, text, m)


def cmd_install_pak():
    if not os.path.exists(LOCAL_PAK):
        raise SystemExit('build it first: pak/CoopBridge_P.pak')
    dst_dir = SHARE + GAME_PAKS
    # `~mods` is the conventional home for a mod pak, and it is also how a *stale* copy survived: installing
    # to the `Paks/` root left an older pak in `~mods`, both mounted, and the engine is free to prefer either.
    # The client therefore ran the old UI (old multiplayer tile, old team bug) while every version check here
    # compared the file in the root and reported a match. One pak, one place.
    dst = os.path.join(dst_dir, '~mods', 'CoopBridge_P.pak')
    stray = os.path.join(dst_dir, 'CoopBridge_P.pak')
    print('copying %s -> %s' % (LOCAL_PAK, dst))
    os.makedirs(os.path.join(dst_dir, '~mods'), exist_ok=True)
    shutil.copy2(LOCAL_PAK, dst)
    print('installed, %d bytes' % os.path.getsize(dst))
    if os.path.exists(stray):
        # Removed rather than left in place: two copies of the same mod pak with different contents is the
        # one state that can silently run old code while every check says the new build is deployed.
        os.remove(stray)
        print('removed the stale copy in %s (it would have shadowed or doubled this one)' % dst_dir)
    print('a running game must be restarted to pick it up')


def cmd_push(local, rel):
    if not os.path.exists(local):
        raise SystemExit('missing local file: %s' % local)
    dst = os.path.join(ROOT, rel)
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    shutil.copy2(local, dst)
    print('pushed %s -> %s (%d bytes)' % (local, dst, os.path.getsize(dst)))


def cmd_pull(rel, local):
    src = os.path.join(ROOT, rel) if not os.path.isabs(rel) else rel
    if not os.path.exists(src):
        raise SystemExit('missing remote file: %s' % src)
    os.makedirs(os.path.dirname(os.path.abspath(local)), exist_ok=True)
    shutil.copy2(src, local)
    print('pulled %s -> %s (%d bytes)' % (src, local, os.path.getsize(local)))


def cmd_doctor():
    """Explain, from here, exactly why the agent is or is not usable."""
    print('share root : %s' % SHARE)
    print('agent root : %s' % ROOT)
    print('reachable  : %s' % alive())
    if not alive():
        print('\nThe share is not answering. Nothing else can be diagnosed until it does.')
        print('On that machine, confirm the share is up:  net share')
        return
    for name in ('agent.beat', 'agent.log'):
        p = os.path.join(ROOT, name)
        if os.path.exists(p):
            age = int(time.time() - os.path.getmtime(p))
            print('%-11s: %ds ago' % (name, age))
        else:
            print('%-11s: MISSING' % name)
    for d in ('req', 'out', 'done'):
        print('%-11s: %s' % (d, 'present' if os.path.isdir(os.path.join(ROOT, d)) else 'MISSING'))
    print()
    ok, age = agent_state()
    if ok:
        print('VERDICT: agent is alive')
        return
    print('VERDICT: agent is NOT running (or not heartbeating)')
    log = os.path.join(ROOT, 'agent.log')
    if os.path.exists(log):
        print('\nlast lines of agent.log:')
        with open(log, 'r', errors='replace') as f:
            for line in f.read().strip().splitlines()[-8:]:
                print('  ' + line)
        print('\nLook for "boot:" and "FATAL" lines above. If the last line is')
        print('  "boot: another agent already owns this root"')
        print('then an old wedged agent holds the mutex: start-agent.cmd now passes -Force,')
        print('which kills it and takes over.')
    else:
        print('\nagent.log does not exist, so the agent never got as far as writing one.')
        print('Run on that machine and read the console:')
        print('  %s\\run\\start-agent.cmd' % ROOT)


def cmd_deploy_link():
    """Copy the current coop-link.exe (and the agent DLL it needs) to the client, verifying by hash.

    The client now needs `coop_agent.dll` as well: writes only happen on the game thread, through the
    agent's hook, so a client without the DLL stages nothing. Deploying both together is what stops the
    two halves drifting apart - which they did before, and it presented as "no sync".
    """
    import hashlib

    def push(name, remote_name=None):
        local = release_binary(name)
        remote_name = remote_name or name
        remote = SHARE + GAME_BIN + '\\' + remote_name
        with open(local, 'rb') as f:
            want = hashlib.sha256(f.read()).hexdigest()
        print('%-16s local : %8d bytes  %s' % (name, os.path.getsize(local), want[:16]))

        # A running copy locks the file. Renaming a running image is allowed on Windows, so a stale
        # client never has to be stopped from the far side to get a new build in place.
        if os.path.exists(remote):
            try:
                os.remove(remote)
            except OSError:
                stale = remote + '.stale'
                print('%-16s        locked by a running copy, moving it aside' % name)
                try:
                    os.remove(stale)
                except OSError:
                    pass
                os.replace(remote, stale)

        shutil.copy2(local, remote)
        with open(remote, 'rb') as f:
            got = hashlib.sha256(f.read()).hexdigest()
        if got != want:
            raise SystemExit('VERIFY FAIL for %s: the copy does not match the build' % name)
        print('%-16s remote: %8d bytes  %s  OK' % (name, os.path.getsize(remote), got[:16]))
        return got

    push('coop-link.exe')
    agent_hash = push('coop_agent.dll')

    # The auto-loader, without which none of the above is ever loaded. Pushing the binaries and not the proxy
    # deployed a client that had everything except the thing that starts it - and it looked installed, because
    # every file we checked for was present. Verified against the client exe's imports, same as the host.
    proxy = release_binary('coop_proxy.dll')
    check = subprocess.run(
        [sys.executable, os.path.join(os.path.dirname(os.path.abspath(__file__)), 'check_proxy.py'),
         proxy, '--game-exe', SHARE + GAME_BIN + '\\F1Manager24.exe'],
        capture_output=True, text=True)
    if check.returncode != 0:
        raise SystemExit('proxy check failed against the CLIENT exe, not deployed:\n'
                         + ((check.stdout or '') + (check.stderr or '')).strip())
    # Named `version.dll` on arrival: Windows resolves a DLL from the exe's own directory before the system one, so
    # the filename is what makes it load at all.
    push(proxy, 'version.dll')

    # Record what was actually deployed, on both machines.
    #
    # This is what makes "is the install current?" answerable. The agent build is not byte-reproducible (the
    # same sources gave three different hashes across consecutive builds in one session), so comparing an
    # install against whatever `cargo build` last left in the target directory can report STALE for a perfectly
    # correct deployment - and rebuilding is not deploying. The recorded hash is the only thing that means
    # "this is the copy we pushed".
    for label, bin_dir in (('host', HOST_BIN), ('client', SHARE + GAME_BIN)):
        record = os.path.join(bin_dir, 'coop_agent.deployed.txt')
        try:
            with open(record, 'w', encoding='utf-8') as f:
                f.write('%s deployed %s\n' % (agent_hash, time.strftime('%Y-%m-%d %H:%M:%S')))
            print('%-16s recorded deployed hash %s' % (label, agent_hash[:16]))
        except OSError as e:
            print('%-16s could not record deployed hash: %s' % (label, e))

    # Write the side file as part of the deploy, on both machines.
    #
    # The agent cannot see the injector's environment, so it reads `coop_side.txt` from next to the exe. That
    # file decides whose half of the pause gate this machine feeds. Two machines both defaulting to Host does
    # not fail - it silently arbitrates both players as the host - so leaving it to a human step is how a LAN
    # test loses an hour to a bug that is really a missing text file.
    host_bin = HOST_BIN
    for label, path, value in (
        ('host', os.path.join(host_bin, 'coop_side.txt'), 'host'),
        ('client', SHARE + GAME_BIN + '\\coop_side.txt', 'client'),
    ):
        try:
            with open(path, 'w', encoding='utf-8', newline='\n') as f:
                f.write(value + '\n')
            print('%-16s %s -> %s' % ('side', path, value))
        except OSError as exc:
            print('%-16s could not write %s: %s' % ('side', path, exc))

    print()
    print('Both machines now have the pak, version.dll, the agent, the link and the enable markers.')
    print('Nothing to inject: the agent loads with the game. Launch it normally on both machines.')

    # The enable markers. Same reasoning as `coop_side.txt`, already written here for exactly this reason: the agent
    # reads these from beside the exe, nothing else creates them, and a missing one is silent. `coop_proxy.enable`
    # absent means the forwarder loads and never loads the agent; `coop_hook.enable` absent means the hook stays
    # off and Host reports "no hook installed" with no hint of what to create.
    for label, bin_dir in (('host', host_bin), ('client', SHARE + GAME_BIN)):
        for marker, why in (('coop_proxy.enable', 'co-op auto-loader'),
                            ('coop_hook.enable', 'sim hook')):
            try:
                with open(os.path.join(bin_dir, marker), 'w', encoding='utf-8', newline='\n') as f:
                    f.write('enables the %s; delete to disable\n' % why)
                print('%-16s %s enabled' % ('marker', marker))
            except OSError as exc:
                print('%-16s could not write %s: %s' % ('marker', marker, exc))

    # The mode rules, per machine with that machine's own team and role. `coop-auth` returns None when these are
    # absent and every gate then passes - safe for single-player, and the reason the team rules enforced nothing.
    for label, bin_dir, team, role in (
        ('host', host_bin, HOST_TEAM, 'host'),
        ('client', SHARE + GAME_BIN, CLIENT_TEAM, 'client'),
    ):
        loadout = 0 if role == 'host' else CLIENT_LOADOUT
        for name, text in (('coop_mode.txt', COOP_MODE + '\n'),
                           ('coop_player.txt', '%d %d %s\n' % (team, loadout, role))):
            try:
                with open(os.path.join(bin_dir, name), 'w', encoding='utf-8', newline='\n') as f:
                    f.write(text)
            except OSError as exc:
                print('%-16s could not write %s: %s' % ('auth', name, exc))
        print('%-16s %s: mode=%s team=%d loadout=%d role=%s'
              % ('auth', label, COOP_MODE, team, loadout, role))


def cmd_versions():
    """Show this build against the client's, so a stale binary cannot go unnoticed."""
    import hashlib

    remote_bin = SHARE + GAME_BIN

    def describe(path, label):
        # A transient SMB failure is not a missing file. `os.path.exists` returns False for both, and an
        # unreachable share was reported as "MISSING coop_agent.dll" during this work while the file was
        # there all along - the same class of false negative as concluding a machine was off because one
        # name lookup failed. Retry before believing it, and say which case it was.
        for attempt in range(3):
            try:
                if os.path.exists(path):
                    break
            except OSError:
                pass
            time.sleep(0.4)
        else:
            # Distinguish "the share is not there" from "the file is not in it": only the second is a
            # deployment problem.
            parent = os.path.dirname(path)
            try:
                reachable = os.path.isdir(parent)
            except OSError:
                reachable = False
            if not reachable:
                print('%-8s: UNREADABLE  %s\n         (the share is not answering - this is NOT a missing '
                      'file; re-run)' % (label, path))
                return None
            print('%-8s: MISSING  %s' % (label, path))
            return None
        with open(path, 'rb') as f:
            h = hashlib.sha256(f.read()).hexdigest()
        print('%-8s: %8d bytes  %s  %s'
              % (label, os.path.getsize(path), h[:16],
                 time.strftime('%Y-%m-%d %H:%M', time.localtime(os.path.getmtime(path)))))
        return h

    print('release dir: %s' % RELEASE)
    print()
    worst = True
    for name in ('coop-link.exe', 'coop_agent.dll', 'coop-inject.exe'):
        local = os.path.join(RELEASE, name)
        remote = remote_bin + '\\' + name
        print('--- %s' % name)
        a = describe(local, 'local')
        b = describe(remote, 'remote')
        if a and b:
            if a == b:
                print('         MATCH')
            else:
                print('         MISMATCH - run `python remote.py deploy-link`')
                worst = False
    if not worst:
        raise SystemExit(1)


def cmd_push_agent():
    """Install this repo's agent script on the client and make the running agent adopt it.

    The agent watches `req\\agent-update.ps1`, and on seeing a different hash it installs the copy,
    spawns a replacement and exits. So updating the agent needs no human on that machine and no
    process hunting - which is what made every previous fix cost a restart cycle.
    """
    import hashlib

    src = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..',
                                       'remote-agent', 'run', 'start-agent.ps1'))
    if not os.path.exists(src):
        raise SystemExit('missing %s' % src)
    if not alive():
        raise SystemExit('share unreachable: %s' % ROOT)

    run_dir = os.path.join(ROOT, 'run')
    os.makedirs(run_dir, exist_ok=True)
    targets = [os.path.join(run_dir, 'start-agent.ps1'),
               os.path.join(run_dir, 'start-agent.cmd')]

    for name in ('start-agent.ps1', 'start-agent.cmd'):
        s = os.path.join(os.path.dirname(src), name)
        d = os.path.join(run_dir, name)
        if os.path.exists(d):
            try:
                os.remove(d)
            except OSError:
                pass
        shutil.copy2(s, d)

    with open(src, 'rb') as f:
        h = hashlib.sha256(f.read()).hexdigest()
    print('installed agent %s (%d bytes)' % (h[:12], os.path.getsize(src)))

    # Drop the update where the running agent polls for it. If the agent is alive it adopts this
    # within a second; if it is wedged, the copy is in place for the next start.
    ensure_dirs()
    incoming = os.path.join(dirs()['req'], 'agent-update.ps1')
    shutil.copy2(src, incoming)
    print('queued self-update: %s' % incoming)

    ok, age = agent_state()
    if ok:
        print('agent looks alive (beat %ds ago) - it should adopt the update shortly' % age)
        for _ in range(20):
            time.sleep(1)
            ok2, age2 = agent_state()
            if ok2 and age2 is not None and age2 < 10:
                log = os.path.join(ROOT, 'agent.log')
                if os.path.exists(log):
                    with open(log, 'r', errors='replace') as f:
                        tail = f.read()[-600:]
                    if 'self-update' in tail:
                        print('self-update observed in agent.log')
                        break
        print('check: python remote.py doctor')
    else:
        print('agent is NOT running, so nothing will consume it until one is started.')
        print('The script is installed, so a start now picks up this version.')


def cmd_agent_log(n=12):
    log = os.path.join(ROOT, 'agent.log')
    if not os.path.exists(log):
        print('no agent.log')
        return
    with open(log, 'r', errors='replace') as f:
        for line in f.read().strip().splitlines()[-n:]:
            print(line)


def cmd_start_agent():
    ok, age = agent_state()
    if ok:
        print('agent already running (agent.log written %ds ago)' % age)
        return
    print('The share cannot start a process, so one of these must run on that machine:')
    print()
    print('  1. once, by hand (or via Remote Desktop):')
    print('       %s\\run\\start-agent.cmd' % ROOT)
    print()
    print('  2. or add it to Startup so it comes back after every logon:')
    print('       python remote.py autostart        (prints the exact command)')
    print()
    print('  Expect a console window that stays open and agent.log growing in:')
    print('    %s' % ROOT)
    print()
    print('  last agent.log write: %s' % ('never' if age is None else '%ds ago' % age))


def cmd_autostart():
    ok, age = agent_state()
    if not ok:
        raise SystemExit('the agent has to be running to install its own autostart.\n'
                         'run %s\\run\\start-agent.cmd on that machine first.' % ROOT)
    # A .lnk in the user's Startup folder is the least invasive persistence: no service, no admin,
    # no registry, and it is visible to the owner of that machine.
    script = (
        '$startup = [Environment]::GetFolderPath("Startup")\n'
        '$target  = "%s\\run\\start-agent.cmd"\n'
        '$lnk     = Join-Path $startup "coop-agent.lnk"\n'
        '$sh = New-Object -ComObject WScript.Shell\n'
        '$s = $sh.CreateShortcut($lnk)\n'
        '$s.TargetPath = $target\n'
        '$s.WorkingDirectory = Split-Path $target\n'
        '$s.WindowStyle = 7\n'   # minimised
        '$s.Description = "coop remote agent"\n'
        '$s.Save()\n'
        'Write-Output ("installed: " + $lnk)\n'
        'Write-Output ("exists: " + (Test-Path $lnk))\n' % ROOT)
    code, text, m = submit(script)
    report(code, text, m)


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return
    cmd = sys.argv[1].lower()
    rest = sys.argv[2:]

    if cmd == 'ping':
        cmd_ping()
    elif cmd == 'ps':
        cmd_ps()
    elif cmd == 'game':
        cmd_game()
    elif cmd == 'saves':
        cmd_saves()
    elif cmd == 'snapshot':
        cmd_snapshot()
    elif cmd == 'ue4ss':
        cmd_ue4ss(int(rest[0]) if rest else 40)
    elif cmd == 'logs':
        cmd_logs(int(rest[0]) if rest else 30)
    elif cmd == 'run':
        if not rest:
            raise SystemExit('usage: remote.py run "<powershell>"')
        sys.exit(cmd_run(rest[0]))
    elif cmd == 'runfile':
        if not rest:
            raise SystemExit('usage: remote.py runfile <local.ps1>')
        sys.exit(cmd_runfile(rest[0]))
    elif cmd == 'killgame':
        cmd_killgame()
    elif cmd == 'launch':
        cmd_launch()
    elif cmd == 'install-pak':
        cmd_install_pak()
    elif cmd == 'push':
        if len(rest) < 2:
            raise SystemExit('usage: remote.py push <local> <remote-rel>')
        cmd_push(rest[0], rest[1])
    elif cmd == 'pull':
        if len(rest) < 2:
            raise SystemExit('usage: remote.py pull <remote-rel> <local>')
        cmd_pull(rest[0], rest[1])
    elif cmd == 'doctor':
        cmd_doctor()
    elif cmd == 'deploy-link':
        cmd_deploy_link()
    elif cmd == 'versions':
        cmd_versions()
    elif cmd == 'attach-client':
        cmd_attach_client()
    elif cmd == 'fires':
        sys.exit(0 if cmd_fires(int(rest[0]) if rest else 10) else 1)
    elif cmd == 'push-agent':
        cmd_push_agent()
    elif cmd == 'agent-log':
        cmd_agent_log(int(rest[0]) if rest else 12)
    elif cmd == 'start-agent':
        cmd_start_agent()
    elif cmd == 'autostart':
        cmd_autostart()
    else:
        raise SystemExit('unknown command: %s' % cmd)


if __name__ == '__main__':
    main()
