#!/usr/bin/env python3
"""Check that the client deploy pushes everything a client needs, and nothing it does not.

Why this exists
---------------
Two omissions here were found only by running the deploy against a scratch install and auditing the result:

  * `version.dll` was never pushed, so the client had the agent binary and nothing to load it.
  * the **pak** was never pushed, so the client had every DLL, a working HTTP endpoint, and no multiplayer menu -
    the UI is what exposes the mod, and every file a naive check looks for was present.

Both failures look identical from the outside ("the client does nothing") and neither errors anywhere. A static
check is the only way to catch them without deploying to a real machine, and the source is small enough that
reading the right strings is honest rather than fragile - if this breaks because the deploy was restructured, that
is a signal to re-read the deploy, which is the point.

Usage
-----
    python tools/deploy_push_test.py
"""

import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REMOTE = os.path.join(ROOT, 'tools', 'remote.py')

results = []


def ok(cond, name, detail=''):
    results.append(bool(cond))
    print('  %s  %s%s' % ('ok  ' if cond else 'FAIL', name, '' if cond else '  -- ' + str(detail)))


def main():
    if not os.path.exists(REMOTE):
        print('SKIP: tools/remote.py not found')
        return 0
    src = open(REMOTE, encoding='utf-8').read()

    at = src.find('def cmd_deploy_link')
    ok(at > 0, 'cmd_deploy_link is present')
    end = src.find('\ndef ', at + 1)
    body = src[at:end if end > at else len(src)]

    # Everything the client must receive, by the name it lands under.
    required = [
        ('coop-link.exe', 'the process that moves car frames'),
        ('coop_agent.dll', 'the mod itself; version.dll loads it by bare name'),
        ('version.dll', 'the auto-loader - without it nothing is loaded at all'),
        ('CoopBridge_P.pak', 'the UI; without it there is no multiplayer menu'),
        ('coop_proxy.enable', 'gates agent loading'),
        ('coop_hook.enable', 'gates the sim hook'),
        ('coop_mode.txt', 'coop-auth mode'),
        ('coop_player.txt', 'this player\'s team/loadout/role'),
    ]
    for name, why in required:
        ok(name in body, 'the client deploy pushes %s' % name, why)

    # And the things it must NOT do any more.
    ok('coop-inject.exe' not in body,
       'no longer pushes the manual injector',
       'coop-inject.exe is the ritual the version.dll forwarder replaced')
    ok('attach-client' not in body,
       'no longer tells the operator to attach by hand',
       'the agent loads with the game; an attach step reintroduces the failure the forwarder removed')

    # The proxy must be checked against the CLIENT's exe, not the host's: the export/import contract is per-binary.
    ok(re.search(r'check_proxy\.py.*F1Manager24\.exe', body, re.S) is not None,
       'verifies the proxy against the client exe before pushing it',
       'a proxy whose exports do not match stops the game launching')

    # The two machines are different players. Copying the host's identity to the client makes both claim the same
    # team, and coop-auth then refuses one of them - which presents as "player 2 cannot edit anything".
    ok('CLIENT_TEAM' in body and 'CLIENT_LOADOUT' in body,
       'writes the client its own team and loadout, not the host\'s',
       'identical team ids make the two machines fight over one team')

    # Both deploy scripts take their binaries from `CARGO_TARGET_DIR`, which two working trees can share - this
    # repository has a private `local/` tree that does, with its own older `coop-rs`. Building from the older tree
    # writes a previous `coop_agent.dll` over a current one and every file is still present, so an audit that only
    # checks for presence says READY while the machine runs older logic. The deploy must therefore copy from a
    # build made from *these* sources, which it cannot verify - so the audit checks the mtime instead, and this
    # asserts that check exists rather than trusting that someone remembers why it is there.
    audit = os.path.join(ROOT, 'tools', 'deploy_audit.py')
    if os.path.exists(audit):
        asrc = open(audit, encoding='utf-8').read()
        ok('newest_agent_source' in asrc and 'getmtime' in asrc,
           'the deploy audit notices a coop_agent.dll older than the sources',
           'present-but-stale is silent; without this the audit reports READY for an old build')
        ok('STALE' in asrc,
           'and says so under a name that is not "MISSING"',
           'a stale file is not a missing one, and conflating them sends the reader looking for the wrong problem')

    failed = results.count(False)
    print('\n  %d/%d passed' % (len(results) - failed, len(results)))
    if failed:
        print('  %d failed' % failed)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
