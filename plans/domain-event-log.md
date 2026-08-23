# Plan: bounded domain event log

**Status**: Proposed

## Goal

Persist a bounded, server-only history of meaningful combat events so future
stats, inspect, and gateway features use one source of truth without logging
simulation frames.

## Acceptance criteria

- [ ] A resolved hit creates one `DamageDealt` row containing attacker, target,
      applied amount, and ability/source id.
- [ ] A death creates one `EntityDied` row, with killer when known, and player
      deaths are distinguishable.
- [ ] A completed cast creates one `SpellCast` row.
- [ ] A short fight produces rows proportional to resolved hits/deaths/casts,
      not physics/tick frames.
- [ ] The log is not public to clients and old rows are removed by a retention
      pass.
- [ ] Logging can be disabled without changing simulation results, and a
      repeatable tick benchmark records enabled/disabled timings.

## Slice 1: record and retain resolved combat events

**Value**: Gateway/admin and later inspect consumers can query meaningful
combat history without subscribing clients to the complete event stream.

**Path**: effect resolution -> `apply_damage`/death/cast completion -> private
append-only table -> tick retention pass. Client-facing transient event tables
remain unchanged.

**RED**: Add module tests or a deterministic benchmark fixture proving damage,
death, and cast resolution emit bounded rows, preserve attribution, skip
disabled logging, and delete expired rows.

**GREEN**: Add fixed-column `DomainEvent` rows, a logging toggle, shared emit
helpers, ability propagation through effect resolution, death attribution, and
retention in `game_tick`.

**MUTATE / KILL MUTANTS**: Verify tests fail if event kind, actor/target,
ability id, disabled guard, or retention cutoff is removed or inverted.

**REFACTOR**: Keep logging concerns isolated from simulation rules and preserve
the existing client VFX event schema.

**Done when**: module build/check passes, benchmark output documents enabled vs
disabled overhead, and all acceptance criteria are verified.
