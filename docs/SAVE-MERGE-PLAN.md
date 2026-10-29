# Plan: live save merge (client teams → host)

Status: **planning only**. Nothing here is implemented. Written down before the details are lost,
with what is verified marked apart from what is guessed.

The goal: several players each run their own team in the same save. Every day advance is a
synchronisation point. The host's world is authoritative; each client's decisions for its own team are
merged in, replacing whatever the AI did for that team.

## Why a merge is needed at all

The game is single-player and the AI runs every team it is not given. In a co-op save the client's team
exists on the host too, but the host's AI is driving it. The client's own game is simulating its own
season; its decisions live in its own save. Neither save is a superset of the other for the other's team,
so neither can simply overwrite the other. What is wanted is: host's world + client's team decisions.

## The hard constraint

Races must be won on the client's own game, not on the host. A client that wins on its own machine must
end up with the points. That means the merge has to carry **results**, not just inputs.

This is the part that decides feasibility, and it is unproven. Two candidate models:

| model | what the client sends | works if |
|---|---|---|
| **A: inputs only** | setups, strategy, driver orders, transfers, upgrades | the host's own simulation reproduces the same outcome from the same inputs |
| **B: results** | its race classification, points, prize money, standings | the host accepts externally supplied results for a team |

A is much simpler and keeps the host authoritative, but it cannot guarantee that a client's race result
is reproduced — F1 Manager's race simulation is deterministic given the same seed and inputs in
principle, but that is an assumption to *test*, not assume. B is what "races are won on the client"
actually implies, and it needs a write path into the standings that nothing here has yet.

**This table is the one thing to resolve first.** Everything else in this plan is cheap by comparison.

## Sync points

A day advance is the natural barrier because the game already stops there. The sequence:

```
day advance pressed
  -> barrier: every participant must reach the same day, or nothing proceeds
  -> exchange: each client sends its team's delta for that day
  -> host merges
  -> host publishes the new authoritative state
  -> next day
```

Rules that follow from the requirement as stated:

- **The barrier is by day, not by wall clock.** All participants must be on the *same day* before any
  merge is applied. A client that is behind must be caught up, not ignored, or its team silently
  misses a day of AI decisions.
- **The host's own team is never overwritten.** If the client is playing the host's team, nothing is
  applied — it is the same team, and the host already has the truth. This is not a special case to bolt
  on; it is the identity check `client_team != host_team` on every merge record.
- **One team, one writer.** If two clients send a delta for the same team something is wrong; refuse
  both rather than pick one, because a silent pick corrupts a player's season.
- **Merge is idempotent per (day, team).** Re-sending a delta for a day already merged must not
  double-apply points. This matters because the transfer is already designed to re-offer.

## Ownership record

Every mergeable field needs an owner, and the owner is a team id, not "the client". The merge is then
mechanical: for each record, if `owner != host_team`, take the client's version.

This is the same actor-ownership pattern the playbook describes for co-op sync, applied to save data
instead of actors. The existing `Msg::Hello { cars_owned }` is the wrong granularity already — it should
carry the team id.

## Where the data comes from

Two sources, and they are not interchangeable:

1. **The save file.** What is implemented today. Byte-exact, opaque, verified by hash. Fine for
   bootstrap, useless for merge: it cannot be edited, because it is frontier-compressed (7.99 bits/byte,
   no SQLite or zlib marker, `Oodle`/`zstd` static in the exe).
2. **The live DB in memory.** This is the direction requested, and it is the only one that can work for
   merge. If the database is live in the process as an SQLite-backed structure, the delta is a set of
   rows and the merge is SQL — no format authorship of the save at all.

So the merge feature is really a question about the live DB, and that is where the investigation should
go next: find the database object in memory, confirm it is a real SQLite connection or index, and
locate the tables that hold the mergeable state (standings, results, contracts, transfers, upgrades,
setups, strategy).

### What is already known that helps

- The object graph is reachable: `GUObjectArray` is locatable by content, and objects are addressable by
  class size. The RaceSimManager was found that way.
- There is an existing external reader (`coop-mem`) that can follow pointers without touching the game's
  threads, so exploring for a database object is cheap and safe.
- A save viewer already exists that turns saves into CSV/SQL for inspection, which means the *schema* is
  known from the file side even though the file itself is compressed. That schema is the shopping list
  for what a merge must cover.

### What is unknown, and must be measured

- whether the DB is SQLite in memory, or the DB is only on disk and memory holds structures.
- whether a merged row is noticed by the running game, or only at load.
- which table holds race results and whether they can be written without invalidating caches.
- whether the game recomputes standings from results, or stores standings directly (this decides whether
  a merge can write one place or many).

## Order of work

1. **Resolve A vs B** (inputs only vs results). Everything depends on it.
2. **Find the live DB** and prove a read of one mergeable table.
3. **Prove a write is observed** by the running game — one field, then read it back through the game's
   own UI if possible, not just through our reader.
4. **Define the day-delta record** and the mesh barrier on top of the existing relay.
5. **Merge with ownership rules**, refusing conflicts rather than resolving them.
6. **Only then** consider the bootstrap: sending a save is already proven (2.9 MB and 11.58 MB,
   byte-identical, 42–117 ms), and it stays as the way to get a client onto the host's world before the
   first merge.

## What is verified today, so it is not re-litigated later

- Save transfer over the link: **11 575 739 bytes in 84 ms**, SHA-256 identical on both sides. Also
  2 936 598 bytes in 42 ms. No manual file handling.
- The save is not editable: 7.99–8.00 bits/byte from 64 KB on, no SQL markers, no gzip, compressor
  static in the exe.
- The live 22-car snapshot channel works: 2488-byte frames at 20 Hz over the relay.
- Object discovery works without fixed addresses, by content, and survives a save load.
