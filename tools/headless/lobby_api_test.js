'use strict';
/*
 * lobby_api_test.js - drives the agent's Steam-lobby HTTP endpoints over a real socket.
 *
 * The lobby cannot be tested by unit tests alone: the interesting behaviour is what the *endpoint* answers when
 * Steam is not there, because that is the case on a machine without Steam running, and a host tab that says
 * "waiting" while nothing is advertised is the confusing failure this must not produce.
 *
 * This spawns a tiny HTTP server that serves the agent's own handlers... it cannot: the handlers live inside a
 * DLL that needs the game. So instead this checks the two things that CAN be checked here:
 *
 *   1. the routes and their JSON shapes, against the documented contract (a golden check on the source), and
 *   2. the decision logic, by re-implementing nothing - it requires the Rust tests to cover the policy, and
 *      asserts here only that the bridge calls the endpoints with the right verbs and parameters.
 *
 * Being explicit about that boundary is the point: a test that pretended to exercise Steam would be a lie.
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const AGENT = path.join(ROOT, 'coop-rs', 'crates', 'coop-agent', 'src', 'lib.rs');
const STEAM = path.join(ROOT, 'coop-rs', 'crates', 'coop-agent', 'src', 'lobby_steam.rs');
const POLICY = path.join(ROOT, 'coop-rs', 'crates', 'coop-agent', 'src', 'lobby_policy.rs');
const BRIDGE = path.join(ROOT, 'pak', 'f1manager24', 'Content', 'UIGameface', 'js', 'coop', 'bridge.js');
const LOBBY = path.join(ROOT, 'pak', 'f1manager24', 'Content', 'UIGameface', 'js', 'coop', 'lobby.js');

const results = [];
function ok(cond, name, detail) {
  results.push({ ok: !!cond, name });
  console.log(`  ${cond ? 'ok  ' : 'FAIL'}  ${name}${cond ? '' : '  -- ' + (detail === undefined ? '' : detail)}`);
}

const agent = fs.readFileSync(AGENT, 'utf8');
const steam = fs.readFileSync(STEAM, 'utf8');
const policy = fs.readFileSync(POLICY, 'utf8');
const bridge = fs.readFileSync(BRIDGE, 'utf8');

console.log('lobby_api: the Steam lobby endpoints a host tab and a friend both need\n');

// ---------------------------------------------------------------------------------------------
// The routes exist, with the verbs the bridge uses
// ---------------------------------------------------------------------------------------------

console.log('routes');

ok(/\(\"GET\", \"\/coop\/lobby\"\)/.test(agent), 'GET /coop/lobby reports the session');
ok(/p\.starts_with\(\"\/coop\/lobby\/set\"\)/.test(agent), 'POST /coop/lobby/set drives the host tab');
ok(/p\.starts_with\(\"\/coop\/lobby\/join\"\)/.test(agent), 'POST /coop/lobby/join joins a friend');
ok(/\(\"GET\", \"\/coop\/connectlobby\"\)/.test(agent), 'GET /coop/connectlobby reports +connect_lobby');

/* Both verbs on the mutating endpoints: the caller is XMLHttpRequest inside the game's web view, where a
   preflight is one more thing to get wrong and the requests are idempotent reports, not resource commands.
   The coordination endpoint already made this choice; these must not diverge. */
ok(/starts_with\(\"\/coop\/lobby\/set\"\) && \(m == \"POST\" \|\| m == \"GET\"\)/.test(agent),
  'the host-tab endpoint accepts GET as well as POST (no preflight in the game web view)');
ok(/starts_with\(\"\/coop\/lobby\/join\"\) && \(m == \"POST\" \|\| m == \"GET\"\)/.test(agent),
  'the join endpoint accepts GET as well as POST');

// ---------------------------------------------------------------------------------------------
// Honest degradation
// ---------------------------------------------------------------------------------------------

console.log('\nno Steam: reported, not silently fine');

/* A host tab that says "waiting" while nothing is advertised is the failure mode that wastes a session. The
   handler must surface the attach error instead of reporting success. */
ok(/Err\(e\) => format!\(\"\{\{\\\"error\\\":\\\"\{e\}\\\",\\\"state\\\":\\\"unavailable\\\"\}\}\"\)/.test(agent),
  'a failed attach answers with the error and state=unavailable',
  'the host tab must be able to tell "hosting" from "wanted to host but could not"');

ok(/only available on Windows/.test(steam), 'the non-Windows build reports Steam unavailable rather than pretending');

/* The report distinguishes intent from reality. Without both, id=0 is indistinguishable from "not hosting". */
ok(/state=waiting id=/.test(agent) || /state=\{want:\?\} id=\{lobby\}/.test(agent),
  'the log line carries both the wanted state and the actual lobby id');

// ---------------------------------------------------------------------------------------------
// The join decision refuses before connecting
// ---------------------------------------------------------------------------------------------

console.log('\njoining refuses before connecting');

ok(/accept_friend\(&session\)/.test(agent),
  'the join path runs the policy check before JoinLobby');
ok(agent.indexOf('accept_friend(&session)') < agent.indexOf('lobby_steam::join(&api, id)'),
  'the check happens before the join call, not after');

// The policy itself is Rust-tested; assert the four outcomes exist and are distinct, which is what the
// endpoint's JSON error text depends on.
for (const name of ['NotOurs', 'VersionMismatch', 'WrongApp', 'NoLobby']) {
  ok(policy.includes(`Reject::${name}`) || policy.includes(`${name} =>`), `${name} is a defined refusal`);
}

// ---------------------------------------------------------------------------------------------
// The Steam FFI is pinned to header-derived signatures
// ---------------------------------------------------------------------------------------------

console.log('\nFFI signatures are the ones in the headers');

/* These three are the ones a reasonable person would get wrong by analogy with the class methods, and each
   error would read a return value at the wrong width. Pinning them in the source is cheap insurance. */
ok(/type GetLobbyDataFn = unsafe extern "system" fn\([^)]*\) -> \*const u8/.test(steam),
  'GetLobbyData returns const char* (not bool)');
ok(/type SetLobbyDataFn = unsafe extern "system" fn\([^)]*\) -> u8/.test(steam),
  'SetLobbyData returns bool');
ok(/type CreateLobbyFn = unsafe extern "system" fn\([^)]*\) -> SteamAPICall/.test(steam),
  'CreateLobby returns SteamAPICall_t (not bool)');
ok(/type JoinLobbyFn = unsafe extern "system" fn\([^)]*\) -> SteamAPICall/.test(steam),
  'JoinLobby returns SteamAPICall_t (not bool)');

// ---------------------------------------------------------------------------------------------
// The bridge drives the host tab
// ---------------------------------------------------------------------------------------------

console.log('\nthe host tab opens and closes the lobby');

/* The host tab is a lobby.js screen, not a bridge command: `Screen.prototype.host` is what runs when a player
   picks "host", so the lobby's lifetime belongs there. Checking bridge.js found nothing - which is how this
   assertion first failed, and why it now names the right file. */
{
  const lobbySrc = fs.readFileSync(LOBBY, 'utf8');
  ok(/lobby\/set\?state=waiting/.test(lobbySrc), 'the host tab opens the lobby when it is chosen');
  ok(/lobby\/set\?state=idle/.test(lobbySrc), 'leaving the host tab closes it again');
  ok(/lobby\/join/.test(lobbySrc), 'the client asks Steam to join');
  const hostFn = lobbySrc.slice(lobbySrc.indexOf('Screen.prototype.host'));
  const body = hostFn.slice(0, hostFn.indexOf('Screen.prototype.join'));
  /* The call must be `agent(...).catch(...)`, not `await agent(...)`: awaiting it would put the lobby's
     round trip in front of the sim-armed status, so a Steam failure would delay or replace the message the
     player actually needs. */
  ok(/agent\("lobby\/set\?state=waiting", "POST"\)\.catch\(/.test(body),
    'the lobby call is fire-and-forget so a Steam error is not reported as "cannot host"',
    body.split('\n').filter((l) => /lobby\/set/.test(l)).join(' | '));
}

/* The invite path lives in the bridge because it must run on boot, before any screen: Steam launches the game
   with a lobby id, and nothing else would read it. */
ok(/connectFromInvite\(\)/.test(bridge), 'the bridge checks for a launch-from-invite on boot');
ok(/coop\/connectlobby/.test(bridge), 'and asks the agent, which parses its own command line');

// ---------------------------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------------------------

const failed = results.filter((r) => !r.ok);
console.log(`\nlobby_api: ${results.length - failed.length}/${results.length} passed`);
if (failed.length) {
  console.log('failures:');
  for (const f of failed) console.log('  - ' + f.name);
  process.exit(1);
}
