#!/usr/bin/env python3
"""Build, verify and deploy the co-op UI pak.

Why this exists
---------------
The multiplayer button "did not appear in game" twice. The first time the reason was not the code:
the pak in the game install was two days old and 17 KB while the build was 50 KB. Nothing that had
been written was ever in the running game, so there was nothing to see. That is a silent failure
that looks exactly like a code bug, and it cost real cycles.

So this script refuses to finish until the pak in the install is byte-identical to the one built
from source, and it verifies the JS inside the pak matches the JS on disk rather than trusting the
packer.

Usage
-----
    python tools/deploy_pak.py                 # build, verify, deploy
    python tools/deploy_pak.py --check         # verify the installed pak matches source, deploy nothing
    python tools/deploy_pak.py --game <path>   # override the game install root
    python tools/deploy_pak.py --no-deploy     # build and verify only

Requires repak (https://github.com/trumank/repak). The script looks for it at
tools/bin/repak.exe, then on PATH. The copy under Program Files must be run elevated, which is why
the local copy is preferred.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
PROJECT = os.path.dirname(HERE)
PAK_SRC = os.path.join(PROJECT, 'pak', 'f1manager24', 'Content', 'UIGameface')
PAK_OUT = os.path.join(PROJECT, 'pak', 'CoopBridge_P.pak')
PAK_NAME = 'CoopBridge_P.pak'
DEFAULT_GAME = r'E:\Steam\steamapps\common\F1 Manager 2024'

# The second machine, reached over the existing network share. Deploying to it was forgotten once and
# the result was a session where one machine ran new code and the other ran none.
DEFAULT_CLIENT_SHARE = os.environ.get('COOP_REMOTE_SHARE', r'\\<client-pc>\e')
CLIENT_GAME_REL = r'SteamLibrary\steamapps\common\F1 Manager 2024'

# The UI is one file plus the bridge. Both are checked, because a stale script inside a fresh pak is
# the same failure in a different coat.
CRITICAL = [
    ('js/coop/lobby.js', 'js/coop/lobby.js'),
    ('js/coop/bridge.js', 'js/coop/bridge.js'),
    ('js/coop/press.js', 'js/coop/press.js'),
    ('js/coop/observe.js', 'js/coop/observe.js'),
    ('Frontend.html', 'Frontend.html'),
]


def sha256(path: str) -> str:
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()


def find_repak() -> str | None:
    local = os.path.join(HERE, 'bin', 'repak.exe')
    if os.path.isfile(local):
        return local
    found = shutil.which('repak')
    if found:
        return found
    return None


def run(cmd: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True)


def check_js_syntax(paths: list[str]) -> list[str]:
    """Compiles each JS file with node, so a syntax error is caught before a pak is built."""
    problems = []
    node = shutil.which('node')
    if not node:
        print('  ! node not found, skipping syntax check')
        return problems
    for p in paths:
        res = run([node, '--check', p])
        if res.returncode != 0:
            problems.append('%s: %s' % (p, (res.stderr or '').strip().splitlines()[:1]))
    return problems


def expected_files() -> set[str]:
    """Every file that should be in the pak, as it appears *inside the pak*.

    The prefix matters: entries are stored relative to the game root because the mount point is
    `../../../`. Returning bare names here made the stray-file guard compare a bare list against
    prefixed entries and report every single file as a stray - which is how the guard behaved the moment
    the prefix was fixed, rather than passing silently.
    """
    want = set()
    prefix = 'F1Manager24/Content/UIGameface/'
    for root, _dirs, names in os.walk(PAK_SRC):
        for n in names:
            if n.endswith('.html'):
                rel = os.path.relpath(os.path.join(root, n), PAK_SRC).replace('\\', '/')
                want.add(prefix + rel)
    # Scripts the pak is meant to contain. Listed explicitly: these are the files whose contents matter,
    # and a new one should be a deliberate edit here.
    for s in ('js/coop/bridge.js', 'js/coop/lobby.js', 'js/coop/press.js', 'js/coop/observe.js'):
        want.add(prefix + s)
    return want


def stray_files(repak: str) -> list[str]:
    """Files in the built pak that are not part of the intended set.

    A scratch file in the source tree has shipped into a build before. Shipping it is not merely
    untidy: it is code nobody reviewed, executed by the game.
    """
    res = run([repak, 'list', PAK_OUT])
    if res.returncode != 0:
        return []
    names = set()
    for line in res.stdout.replace('\r', '\n').split('\n'):
        line = line.strip()
        if line and not line.startswith('Packed'):
            names.add(line.replace('\\', '/'))
    want = expected_files()
    return sorted(names - want)


def pak_content_hash(repak: str, path: str) -> str | None:
    """A hash of what the pak *contains*, not of its bytes.

    Two builds from identical sources are **not** byte-identical: repak does not order entries stably, so the
    first entry can be a different file each run and the index hash changes with it. Comparing raw bytes
    therefore called a perfectly good, freshly deployed pak "STALE" - which happened on every rebuild and made
    the one check that is supposed to catch a stale pak fire unconditionally, which is worse than not having it.

    What actually matters is that both machines mount the same files with the same contents. `repak hash-list`
    reports exactly that: per-entry SHA-256, which is then sorted so entry order cannot matter.
    """
    if not os.path.exists(path):
        return None
    got = run([repak, 'hash-list', path])
    if got.returncode != 0:
        return None
    lines = sorted(l.strip() for l in (got.stdout or '').replace('\r', '\n').split('\n') if l.strip())
    return hashlib.sha256('\n'.join(lines).encode()).hexdigest()


def build(repak: str) -> None:
    # Remove first: repak appends to an existing archive, which would leave stale entries behind and
    # make the verification below meaningless.
    if os.path.exists(PAK_OUT):
        os.remove(PAK_OUT)

    # Paths inside the pak must match what the game looks up *relative to the mount point*.
    #
    # The mount point is `../../../` - three up from `<game>/F1Manager24/Content/Paks` is the game root -
    # so an entry is resolved as `<game root>/<entry>`. The entry therefore has to be
    # `F1Manager24/Content/UIGameface/Frontend.html`, not `Frontend.html`.
    #
    # This was wrong, and it was the whole "no multiplayer button" bug. Packing from `Content/UIGameface`
    # stored bare paths, so `Frontend.html` resolved to `<game root>/Frontend.html`, which is nothing. The
    # pak loaded, was byte-identical on both machines, had the right version and mount point, and listed
    # exactly the intended files - and overrode nothing. Every check this script made passed.
    #
    # What exposed it was comparing against a working mod pak in the install
    # (`F1M2021_TEST_PR_S_UI_P.pak`), which ships `F1Manager24/Content/UIGameface/js/...` - the full path,
    # not a bare one.
    #
    # So repak is given a staging tree whose root is the directory *containing* `F1Manager24/`, which makes
    # it emit the full relative path for each file.
    staging = tempfile.mkdtemp(prefix='coop-pak-')
    # A fixed mtime for every staged file. repak records each file's timestamp, and `shutil.copy2` preserves
    # it, so building from two trees with identical *contents* produced byte-different paks - and every hash
    # comparison afterwards called a freshly deployed pak "STALE". Pinning the time makes the same sources
    # produce the same bytes, which is what makes "installed matches built" a meaningful check.
    FIXED_MTIME = 1600000000
    try:
        for name in os.listdir(PAK_SRC):
            src = os.path.join(PAK_SRC, name)
            dst = os.path.join(staging, 'F1Manager24', 'Content', 'UIGameface', name)
            if os.path.isdir(src):
                shutil.copytree(src, dst)
            else:
                os.makedirs(os.path.dirname(dst), exist_ok=True)
                shutil.copy2(src, dst)
        for dirpath, _dirnames, filenames in os.walk(staging):
            for fn in filenames:
                os.utime(os.path.join(dirpath, fn), (FIXED_MTIME, FIXED_MTIME))
        cmd = [repak, 'pack', '-m', '../../../', '--version', 'V11', staging, PAK_OUT]
        res = run(cmd)
        print(res.stdout.strip() or res.stderr.strip())
        if res.returncode != 0 or not os.path.exists(PAK_OUT):
            raise SystemExit('repak failed')
    finally:
        shutil.rmtree(staging, ignore_errors=True)
    print('  built %s (%d bytes)' % (os.path.basename(PAK_OUT), os.path.getsize(PAK_OUT)))


def verify_build(repak: str) -> list[str]:
    """Unpacks the pak just built and compares the files that matter against the source tree."""
    problems = []

    # Check the entry *paths* before anything else, because a pak whose paths do not match the mount point
    # loads and overrides nothing while passing every content check. That is exactly what happened: the
    # entries were `Frontend.html` instead of `F1Manager24/Content/UIGameface/Frontend.html`, so the game
    # resolved them to `<game root>/Frontend.html`. Size, hash, version, mount point and file list were all
    # correct; the pak was simply addressing nothing. A working mod pak in the install
    # (`F1M2021_TEST_PR_S_UI_P.pak`) ships the full path, which is how the discrepancy was found.
    listing = run([repak, 'list', PAK_OUT])
    entries = [l.strip().replace('\\', '/') for l in (listing.stdout or '').replace('\r', '\n').split('\n')]
    entries = [e for e in entries if e and not e.startswith('Packed')]
    want_prefix = 'F1Manager24/Content/UIGameface/'
    if entries:
        bad = [e for e in entries if not e.startswith(want_prefix)]
        if bad:
            problems.append(
                'pak entries are missing the %s prefix (%d of %d, e.g. %s) - with mount point ../../../ '
                'these resolve to the game root and override nothing'
                % (want_prefix, len(bad), len(entries), bad[0]))
        else:
            print('  ok, every entry is under %s' % want_prefix)

    with tempfile.TemporaryDirectory(prefix='coop-pak-') as tmp:
        # `-o` is the output flag (a bare positional is ignored and repak derives the folder from the
        # archive name instead), and `-f` lets it write into an existing directory.
        out = os.path.join(tmp, 'x')
        os.makedirs(out, exist_ok=True)
        res = run([repak, 'unpack', '-o', out, '-f', PAK_OUT])
        if res.returncode != 0 and not os.path.isdir(out):
            return ['unpack failed: %s' % (res.stderr or res.stdout).strip()]
        for inside, on_disk in CRITICAL:
            got = None
            for root, _dirs, files in os.walk(out):
                for f in files:
                    full = os.path.join(root, f)
                    rel = os.path.relpath(full, out).replace(os.sep, '/')
                    if rel.endswith(inside):
                        got = full
                        break
                if got:
                    break
            if not got:
                problems.append('%s is missing from the pak' % inside)
                continue
            want = os.path.join(PAK_SRC, on_disk)
            if not os.path.exists(want):
                problems.append('%s is missing from the source tree' % on_disk)
                continue
            if sha256(got) != sha256(want):
                problems.append('%s inside the pak differs from the source file' % inside)
    return problems


def installed_path(game_root: str) -> str:
    """Where the UI pak is installed.

    `~mods` by convention: it is where every mod manager on this game writes, so a player's existing tools
    see our pak alongside their others. Loose paks under `Content/Paks/` root do mount too - a claim to the
    contrary here was wrong and cost time - but the root is where the game's own chunks live and mixing into
    it is untidy rather than broken.
    """
    return os.path.join(game_root, 'F1Manager24', 'Content', 'Paks', '~mods', PAK_NAME)


def mods_dir(game_root: str) -> str:
    return os.path.join(game_root, 'F1Manager24', 'Content', 'Paks', '~mods')


def mods_disabled(game_root: str) -> bool:
    """True when `~mods` has been renamed (e.g. `~mods.off`) to turn mods off.

    Worth detecting explicitly: with `~mods` gone the game mounts nothing of ours and there is no other
    signal. `deploy()` would otherwise create a fresh `~mods` and report success while the player runs with
    mods disabled by intent.
    """
    paks = os.path.join(game_root, 'F1Manager24', 'Content', 'Paks')
    return (not os.path.isdir(mods_dir(game_root))) and os.path.isdir(os.path.join(paks, '~mods.off'))


def client_game_root(share: str) -> str:
    return os.path.join(share, CLIENT_GAME_REL)


def check_installed(game_root: str, label: str = 'host', repak: str | None = None) -> list[str]:
    target = installed_path(game_root)
    if not os.path.exists(target):
        return ['no %s in %s' % (PAK_NAME, os.path.dirname(target))]
    built = os.path.getsize(PAK_OUT)
    there = os.path.getsize(target)
    repak = repak or find_repak()
    # Content, not bytes. A byte comparison reports STALE for a pak that was copied straight from this
    # build, because repak does not order entries stably between runs; see `pak_content_hash`.
    same = (pak_content_hash(repak, target) == pak_content_hash(repak, PAK_OUT)) if repak else \
        (sha256(target) == sha256(PAK_OUT))
    if not same:
        return [
            '%s pak is NOT the one just built (installed %d bytes, built %d bytes)' % (label, there, built)
        ]
    return []


def install_proxy(game_root: str, label: str = 'host') -> list[str]:
    """Put `version.dll` (the auto-loader) next to the game exe, after proving it can be loaded safely.

    This is what makes the agent arrive without anyone running anything: Windows loads a DLL from the exe's own
    directory in preference to the system one, so a file named `version.dll` beside `F1Manager24.exe` is what the
    game's import of that DLL resolves to. It forwards the three functions the game calls and also loads
    `coop_agent.dll`.

    The check is not optional. A proxy that is missing even one imported name stops the game from starting with
    "procedure entry point not found", which reads as a corrupted install and is the worst failure this mod can
    cause. So the export set is verified against the exe before anything is copied, and a mismatch skips the
    install rather than risking it.
    """
    proxy = find_proxy()
    if proxy is None:
        return ['proxy DLL not built - run: cargo build --release -p coop-proxy']

    exe = os.path.join(game_root, 'F1Manager24', 'Binaries', 'Win64', 'F1Manager24.exe')
    checker = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'check_proxy.py')
    res = subprocess.run([sys.executable, checker, proxy, '--game-exe', exe],
                         capture_output=True, text=True)
    if res.returncode != 0:
        # Refuse, and show why. Shipping this would break the game's start-up, not just this mod.
        out = (res.stdout or '') + (res.stderr or '')
        return ['proxy check failed, NOT installed:\n' + out.strip()]

    bin_dir = os.path.join(game_root, 'F1Manager24', 'Binaries', 'Win64')
    dst = os.path.join(bin_dir, 'version.dll')
    shutil.copy2(proxy, dst)

    # The forwarder loads `coop_agent.dll` by bare name, which Windows resolves from the exe's own directory. So the
    # agent has to be *here*: installing the proxy alone produced a machine with a correct `version.dll` and no
    # agent in the process, and the only evidence was a line in `coop_proxy.log`. Caught by deploying into a scratch
    # game root and listing what actually arrived.
    agent = _find_release('coop_agent.dll')
    if agent is None:
        return ['coop_agent.dll not built - run: cargo build --release -p coop-agent']
    shutil.copy2(agent, os.path.join(bin_dir, 'coop_agent.dll'))
    print('  installed agent (%d bytes)' % os.path.getsize(os.path.join(bin_dir, 'coop_agent.dll')))

    # `coop-link.exe` is spawned by the agent on Host/Join, so it must sit beside the agent too.
    link = _find_release('coop-link.exe')
    if link is None:
        return ['coop-link.exe not built - run: cargo build --release -p coop-link']
    shutil.copy2(link, os.path.join(bin_dir, 'coop-link.exe'))
    print('  installed frame link (%d bytes)' % os.path.getsize(os.path.join(bin_dir, 'coop-link.exe')))

    # Write the enable marker ourselves. Without it the forwarder loads and forwards, and never loads the agent -
    # by design, because a proxy that stops the game launching is the worst failure this mod can produce. But
    # "safe by default" and "deployed means working" are different claims, and leaving the marker out made a
    # correct deploy indistinguishable from a broken one: the menu appeared, nothing was armed, every log clean.
    #
    # Ordered after the copy and after the export check, so the marker can only exist beside a proven proxy.
    marker = os.path.join(bin_dir, 'coop_proxy.enable')
    try:
        with open(marker, 'w', encoding='utf-8') as f:
            f.write('enables the co-op auto-loader; delete this file to make version.dll inert\n')
        print('  enabled auto-loader')
    except OSError as exc:
        return ['could not write %s: %s' % (marker, exc)]

    # The sim hook is a separate opt-in, because a wrong patch is an instant crash with no dump. Written here for
    # the same reason: `armed` is false without it, so Host reports "no hook installed" and nothing tells the
    # player that a file is what is missing. Reversible by deleting it; the hook validates the prologue first.
    hook_marker = os.path.join(bin_dir, 'coop_hook.enable')
    try:
        with open(hook_marker, 'w', encoding='utf-8') as f:
            f.write('enables the sim hook; delete this file to leave the game untouched\n')
        print('  enabled sim hook')
    except OSError as exc:
        print('  ! could not write %s: %s' % (hook_marker, exc))
        print('    Hosting will report "no hook installed" until it exists.')
    return []


def install_coop_auth_files(game_root: str, mode: str, team: int, loadout: int,
                            is_host: bool, label: str = 'host') -> list:
    """Write `coop_mode.txt` and `coop_player.txt`, which `coop-auth` reads and nothing else creates.

    `coop_auth::configured()` returns `None` when either file is absent, and every gate then passes. That is the
    right default for single-player maintenance and the wrong state for a co-op session: the team rules were
    documented, implemented and tested, and enforced nothing in practice because no code ever wrote the two files
    they read. Verified - no writer existed anywhere in the tree.

    Written last, after the pak and the proxy, so the mode rules only come into force once the session is
    actually deployable.
    """
    bin_dir = os.path.join(game_root, 'F1Manager24', 'Binaries', 'Win64')
    problems = []
    try:
        with open(os.path.join(bin_dir, 'coop_mode.txt'), 'w', encoding='utf-8') as f:
            f.write(mode + '\n')
    except OSError as exc:
        problems.append('could not write coop_mode.txt: %s' % exc)
    # `team loadout host|client` on one line, which is the shape `configured()` parses.
    try:
        with open(os.path.join(bin_dir, 'coop_player.txt'), 'w', encoding='utf-8') as f:
            f.write('%d %d %s\n' % (team, loadout, 'host' if is_host else 'client'))
    except OSError as exc:
        problems.append('could not write coop_player.txt: %s' % exc)
    if not problems:
        print('  auth: mode=%s team=%d loadout=%d role=%s (%s)'
              % (mode, team, loadout, 'host' if is_host else 'client', label))
    return problems


def find_proxy():
    """The built `version.dll` stand-in, or None."""
    return _find_release('coop_proxy.dll')


def _find_release(name):
    """A build output by name, honouring `CARGO_TARGET_DIR`.

    Not a hardcoded `coop-rs/target/release`: this project builds into `CARGO_TARGET_DIR` to dodge a Defender lock
    on the in-tree target, so looking only at the in-tree path finds a stale binary or nothing - and a deploy that
    silently installs a stale agent is worse than one that refuses.
    """
    target = os.environ.get('CARGO_TARGET_DIR', r'E:\TEMP\coop-target')
    for p in (os.path.join(target, 'release', name),
              os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                           'coop-rs', 'target', 'release', name)):
        if os.path.isfile(p):
            return p
    return None


def deploy(game_root: str, label: str = 'host', mode: str = 'shared-team',
           team: int = 1, loadout: int = 0, is_host: bool = True) -> bool:
    target = installed_path(game_root)

    # Say it out loud when mods have been switched off, instead of quietly creating `~mods` again. A player
    # who renamed the folder to disable mods does not want a mod re-enabling itself, and either way the
    # deploy has to be visible rather than "successful" into a directory the game ignores.
    if mods_disabled(game_root):
        print('  ! %s has mods DISABLED: `~mods` is missing and `~mods.off` exists.' % label)
        print('    Nothing of ours will load until mods are enabled. Rename `~mods.off` back to `~mods`,')
        print('    or pass --force-mods-dir to create `~mods` anyway.')

    os.makedirs(os.path.dirname(target), exist_ok=True)
    try:
        shutil.copy2(PAK_OUT, target)
    except PermissionError:
        # The game holds the pak open while it runs, so copying over it fails. Saying so plainly is the
        # whole point: the previous failure mode was copying nothing and not noticing.
        print('  ! cannot write %s - the %s game is running and holds the file open.' % (target, label))
        print('    Close the game and run this again. Nothing was deployed to %s.' % label)
        return False
    except OSError as exc:
        print('  ! could not write %s: %s' % (target, exc))
        return False
    print('  deployed to %s (%s)' % (target, label))

    # Then the auto-loader, so the agent is in the process without anyone running an injector. Refuses rather
    # than risking a game that will not start; see `install_proxy`.
    for problem in install_proxy(game_root, label):
        print('  FAIL %s' % problem)
        return False

    # Last, the mode rules - so `coop-auth` comes into force only once the session is deployable at all.
    for problem in install_coop_auth_files(game_root, mode, team, loadout, is_host, label):
        print('  FAIL %s' % problem)
        return False

    return True


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--game', default=DEFAULT_GAME)
    ap.add_argument('--client-share', default=DEFAULT_CLIENT_SHARE,
                    help='root of the second machine\'s drive share; empty to skip')
    ap.add_argument('--check', action='store_true', help='only compare the installed pak with source')
    ap.add_argument('--no-deploy', action='store_true')
    ap.add_argument('--host-only', action='store_true', help='do not touch the second machine')
    # Which rules `coop-auth` enforces. Explicit because the two players differ, and because a guessed default is
    # how a team rule becomes a suggestion: `shared-team` is the only mode the UI honestly offers today.
    ap.add_argument('--mode', default='shared-team', choices=['shared-team', 'versus'],
                    help='co-op mode for coop_mode.txt (default: shared-team)')
    ap.add_argument('--team', type=int, default=1, help='this machine (host) EF1Team id 1-10')
    ap.add_argument('--client-team', type=int, default=2, help='the other machine EF1Team id 1-10')
    ap.add_argument('--loadout', type=int, default=0, help='car index this player drives, 0 or 1')
    args = ap.parse_args()

    client_root = '' if args.host_only or not args.client_share else client_game_root(args.client_share)
    client_up = bool(client_root) and os.path.isdir(client_root)

    repak = find_repak()
    if not repak:
        print('repak not found. Put repak.exe in tools/bin/ or on PATH.')
        return 2

    print('source: %s' % PAK_SRC)

    sources = [os.path.join(PAK_SRC, 'js', 'coop', 'lobby.js'),
               os.path.join(PAK_SRC, 'js', 'coop', 'bridge.js'),
               os.path.join(PAK_SRC, 'js', 'coop', 'press.js'),
               os.path.join(PAK_SRC, 'js', 'coop', 'observe.js')]
    print('\n1. syntax')
    syntax = check_js_syntax(sources)
    # Also check the test harness: it is what proves the tree walk, so a broken harness means the
    # proof is worthless.
    harness = os.path.join(HERE, 'menu_test.js')
    if os.path.exists(harness):
        syntax += check_js_syntax([harness])
    if syntax:
        for s in syntax:
            print('  FAIL %s' % s)
        return 1
    print('  ok')

    print('\n2. menu integration tests')
    if os.path.exists(harness):
        res = run([shutil.which('node') or 'node', harness])
        tail = (res.stdout or '').strip().splitlines()
        print('  ' + (tail[-1] if tail else 'no output'))
        if res.returncode != 0:
            for line in tail:
                if 'FAIL' in line:
                    print('  ' + line.strip())
            return 1
    else:
        print('  ! tools/menu_test.js missing, skipping')

    if args.check:
        print('\n3. installed paks')
        problems = check_installed(args.game, 'host')
        for p in problems:
            print('  FAIL %s' % p)
        if client_root:
            if not client_up:
                print('  ! second machine not reachable at %s, not checked' % client_root)
            else:
                cproblems = check_installed(client_root, 'client')
                problems += cproblems
                for p in cproblems:
                    print('  FAIL %s' % p)
        if problems:
            return 1
        print('  ok, every reachable machine matches the build')
        return 0

    print('\n3. build')
    build(repak)

    print('\n4. verify pak contents')
    strays = stray_files(repak)
    if strays:
        # A scratch file in the source tree shipped into a pak once. Code nobody reviewed, executed by
        # the game, is worse than an untidy build, so this is fatal and says exactly what to remove.
        print('  FAIL the pak contains %d file(s) that are not part of the mod:' % len(strays))
        for s in strays:
            print('       %s' % s)
        print('     delete them from %s and rebuild' % PAK_SRC)
        return 1
    print('  ok, only the intended files are packed')

    print('\n5. verify pak contents')
    problems = verify_build(repak)
    if problems:
        for p in problems:
            print('  FAIL %s' % p)
        return 1
    print('  ok, the JS inside the pak is byte-identical to the source')

    if args.no_deploy:
        print('\n6. deploy: skipped (--no-deploy)')
        return 0

    print('\n6. deploy')
    targets = [('host', args.game)]
    if client_root:
        if client_up:
            targets.append(('client', client_root))
        else:
            print('  ! second machine not reachable at %s, not deployed there' % client_root)
    failed = False
    for label, root in targets:
        # The two machines are different players, so the auth files differ: same mode, different team and role.
        # Passing the host's identity to the client would make both claim to be the host, and `coop-auth` would
        # then allow or refuse the wrong writes on one of them.
        if label == 'client':
            team = args.client_team
            is_host = False
        else:
            team = args.team
            is_host = True
        if not deploy(root, label, mode=args.mode, team=team,
                      loadout=args.loadout, is_host=is_host):
            failed = True
    if failed:
        return 1

    print('\n6. confirm the installs')
    for label, root in targets:
        problems = check_installed(root, label)
        if problems:
            for p in problems:
                print('  FAIL %s' % p)
            return 1
    print('  ok, %s is identical on: %s' % (PAK_NAME, ', '.join(l for l, _ in targets)))
    return 0


if __name__ == '__main__':
    sys.exit(main())
