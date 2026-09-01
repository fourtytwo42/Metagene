# Metagene Development Roadmap

This roadmap orders the remaining work by dependency and risk. Complete a stage's exit gate before treating the next stage as the primary focus. Small fixes may cross stage boundaries, but major feature work should follow this sequence.

## Goal And Checkpoint Protocol

The persistent project goal remains active until every checkbox and exit gate in this roadmap is complete and verified. Do not mark the goal complete because the application merely builds, one stage finishes, or a release candidate exists.

At every meaningful checkpoint and every stage transition, report:

1. **Completed:** Work and verification finished since the previous checkpoint.
2. **Next:** The single concrete item being worked on next.
3. **Left in the current stage:** Every unchecked item required by the active exit gate.
4. **Later stages:** The remaining stages in execution order.
5. **Blockers or risks:** Any condition that could prevent or invalidate completion.

Update this file's checkboxes and `MEMORY.md` at each stage transition. A stage is complete only when its exit gate has evidence, not when implementation first appears to work.

## Working Baseline

Metagene already has a working Rust/Tauri vertical slice: cross-platform card catalogs and legality, constructed and Commander genomes, structural evolution, card art, import/export, run history, HexDek and Forge adapters, saved weighted gauntlets, successive halving, and a hybrid evolve/validate/reseed loop.

The remaining job is to turn that capable alpha into a trustworthy, repeatable, distributable deck-evolution product.

## Stage 0 — Stabilize Hybrid Recovery

**Status:** Complete — 2026-08-29.

- [x] Finish persistence for the complete hybrid request and checkpoint state.
- [x] Resume correctly from structural evolution, validation, and cycle-complete boundaries.
- [x] Preserve cycle number, generation offset, gauntlet revision, cumulative game budget, seeds, and champion provenance.
- [x] Make repeated interruptions idempotent: no duplicated cycles, lost completed work, or double-counted games.
- [x] Ensure cancellation, process failure, and application restart produce clear recoverable states.
- [x] Run formatting, the complete Rust test suite, strict Clippy, and the frontend production build.
- [x] Perform live kill/restart tests during each hybrid phase and document the results.

**Exit gate:** An interrupted multi-cycle hybrid run can restart during every phase, finish with consistent budgets and provenance, and pass all automated and live recovery checks.

**Evidence:** Recovery metadata and job state now commit atomically. Subprocess tests terminate without shutdown handling in structural, validation, and cycle-complete phases, then reopen and reconcile the database idempotently. Pure recovery tests cover repeated structural interruptions and phase-specific budgets. The final gate passed 62 Rust tests, formatting, strict Clippy, the frontend production build, and a real two-cycle HexDek run with 320 structural evaluations, 4,096 game attempts, zero crashes, and a legal export in 26.904 seconds.

## Stage 1 — Establish Simulator Trust

**Goal:** Make it clear which results are reliable and prevent unsupported mechanics from silently influencing evolution.

**Status:** Complete — 2026-08-29.

- [x] Define a canonical, versioned scenario and event format for rules conformance tests.
- [x] Add an event-producing Forge adapter for independent comparison with HexDek.
- [x] Build scenarios for copy effects, token doublers, replacement effects, layers, targeting, triggers, state-based actions, multiplayer interactions, and Arena-only/rebalanced cards.
- [x] Compare normalized outcomes and canonical event hashes across engines.
- [x] Classify cards and mechanics as verified, partially covered, disputed, or unsupported.
- [x] Surface those classifications in pool selection, candidate inspection, and validation reports.
- [x] Allow users to exclude unsupported or disputed cards from evolution.
- [x] Version every result by engine build, card-data revision, scenario schema, and configuration.
- [x] Add regression tests for every simulator disagreement that is resolved.

**Exit gate:** Critical mechanics have repeatable conformance evidence, disagreements are visible, and no deck is presented as fully validated when its important cards lack semantic coverage.

**Evidence:** The three token/copy scenarios, targeted lethal damage/SBA, creature-ETB life trigger, unblocked combat damage, three-seat combat elimination, and rebalanced-card Flash timing agree across Forge and HexDek. `arena.rebalanced-falcon-abomination-has-flash@1` rejects the original Falcon Abomination off-turn and permits the rebalanced A-version at `sha256:3ab1d50217f6b174543caed2030569c4e1b55c4d021c7549fa8fec3b32ed0eb6`. `arena.oracle-of-the-alpha-conjures-power-nine@1` exposes HexDek's unsupported Conjure residual while Forge creates all nine cards. `layers.clone-copies-base-before-anthem@1` exposes HexDek's missing anthem layer. Both disputes remain visible, while the earlier copied-doubler adapter disagreement has an exact regression. Provenance schema 1 binds every new conformance and matchup result to engine revision, actual card data, scenario schema when applicable, and a canonical complete-configuration hash; legacy rows migrate as unknown and cannot merge with modern evidence. Live conformance, HexDek cold/cache, and Forge cold/cache probes retained their manifests. The final gate passed 74 app tests plus 19 core tests, formatting, strict Clippy, and the frontend production build with Cargo build concurrency bounded to one job after Windows' parallel linker exhausted PDB resources.

## Stage 2 — Implement Real Best-of-Three Evolution

**Goal:** Turn the existing sideboard genome into actual post-board match performance.

**Status:** Complete — 2026-08-29.

- [x] Represent opponent-specific sideboard plans or an evolvable sideboarding policy.
- [x] Validate legal swaps while preserving deck size and shared copy limits.
- [x] Run game one and post-board games with explicit play/draw sequencing.
- [x] Track pre-board, post-board, and full-match win rates separately.
- [x] Include sideboard decisions in cache keys and reproducibility manifests.
- [x] Evolve sideboards against weighted metagame opponents rather than only structural proxies.
- [x] Show the recommended ins/outs for each matchup in the GUI.

**Exit gate:** A Best-of-Three candidate is ranked using actual post-board games and has a legal, inspectable plan for every opponent in its gauntlet.

**Evidence:** Engine-neutral sideboard exchanges enforce equal in/out counts, source availability, zone sizes, full-deck copy limits, and one on-play/on-draw plan for every stable opponent hash. The evidence-driven policy search evaluates legal exchanges against weighted opponents with real fixed-order HexDek games, retains inspectable alternatives, and feeds the selected policies into explicit match sequencing where game-one play/draw alternates and the previous-game loser plays next. Reports and separate SQLite histories preserve pre-board, post-board, match, cache, policy, and complete configuration provenance. The GUI configures evolution, shows exact matchup ins/outs and alternatives, and runs the resulting Best-of-Three ranking. A live smoke completed cold and cached four-match runs, evaluated eight policies over sixteen games, exercised both candidate-on-draw and forced candidate-on-play post-board branches, and retained exact plans. The final gate passed 81 app tests plus 23 core tests (104 total), formatting, strict Clippy, and the frontend production build.

## Stage 3 — Automate Current Metagame Acquisition

**Goal:** Let users build a relevant opponent field without manually transcribing every deck.

**Status:** Complete — 2026-08-30.

- [x] Add source adapters for permitted public tournament and metagame data.
- [x] Record source URL, retrieval date, event/date range, sample size, and transformation notes.
- [x] Normalize archetype names and merge duplicate decklists without losing provenance.
- [x] Resolve every card against the selected platform and format before accepting a field.
- [x] Reject or clearly quarantine stale, illegal, ambiguous, and unresolvable lists.
- [x] Let users preview, edit, weight, save, and export the resulting portable bundle.
- [x] Support manual refresh and comparison with older field revisions.
- [x] Add fixtures so source-parser changes cannot silently corrupt fields.

**Exit gate:** A user can select a supported format, acquire a dated legal field, inspect its provenance and weights, and immediately use it in an evolution run.

**Evidence:** Three versioned adapters use only documented APIs: unauthenticated GetPaird public decks, Spicerack's public decklist database, and key-authenticated TopDeck tournament API v2 with required attribution. All responses share bounded HTTPS/time/size rules, platform/format catalog resolution, full legality validation, published or normalized editable archetype labels, canonical exact-list merging, occurrence weights, event-level provenance, and explicit quarantine. Synthetic fixtures cover duplicates, Commander command zones, missing lists, stale/future windows, wrong formats, malformed/ambiguous text, unknown cards, illegal decks, external-only URLs, required-field schema drift, and forward-compatible additions. The GUI selects a source/window, keeps keys request-only, previews provenance/weights/quarantine, loads the legal field immediately, edits labels and weights, refreshes, saves immutable revisions, compares revisions, and exports portable bundles with complete acquisition metadata. A live GetPaird Legacy smoke acquired 18 records, accepted 17 legal lists into 14 archetypes, quarantined one, evolved a fetched deck over four candidate evaluations, saved/diffed two eight-opponent revisions, exported and re-imported an 8,717-byte portable bundle, then removed its smoke revisions. The final gate passed 88 app tests plus 23 core tests (111 total), formatting, strict Clippy, and the frontend production build.

## Stage 4 — Deepen Commander Evaluation

**Goal:** Make Commander evolution measure multiplayer deck quality rather than only raw wins.

- [x] Replace special-case commander pairing logic with a data-driven compatibility policy.
- [x] Add conformance coverage for partner, background, Doctor's companion, Friends Forever, and other supported variants.
- [x] Track pod win share, seat performance, threat development, interaction, recovery, mana reliability, and ability to close games.
- [x] Separate two-player Commander evidence from multiplayer pod evidence.
- [x] Build configurable pod archetypes and repeatable 2–6 player seat rotations.
- [x] Penalize kingmaking, stalls, timeouts, and simulator exploits instead of treating them as useful results.
- [x] Explain that political behavior remains an AI limitation and expose diagnostics that reveal seat or pod bias.

**Exit gate:** Commander rankings use multiplayer-aware objectives, legal command zones, repeatable seat rotations, and transparent limitations.

**Evidence:** Command-zone legality uses versioned policy `commander-pairing-2026-08`, and 11 conformance fixtures cover positive and negative Partner, Partner With, Friends Forever, Choose a Background, and Doctor's companion cases. Multiplayer scoring uses versioned policy `commander-multiplayer-objectives-2026-08`, explicitly labels duel versus pod evidence, retains named pod archetypes, rounds direct and successive-halving budgets into reproducible complete 2–6 seat rotations, and exposes native pod wins, per-seat floor/spread, elimination order, closing speed, directed targets, and compact rules-engine threat/interaction/recovery/mana telemetry. Draws/stalls, crashes/timeouts, concessions, seat bias, and conservative kingmaking signals subtract from the objective; failed or known unsupported/disputed simulation paths are rejected or scored as losses. The GUI shows all sources, penalties, policy IDs, rotation details, and the political-AI warning. A live four-seat smoke produced four complete rotations plus an identical cache hit. A live 8→4→2 successive-halving gate scheduled 384 complete-rotation attempts, completed 381 games with three penalized crashes, evaluated 14 native Commander reports in 17.5 seconds (21.8 games/s), and selected a winner. The final gate passed 89 app tests plus 26 core tests (115 total), formatting, strict Clippy, and the frontend production build.

## Stage 5 — Improve Evolution Quality and Robustness

**Goal:** Find stronger decks without overfitting to one field, seed, or simulator quirk.

**Status:** Complete — 2026-08-30.

- [x] Add a Hall of Fame containing diverse earlier champions.
- [x] Test candidates against both the current field and historical champions.
- [x] Add archive diversity or novelty pressure so one archetype does not consume the frontier.
- [x] Support co-evolving adversaries as an optional advanced mode.
- [x] Improve mana-base mutation using color-demand and curve information.
- [x] Add package discovery from successful decks instead of relying only on hand-authored mechanic tags.
- [x] Calibrate confidence racing and successive-halving decisions against held-out games.
- [x] Add train/validation/holdout seeds and gauntlets to measure generalization.
- [x] Compare every optimizer change against fixed benchmark scenarios and wall-clock budgets.

**Exit gate:** New champions beat baselines on held-out opponents and seeds, retain meaningful diversity, and do not depend on a single simulator exploit.

**Evidence:** Policy `evaluation-splits-2026-08` assigns deterministic train/validation/holdout seed domains and stable 60/20/20 constructed opponent partitions; Commander and undersized fields disclose opponent reuse while retaining disjoint seeds. Structural evolution uses train only, halving selects on validation only, and winner plus runner-up run afterward on identical untouched holdout games. A bounded calibration store measures interval coverage, generalization error, and finalist-order agreement; after ten undercovered observations, subsequent hybrid runs automatically raise risk aversion and suspend statistical early elimination until coverage recovers. The persistent Hall of Fame deduplicates exact champions, selects historical opponents by evidence plus minimum deck distance, supports optional co-evolved runner-ups, and mines recurring packages from diverse winners. A fixed 1,280-evaluation operator benchmark improved held-out synthetic quality from 0.081 to 0.243 in 118 ms versus 77 ms under the same five-second cap. The live Historic HexDek gate used disjoint 3/1/1 fields, 160 structural evaluations, 256 validation attempts, and 704 holdout attempts: the selected champion scored 0.936 versus the fixed baseline's 0.330, two finalists remained active-diverse, 32 packages were discovered, finalist-order agreement was 1.000, and detected undercoverage correctly produced effective risk 0.850 with adaptive elimination disabled. The temporary proof databases were deleted. See `docs/EVOLUTION_ROBUSTNESS.md`.

## Stage 6 — Scale Throughput Intelligently

**Goal:** Spend the machine's CPU, memory, and GPU where they measurably improve deck quality per minute.

**Status:** Complete — 2026-08-30.

- [x] Profile the entire pipeline before optimizing it further.
- [x] Keep rules simulation CPU-parallel and use persistent engine workers to reduce startup cost.
- [x] Batch database writes, cache lookups, coverage checks, and deck serialization.
- [x] Add configurable CPU, memory, process, time, and disk budgets.
- [x] Explore a GPU-trained surrogate only after enough trustworthy labeled match data exists.
- [x] Use uncertainty-aware surrogate screening so uncertain decks still reach the real simulator.
- [x] Detect model drift and periodically retrain or discard stale surrogates.
- [x] Benchmark cold, warm-cache, constructed, Commander, Best-of-One, and Best-of-Three workloads separately.
- [x] Enforce retention limits for models, reports, temporary decks, logs, catalogs, and build artifacts.

**Exit gate:** Published local benchmarks report honest unique-candidate and completed-game rates, resource usage stays within configured limits, and acceleration does not reduce held-out deck quality.

**Evidence:** `docs/PERFORMANCE.md` publishes schema-versioned local measurements and exact commands. The measured pipeline separated catalog, coverage, structural, serialization, cache, engine, parse/store, process-start, and RSS costs. A bounded decoded-catalog cache reduced repeated legal-pool loading from 2,197 ms to 68 ms; the deterministic matchup cache reduced a 64-game BO1 replay from 2,567 ms to 28 ms with zero process starts. Structural evolution completed 249 unique evaluations in 28 ms. Cold BO3 completed four matches/eight games in 7.698 seconds and replayed warm in 47 ms. Commander 8→4→2 halving completed 384/384 games with zero crashes at 21.4 games/s. HexDek keeps CPU workers alive across each multi-game process batch; Metagene uses a bounded outer process pool and divides threads to prevent oversubscription. Upstream's available HTTP server was rejected as a persistence shortcut because it is stateful and does not provide a stateless resettable tournament protocol. GUI-persisted CPU, process, memory, run-time, and disk budgets gate hybrid work and prune only 24-hour orphan job directories. All databases/caches have documented bounded retention. The surrogate remains dormant below 500 trustworthy held-out labels, preserves uncertain candidates when enabled, and bypasses drifted models; GPU training remains evidence-gated because the current nine-feature workload is transfer-bound. The post-acceleration held-out regression remained 0.936 for the champion versus 0.330 for baseline over 704 attempts. The final gate passed 101 app tests plus 28 core tests (129 total), formatting, strict Clippy, frontend production build, and the live workload matrix.

## Stage 7 — Add Real-World Feedback

**Goal:** Connect simulator results to games the user actually plays.

- [x] Import supported Arena logs or user-supplied match histories without automating gameplay.
- [x] Associate results with exact exported deck revisions and match context.
- [x] Let users correct archetype labels, play/draw status, and sideboarding metadata.
- [x] Compare predicted and observed matchup performance.
- [x] Reweight objectives or opponent fields using user-approved evidence.
- [x] Keep personal match data local and provide deletion/export controls.

**Exit gate:** Arena results can be traced to exact deck revisions and used as optional, transparent evidence without silently contaminating simulator experiments.

**Evidence (2026-08-30):** `smoke_feedback` traced three Arena-shaped results to one immutable exported revision, quarantined a mismatched deck, produced a platform/format-scoped observed summary, raised a three-loss matchup only in a preview requiring explicit approval, round-tripped portable history, and deleted every local normalized row. Raw logs are never stored. The gate passed 106 application tests plus 28 core tests, strict workspace Clippy, formatting, and the frontend production build. See `docs/REAL_WORLD_FEEDBACK.md`.

## Stage 8 — Product UX and Release Hardening

**Goal:** Make the application understandable, recoverable, and safe to distribute.

- [x] Separate deck lab, card library, gauntlets, experiments, matchups, and settings into clear views.
- [x] Add run comparison, searchable history, notes, tags, and reproducibility manifests.
- [x] Improve candidate diffing, curve/mana charts, package explanations, and bulk export.
- [x] Add first-run setup, engine installation/repair, catalog refresh, and storage management flows.
- [x] Make errors actionable and preserve diagnostics without exposing private data.
- [x] Add database migrations, backup/restore, corruption recovery, and version compatibility tests.
- [x] Build automated end-to-end desktop tests for the primary workflows.
- [x] Produce a reproducible Windows installer or portable build.
- [x] Complete dependency licensing and attribution review.
- [x] Add signing and update delivery when release credentials and distribution method are chosen.
- [x] Write user documentation for constructed, Commander, simulation coverage, and interpreting confidence results.

**Exit gate:** A clean machine can install and operate Metagene, complete the primary workflows, recover from failure, and reproduce an exported experiment without developer intervention.

**Evidence (2026-08-30):** Six explicit views passed browser and automated Edge E2E navigation. Three E2E workflows cover view isolation, first-run/recovery/diagnostics/Arena feedback, and constructed-to-Commander selection; all backend workflows remain covered by 109 app plus 28 core tests and the prior live stage proofs. Local recovery VACUUMs consistent SQLite snapshots, caps ten archives, rejects future/unsafe manifests, verifies integrity before restore, creates a safety backup, and retains corrupt originals under quarantine. Run metadata migrates idempotently and schema-1 manifests contain the exact saved request/outcome. The locked NSIS build produced `Metagene_0.1.0_x64-setup.exe` (4,245,905 bytes, SHA-256 `552BC98C5482595FDE9848561C71514DEAECED63F4BE30DB40A5A92E27E15FC8`); an isolated silent install launched the installed app, silently uninstalled, and left no process or proof directory. `THIRD_PARTY_NOTICES.md` records the dependency review and separate MIT HexDek/GPL-3.0 Forge process boundary. Signing/updater delivery is correctly disabled because no release credentials or distribution endpoint have been chosen; `docs/RELEASING.md` defines the credential-gated activation and rollback policy. `docs/USER_GUIDE.md` covers constructed, Commander, evidence levels, confidence, privacy, and recovery. Final gates: 137 Rust tests, strict all-target/all-feature Clippy, formatting, frontend production build, and three Edge E2E tests.

## Recommended Execution Order

1. Stage 0: hybrid recovery
2. Stage 1: simulator trust
3. Stage 2: Best-of-Three
4. Stage 3: current metagames
5. Stage 4: Commander depth
6. Stage 5: evolution quality
7. Stage 6: throughput and optional GPU surrogate
8. Stage 7: real-world feedback
9. Stage 8: UX and release

Stage 3 and Stage 4 may proceed in parallel after Stage 1 if contributors are available. Stage 6 should not introduce a learned surrogate until Stage 1 has produced trustworthy training labels. Release-hardening work such as migrations and end-to-end tests should be maintained throughout, even though the final release gate is Stage 8.

## Definition of a Trustworthy Release

Metagene is ready to be called a trustworthy release when it can:

- Produce legal, exportable decks for the selected platform and format.
- Explain exactly which engine, field, seeds, rules coverage, and budgets produced a result.
- Resume long runs without corrupting or double-counting work.
- Distinguish structural estimates from rules-simulated and real-world evidence.
- Warn about unsupported mechanics instead of hiding uncertainty.
- Demonstrate improvement on held-out opponents rather than only its training gauntlet.
- Stay within user-configured CPU, memory, and disk limits.
- Install, update, back up, and recover without developer assistance.
