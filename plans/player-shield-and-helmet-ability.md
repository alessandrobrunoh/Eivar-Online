# Plan: Player Shield and Helmet Shield Ability

**Branch**: `feat/player-shield-helmet-ability`
**Status**: Active

## Goal

Add a pure, armor-independent shield pool to player stats and ship a helmet ability that grants 1,000 shield for 5 seconds before the shield expires server-side.

## Assumptions and product decisions to confirm

- Shield is represented as `current_shield` and `max_shield`, separate from health and armor.
- Incoming damage consumes shield first. Only damage that remains after shield absorption is passed through the existing armor mitigation formula and then applied to health.
- Shield absorption itself is pure: armor does not reduce or amplify the amount removed from the shield.
- The helmet ability is an instant self-targeted armor ability named **Aegis** (stable id to be agreed if the existing naming convention requires another name), granting 1,000 shield for 5 seconds.
- At expiration, any remaining temporary shield from Aegis is removed. The ability does not heal health.
- Recasting while an Aegis shield is active is rejected by the normal cooldown path unless the existing status/modifier semantics require refresh behavior; no stacking is introduced in this slice.
- The existing player stats endpoint and player/entity HUD should expose the shield values. Non-player shield presentation is intentionally out of scope unless the current replication path requires it.

## Acceptance Criteria

- [ ] Player stats expose shield values independently from health, armor, and mana.
- [ ] Damage consumes the shield before health; shield damage is not reduced by armor.
- [ ] Damage greater than the shield carries only the remainder into the existing armor mitigation calculation.
- [ ] Shield and health are clamped to valid ranges and never become negative.
- [ ] The helmet ability can be cast only from an equipped helmet that grants it, targets the caster, and grants exactly 1,000 shield for exactly 5 seconds.
- [ ] The shield expires authoritatively after 5 seconds, including when the player took no damage.
- [ ] The ability respects existing dead/casting/mana/cooldown validation and cannot be used by an unequipped helmet.
- [ ] Client replication and the gateway stats response expose the authoritative shield state without editing generated bindings by hand.
- [ ] Focused tests cover shield-only damage, overflow damage with armor, expiration, invalid casts, and the successful helmet cast.

## Slices

Every slice follows RED-GREEN-MUTATE-KILL MUTANTS-REFACTOR. No production code before a failing test. Before implementation, load the repository's TDD, testing, mutation-testing, and refactoring guidance and confirm the slice acceptance criteria.

### Slice 1: A player can receive and lose a pure shield before health

**Value**: Players get the core defensive behavior with deterministic combat semantics before any item or UI wiring is added.

**Path**: Shared domain stats/effect model -> authoritative module damage resolution -> replicated player/entity stats -> focused domain/module tests. The helmet trigger and presentation are intentionally deferred.

**Required implementation skills**: `tdd`, `testing`, `mutation-testing`, `refactoring`, plus Rust guidelines and the repository's gameplay architecture guidance.

**Acceptance criteria**: A player with shield takes shield-only damage without health loss; overflow damage applies only the post-shield remainder through armor; zero/negative and over-cap values are clamped; shield state is represented in the authoritative row/component path.

**RED**: Add failing tests for full absorption, zero shield, overflow with armor, shield clamping, and damage that exactly matches the remaining shield. Account for mutants that remove the shield-first branch, apply armor to shield damage, use the wrong subtraction order, or allow negative/over-cap values.

**GREEN**: Add the minimum shared shield fields and a single damage-resolution path used by the module; update row conversions and generated-binding inputs only through the supported generation workflow.

**MUTATE**: Run mutation testing for the new damage/stat logic and produce a report.

**KILL MUTANTS**: Strengthen boundary tests for exact equality, overflow, armor values, and expiration-independent shield state; ask before changing ambiguous stacking semantics.

**REFACTOR**: Assess whether the shield calculation belongs beside the existing shared stat formulas without introducing a one-use abstraction.

**Done when**: Core tests, workspace tests, and static analysis pass; mutation survivors are reviewed; human approves the slice commit.

### Slice 2: An equipped helmet can grant a timed 1,000-point Aegis shield

**Value**: A player can use the new defensive ability through the real armor-ability path and see the authoritative timed effect.

**Path**: Helmet catalog definition -> armor ability resolver/reducer -> shared effect/status representation -> module tick expiration -> player stats replication. Invalid source, dead caster, cooldown, mana, and recast behavior use existing validation paths.

**Required implementation skills**: `tdd`, `testing`, `mutation-testing`, `refactoring`, Rust guidelines, and the gameplay/effects architecture guidance.

**Acceptance criteria**: A helmet that exposes Aegis can cast it as a self-targeted instant ability; the resulting shield is exactly 1,000 and lasts 5 seconds; the module removes the temporary shield at expiry; an unequipped or non-ability helmet cannot cast it; the ability cannot target another entity and respects existing cooldown/resource/dead-state checks.

**RED**: Add failing content/registry tests for the ability metadata and item loadout, reducer tests for source and target validation, and tick tests for creation plus expiry. Account for mutants that grant the shield to the target, use 1,000 as a permanent max bonus, expire at the wrong boundary, skip cooldown/mana checks, or allow non-helmet sources.

**GREEN**: Define the minimal Aegis ability and connect it to the existing helmet ability loadout and effect scheduling. Reuse existing timed status/modifier infrastructure where it preserves shield ownership and expiration; do not add stacking or refresh behavior beyond the confirmed decision.

**MUTATE**: Run mutation testing over the ability resolver, shield application, and expiration logic.

**KILL MUTANTS**: Add tests for exact 5-second expiry, no-health-heal, self-target-only behavior, and rejected casts; resolve any surviving mutant that reflects an open product decision.

**REFACTOR**: Keep the implementation aligned with existing `armor_cast` and ability registry conventions; avoid a separate one-off helmet execution path.

**Done when**: The real reducer/tick path is covered, module build/tests pass, mutation report is reviewed, and human approves the slice commit.

### Slice 3: Players can observe shield state in stats and HUD

**Value**: Players and API consumers can understand how much protection remains and when it is gone.

**Path**: Authoritative `player_stats`/`entity_stats` row -> generated client bindings -> Bevy mirror/presentation -> gateway `/v1/characters/{id}/stats` response and OpenAPI schema. Generated files are regenerated, never hand-edited.

**Acceptance criteria**: The player HUD displays current/max shield distinctly from current/max health; the gateway stats response includes shield fields; shield changes and expiry arrive through the normal replication path; zero shield is represented consistently and does not hide or corrupt the health display.

**RED**: Add API serialization tests and presentation/component tests for non-zero, zero, full, and expired shield values. Account for mutants that display shield as health, omit zero, swap current/max, or fail to refresh after expiration.

**GREEN**: Thread shield fields through the existing response, mirror, and UI paths with the smallest visual treatment consistent with current health bars/stats.

**MUTATE**: Run mutation testing for response mapping and shield presentation logic where tooling supports those targets.

**KILL MUTANTS**: Add mapping and boundary assertions for current/max shield and expiry updates.

**REFACTOR**: Assess naming and formatting consistency with existing health/mana/armor stats; no unrelated UI redesign.

**Done when**: Client/gateway checks and the full workspace quality gate pass; module generation/build has been verified; mutation report is reviewed; human approves the slice commit.

## Pre-PR Quality Gate

1. Run focused domain/module/client/gateway tests for each changed path.
2. Run `cargo test --workspace`.
3. Run `cargo clippy --workspace --all-targets -- -D warnings`.
4. Build the module with `cd crates/stdb-module && spacetime build` (never `cargo build` for the module).
5. Regenerate SpacetimeDB bindings through the repository script and verify no generated file was hand-edited.
6. Run mutation testing and review survivors.
7. Perform a refactoring assessment; keep only changes that reduce risk or duplication.

---
*Delete this file when the feature is complete and all slices are merged.*
