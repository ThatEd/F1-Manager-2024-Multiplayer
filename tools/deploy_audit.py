#!/usr/bin/env python3
"""Check that a deploy produces every artefact the mod actually needs, and say which is missing.

Why this exists
---------------
The mod needs a specific *set* of files beside `F1Manager24.exe`, and the failure mode is never "the deploy failed"
- it is "the deploy installed almost everything". Each missing piece fails in a different place, and every one of
them is silent:

| file | if absent |
|---|---|
| `version.dll` | the agent is never loaded; nothing reports it |
| `coop_agent.dll` | `version.dll` forwards and loads nothing, logging one line to `coop_proxy.log` |
| `coop-link.exe` | Host/Join reports "coop-link.exe not found beside the agent" |
| `coop_proxy.enable` | `version.dll` is inert by design; the menu appears, nothing is armed |
| `coop_hook.enable` | the sim hook stays off; Host says "no hook installed" |
| `coop_mode.txt` + `coop_player.txt` | `coop-auth` returns None and every team rule passes |
| the pak in `~mods` | the menu does not exist at all |

And a second failure mode, for the same reason: **a stale `coop_agent.dll`**. Present-but-old is indistinguishable
from present-and-current by looking at the file list, and the deploy takes its build from `CARGO_TARGET_DIR`, which
two working trees can share. That is not hypothetical - this repository has a private tree at `local/` that also
holds a `coop-rs` and a `tools/deploy_pak.py`, both older, and building from it writes a `coop_agent.dll` containing
the *previous* replication allowlist over the top of a current one. The audit now warns when the installed DLL is
older than the sources it was supposedly built from.

Usage
-----
    python tools/deploy_audit.py                     # audit this machine
    python tools/deploy_audit.py --client-share \\\\<client-pc>\\e   # and the other one
    python tools/deploy_audit.py --game <path>       # a different install
"""

import argparse
import os
import sys

DEFAULT_GAME = os.environ.get('COOP_GAME', r'E:\Steam\steamapps\common\F1 Manager 2024')
CLIENT_SHARE = os.environ.get('COOP_CLIENT_SHARE', r'\\<client-pc>\e')
CLIENT_GAME = os.path.join(CLIENT_SHARE, 'SteamLibrary', 'steamapps', 'common', 'F1 Manager 2024')

HERE = os.path.dirname(os.path.abspath(__file__))
PROJECT = os.path.dirname(HERE)


def newest_agent_source():
    """Modification time of the newest `.rs` under this project's `coop-rs/crates`."""
    newest = 0.0
    crates = os.path.join(PROJECT, 'coop-rs', 'crates')
    for root, _dirs, files in os.walk(crates):
        for f in files:
            if f.endswith('.rs'):
                try:
                    newest = max(newest, os.path.getmtime(os.path.join(root, f)))
                except OSError:
                    pass
    return newest


def audit(root, label):
    """Return (present, missing) lists for one install."""
    bin_dir = os.path.join(root, 'F1Manager24', 'Binaries', 'Win64')
    pak_dir = os.path.join(root, 'F1Manager24', 'Content', 'Paks', '~mods')

    # (name, path, why it matters)
    required = [
        ('version.dll', os.path.join(bin_dir, 'version.dll'),
         'auto-loader - without it the agent never enters the process'),
        ('coop_agent.dll', os.path.join(bin_dir, 'coop_agent.dll'),
         'the mod itself; version.dll loads it by bare name from this directory'),
        ('coop-link.exe', os.path.join(bin_dir, 'coop-link.exe'),
         'spawned by the agent on Host/Join to move car frames'),
        ('coop_proxy.enable', os.path.join(bin_dir, 'coop_proxy.enable'),
         'gates agent loading; absent = the forwarder is inert by design'),
        ('coop_hook.enable', os.path.join(bin_dir, 'coop_hook.enable'),
         'gates the sim hook; absent = Host reports "no hook installed"'),
        ('coop_mode.txt', os.path.join(bin_dir, 'coop_mode.txt'),
         'coop-auth mode; absent = every team rule passes'),
        ('coop_player.txt', os.path.join(bin_dir, 'coop_player.txt'),
         'this player\'s team/loadout/role; absent = coop-auth is inert'),
    ]
    present, missing = [], []
    for name, path, why in required:
        (present if os.path.isfile(path) else missing).append((name, why, path))
    # Present but wrong is the other half of the problem, and it is silent.
    stale = []

    paks = []
    if os.path.isdir(pak_dir):
        paks = [f for f in os.listdir(pak_dir) if f.lower().endswith('.pak') and 'coop' in f.lower()]
    if not paks:
        missing.append(('coop pak in ~mods',
                        'the multiplayer menu does not exist without it',
                        pak_dir))
    else:
        present.append(('coop pak in ~mods (%s)' % paks[0], '', pak_dir))

    print('=== %s ===' % label)
    print('  install: %s' % root)
    for name, _why, _path in present:
        print('  ok      %s' % name)
    for name, why, path in missing:
        print('  MISSING %s' % name)
        print('          %s' % why)
        print('          expected at %s' % path)

    # Report the mode rules, because "present but wrong" is the next failure after "absent".
    mode_file = os.path.join(bin_dir, 'coop_mode.txt')
    who_file = os.path.join(bin_dir, 'coop_player.txt')
    if os.path.isfile(mode_file) and os.path.isfile(who_file):
        try:
            mode = open(mode_file, encoding='utf-8').read().strip()
            who = open(who_file, encoding='utf-8').read().strip()
            print('  auth    mode=%s player=%r' % (mode, who))
            parts = who.split()
            if len(parts) >= 3 and parts[2] not in ('host', 'client'):
                print('          ! role must be "host" or "client"; coop-auth defaults to host silently')
        except OSError as e:
            print('  auth    unreadable: %s' % e)

    # Is the installed DLL newer than the sources it was built from?
    #
    # Absence is loud; staleness is not. A deploy copies whatever `CARGO_TARGET_DIR` holds, and two working trees
    # can point at the same one - this project has a private `local/` tree that does, complete with its own older
    # `tools/deploy_pak.py`. Building from the older tree overwrites a current `coop_agent.dll` with a previous one
    # and every file is still present, so the audit said READY while the machine ran older logic. This is the check
    # that distinguishes the two, and it is why "READY" now means "present *and* current".
    agent = os.path.join(bin_dir, 'coop_agent.dll')
    src_newest = newest_agent_source()
    if os.path.isfile(agent) and src_newest:
        built = os.path.getmtime(agent)
        if built < src_newest:
            stale.append(('coop_agent.dll is older than the sources',
                          'built %.0f min before the newest .rs - a deploy from a stale tree overwrote it'
                          % ((src_newest - built) / 60),
                          agent))
    for name, why, path in stale:
        print('  STALE   %s' % name)
        print('          %s' % why)
        print('          %s' % path)
    print()
    return missing + stale


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--game', default=DEFAULT_GAME)
    ap.add_argument('--client-share', default=CLIENT_SHARE)
    ap.add_argument('--host-only', action='store_true')
    # Exit 0 even when problems are found. The test runner treats a non-zero exit as failure regardless of what
    # the output says (`ok = returncode == 0 and predicate`), so a CI check of "does this audit work" needs a mode
    # that reports without failing. The default stays non-zero, because for a human "not ready" *is* a failure.
    ap.add_argument('--summary-only', action='store_true',
                    help='always exit 0; the verdict is in the output, not the status')
    args = ap.parse_args()

    total = 0
    if not os.path.isdir(args.game):
        print('SKIP: no install at %s' % args.game)
        return 0
    total += len(audit(args.game, 'host'))

    if not args.host_only:
        if os.path.isdir(args.client_share):
            if os.path.isdir(CLIENT_GAME):
                total += len(audit(CLIENT_GAME, 'client'))
            else:
                print('=== client ===')
                print('  share reachable at %s but no F1 Manager install under it' % args.client_share)
                print('  expected: %s' % CLIENT_GAME)
                total += 1
        else:
            print('=== client ===')
            print('  share NOT reachable at %s (it flaps; retry before concluding anything)' % args.client_share)
            total += 1

    # A shared team with the same team id on both machines means both players claim one team and coop-auth refuses
    # one of them - which presents as "player 2 cannot edit anything".
    if not args.host_only:
        hb = os.path.join(args.game, 'F1Manager24', 'Binaries', 'Win64', 'coop_player.txt')
        cb = os.path.join(CLIENT_GAME, 'F1Manager24', 'Binaries', 'Win64', 'coop_player.txt')
        if os.path.isfile(hb) and os.path.isfile(cb):
            try:
                ht = open(hb, encoding='utf-8').read().split()
                ct = open(cb, encoding='utf-8').read().split()
                if ht and ct and ht[0] == ct[0]:
                    print('WARNING: both machines declare team %s - they will fight over one team' % ht[0])
                    total += 1
                if ht and ct and len(ht) > 1 and len(ct) > 1 and ht[1] == ct[1]:
                    print('WARNING: both machines declare loadout %s - in shared-team they would own the same car'
                          % ht[1])
                    total += 1
            except OSError:
                pass

    print('READY' if total == 0 else '%d problem(s) - not ready' % total)
    if args.summary_only:
        return 0
    return 0 if total == 0 else 1


if __name__ == '__main__':
    sys.exit(main())
