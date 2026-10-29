'use strict';
/*
 * team_claim_test.js - one player must not be able to take every team.
 *
 * The reported symptom: "player 1 being able to take every team and locking them". The cause was a single
 * comparison in `pickTeam`: it released the previous claim by testing
 *
 *     if (k !== this.state.myTeam) next[k] = this.state.takenBy[k];
 *
 * where `k` is an object key (always a string) and `myTeam` is a number. `"1" !== 1` is true for every key, so
 * nothing was ever released and each click added a team. After ten clicks the host held all ten, and the client
 * was refused every one of them.
 *
 * This extracts the real function body out of `lobby.js` and runs it, rather than re-implementing the logic -
 * a copy would keep passing after the source regressed, which is the failure this is meant to catch.
 */

const fs = require('fs');
const path = require('path');

const LOBBY = path.join(__dirname, '..', '..', 'pak', 'f1manager24', 'Content', 'UIGameface', 'js', 'coop', 'lobby.js');
const src = fs.readFileSync(LOBBY, 'utf8');

const results = [];
function ok(cond, name, detail) {
  results.push({ ok: !!cond, name });
  console.log(`  ${cond ? 'ok  ' : 'FAIL'}  ${name}${cond ? '' : '  -- ' + (detail === undefined ? '' : detail)}`);
}

/**
 * Pull the body of `Screen.prototype.pickTeam` out of the source and build a callable from it.
 *
 * The function is self-contained apart from `lsSet` and `this`, both of which are supplied, so evaluating it as
 * a plain function keeps it honest: if the source changes shape this throws instead of silently passing.
 */
function loadPickTeam() {
  const start = src.indexOf('Screen.prototype.pickTeam = function');
  if (start < 0) throw new Error('pickTeam not found in lobby.js');
  // Walk braces from the opening `{` of the function body.
  const braceAt = src.indexOf('{', src.indexOf(')', start));
  let depth = 0;
  let end = -1;
  for (let i = braceAt; i < src.length; i++) {
    if (src[i] === '{') depth++;
    else if (src[i] === '}') {
      depth--;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  if (end < 0) throw new Error('could not find the end of pickTeam');
  const body = src.slice(braceAt + 1, end);

  // `lsSet(KEY_TEAM, id)` is the only external call, plus the `KEY_TEAM` constant and `agent(...)`.
  const factory = new Function(
    'lsSet',
    'KEY_TEAM',
    'agent',
    'return function pickTeam(id) {\n' + body + '\n};'
  );
  return factory(function () {}, 'coop.team', function () {
    return { then: () => ({ catch: () => {} }) };
  });
}

console.log('team_claim: one player cannot take every team\n');

let pick;
try {
  pick = loadPickTeam();
  ok(true, 'pickTeam was extracted from lobby.js');
} catch (e) {
  ok(false, 'pickTeam was extracted from lobby.js', e.message);
  console.log(`\nteam_claim: 0 passed, 1 failed`);
  process.exit(1);
}

// A stand-in for the component instance: `pickTeam` only touches `state`, `setState`, and `state.myTeam`.
function makeScreen(role) {
  return {
    state: { role: role, myTeam: 0, takenBy: {} },
    setState(patch) {
      Object.assign(this.state, patch);
    },
    // The agent call is fire-and-forget in the source; stub it out so no network is attempted.
    agent() {
      return { then: () => ({ catch: () => {} }) };
    },
  };
}

// ---- 1. the reported bug --------------------------------------------------------------
{
  const host = makeScreen('host');
  for (let t = 1; t <= 10; t++) pick.call(host, t);
  const held = Object.keys(host.state.takenBy);
  ok(held.length === 1, 'a host that clicks all ten teams ends up holding exactly one', JSON.stringify(host.state.takenBy));
  ok(held[0] === '10', 'and it is the last team clicked', held.join(','));
  ok(host.state.myTeam === 10, 'the selected team follows the last click', String(host.state.myTeam));
}

// ---- 2. the other player is not locked out --------------------------------------------
{
  const host = makeScreen('host');
  pick.call(host, 1);
  const client = makeScreen('client');
  // The client sees the host's claim only once it has been fetched; set it the way `claimsToMap` would.
  client.state.takenBy = Object.assign({}, host.state.takenBy);
  // Team 1 is the host's, so a client pick must be refused and change nothing.
  pick.call(client, 1);
  ok(client.state.myTeam === 0, 'the client cannot take the team the host holds', String(client.state.myTeam));
  // Any other team is free.
  pick.call(client, 2);
  ok(client.state.myTeam === 2, 'the client can take a free team', String(client.state.myTeam));
  ok(host.state.takenBy['1'] === 'host', "the host's claim is untouched", JSON.stringify(host.state.takenBy));
  ok(client.state.takenBy['2'] === 'client', 'the client holds its own team', JSON.stringify(client.state.takenBy));
}

// ---- 3. both players may hold one team each at the same time -------------------------
{
  const host = makeScreen('host');
  pick.call(host, 4);
  const client = makeScreen('client');
  client.state.takenBy = Object.assign({}, host.state.takenBy);
  pick.call(client, 7);
  const all = Object.assign({}, host.state.takenBy, client.state.takenBy);
  ok(all['4'] === 'host' || all['4'] === 'client', 'team 4 is claimed', JSON.stringify(all));
  ok(all['7'] === 'client', 'team 7 belongs to the client', JSON.stringify(all));
  const roles = Object.values(all).sort();
  ok(roles.length === 2, 'exactly two teams are claimed', JSON.stringify(all));
}

// ---- 4. changing team releases the old one, whichever role you are -------------------
{
  const client = makeScreen('client');
  pick.call(client, 3);
  pick.call(client, 8);
  const held = Object.keys(client.state.takenBy);
  ok(held.length === 1 && held[0] === '8', 'changing team releases the previous one', JSON.stringify(client.state.takenBy));
}

const failed = results.filter((r) => !r.ok).length;
// The summary format is load-bearing: `run_all_tests.py` parses `N passed, M failed` (or `N/M passed`) to
// decide whether a suite passed, so an unparseable line makes a green suite report as a failure.
console.log(`\nteam_claim: ${results.length - failed} passed, ${failed} failed`);
process.exit(failed === 0 ? 0 : 1);
