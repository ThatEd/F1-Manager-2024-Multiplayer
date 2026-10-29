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
    """Every file that should be in the pak, relative to PAK_SRC, from the HTML pages and the scripts
    they load.

    Derived rather than hard-coded so adding a script to a page cannot silently leave it out of the
    check. This also catches the opposite failure, which happened once: a scratch file left in the coop
    folder was packed and shipped, because nothing compared the pak's contents against the intended set.
    """
    want = set()
    for root, _dirs, names in os.walk(PAK_SRC):
        for n in names:
            if n.endswith('.html'):
                rel = os.path.relpath(os.path.join(root, n), PAK_SRC).replace('\\', '/')
                want.add(rel)
    # Scripts the pak is meant to contain. Listed explicitly: these are the files whose contents matter,
    # and a new one should be a deliberate edit here.
    for s in ('js/coop/bridge.js', 'js/coop/lobby.js', 'js/coop/press.js', 'js/coop/observe.js'):
        want.add(s)
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


def build(repak: str) -> None:
    # Remove first: repak appends to an existing archive, which would leave stale entries behind and
    # make the verification below meaningless.
    if os.path.exists(PAK_OUT):
        os.remove(PAK_OUT)
    mount = os.path.relpath(PROJECT, PROJECT)  # "" -> the pak root is the project root
    cmd = [repak, 'pack', '-m', '../../../', '--version', 'V11', PAK_SRC, PAK_OUT]
    res = run(cmd)
    print(res.stdout.strip() or res.stderr.strip())
    if res.returncode != 0 or not os.path.exists(PAK_OUT):
        raise SystemExit('repak failed')
    print('  built %s (%d bytes)' % (os.path.basename(PAK_OUT), os.path.getsize(PAK_OUT)))


def verify_build(repak: str) -> list[str]:
    """Unpacks the pak just built and compares the files that matter against the source tree."""
    problems = []
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
    return os.path.join(game_root, 'F1Manager24', 'Content', 'Paks', PAK_NAME)


def client_game_root(share: str) -> str:
    return os.path.join(share, CLIENT_GAME_REL)


def check_installed(game_root: str, label: str = 'host') -> list[str]:
    target = installed_path(game_root)
    if not os.path.exists(target):
        return ['no %s in %s' % (PAK_NAME, os.path.dirname(target))]
    built = os.path.getsize(PAK_OUT)
    there = os.path.getsize(target)
    if sha256(target) != sha256(PAK_OUT):
        return [
            '%s pak is NOT the one just built (installed %d bytes, built %d bytes)' % (label, there, built)
        ]
    return []


def deploy(game_root: str, label: str = 'host') -> bool:
    target = installed_path(game_root)
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
    return True


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--game', default=DEFAULT_GAME)
    ap.add_argument('--client-share', default=DEFAULT_CLIENT_SHARE,
                    help='root of the second machine\'s drive share; empty to skip')
    ap.add_argument('--check', action='store_true', help='only compare the installed pak with source')
    ap.add_argument('--no-deploy', action='store_true')
    ap.add_argument('--host-only', action='store_true', help='do not touch the second machine')
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
        if not deploy(root, label):
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
