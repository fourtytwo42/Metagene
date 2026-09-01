# Metagene

User and release documentation: [User Guide](docs/USER_GUIDE.md) · [0.2 Migration](docs/MIGRATION_0.2.md) · [0.2.0](docs/RELEASE_0.2.0.md) · [0.2.1](docs/RELEASE_0.2.1.md) · [0.2.2](docs/RELEASE_0.2.2.md) · [0.2.3](docs/RELEASE_0.2.3.md) · [0.3.0](docs/RELEASE_0.3.0.md) · [0.3.1](docs/RELEASE_0.3.1.md) · [0.3.2](docs/RELEASE_0.3.2.md) · [Release Process](docs/RELEASING.md) · [Third-Party Notices](THIRD_PARTY_NOTICES.md)

Metagene is a local-first, platform-agnostic Magic deck-evolution workbench. A Rust engine generates and evaluates legal deck candidates in parallel; a Tauri desktop UI selects Arena, paper, or Magic Online formats, configures runs, shows live progress, inspects candidates with card art, and exports deck text.

The current vertical slice includes:

- a format-agnostic integer deck genome with locked-card and copy-count constraints;
- a GUI focused-pool builder with legal-card search, card art, add/remove controls, imported-deck seeding, and hard per-card minimum/maximum counts;
- tournament-grounded blank-start evolution, explicit focused-pool exploration, and command-zone/Commander constraints;
- Rust-enforced Commander eligibility, color identity, singleton exceptions, and common partner/background/Doctor/Friends Forever pairings;
- Arena/paper/MTGO deck-list import with Commander, Deck, and Sideboard sections, printing-aware and Arena front-face DFC resolution, line-numbered validation errors, a start guard that prevents a failed paste from silently launching a blank genome, and persistent launch errors beside the Evolve control;
- strategy-preserving Tune for imported decks: an immutable baseline and typed copy/token, blink/ETB, sacrifice, graveyard, spellslinger, counter, role, curve, color, and threat signatures constrain evolution without hardcoding card names. Blank starts and explicit Rebuild runs remain unrestricted;
- catalog-v7 logical cards with one case-insensitive row per card name, compact aliases from every Scryfall printing ID, a catalog-derived mechanics inventory, and functional action profiles for draw, interaction, recursion, search, token/copy engines, sacrifice, ramp, crew, damage, mill, and alternate wins. Reprints cannot gain extra mutation probability, and an action contributes only when its zone/timing is reachable, its mana and nonmana costs are payable, and its expected target exists;
- first-class Best-of-Three sideboard genes: configure 0-30 cards, preserve or evolve the board, enforce shared copy limits, and round-trip it through MTGA export. The GUI can then evolve legal opponent-specific on-play/on-draw plans with actual weighted HexDek games, inspect exact ins/outs and alternatives, and rank candidates with separate pre-board, post-board, and full-match evidence;
- deterministic, CPU-parallel evolutionary runs;
- exact per-generation genome reuse: collision-safe grouping evaluates an identical mainboard/sideboard/command zone once, reports population slots separately from real evaluator calls and avoided duplicates, and keeps duplicate genomes from consuming Pareto/frontier slots;
- GUI-configurable package-aware mutation that preindexes mechanic and creature-type groups once per run, swaps correlated cards while preserving legality, and shows each candidate's strongest detected packages;
- common-random-number fitness trials for fair parent/child comparisons;
- user-configurable consistency, tempo, synergy, interaction, resilience, closing-power, archetype-role, Commander-quality, sideboard-quality, and mana-failure weights, with non-dominated Pareto layers preserved alongside the preferred structural screen ranking;
- a fast structural playability evaluator and a pluggable evaluator boundary for full game engines;
- live Tauri progress events, cancellation, candidate inspection, and MTGA export;
- repository-safe cache policy: catalogs, images, run databases, and downloaded engine assets belong in OS application-data directories, never Git.
- native on-demand card-art caching: the GUI fetches approved Scryfall image URLs through Rust, renders raw IPC bytes without exposing filesystem paths, reports live usage, and provides a one-click clear action;
- compact SQLite run history that reconstructs card pools from the catalog instead of duplicating them, retains the latest 200 runs, and reloads candidate results for inspection/export.
- restart-safe structural and hybrid checkpoints: orphaned active rows become visibly interrupted at desktop startup, and the GUI can continue structural evolution, validation, or the next completed-cycle boundary with the correct remaining budgets, deterministic seeds, champion provenance, and saved opponent-field revision;
- a GUI-managed HexDek adapter with isolated job directories, deterministic seeds, two-player constructed and 2-6 player Commander modes, CPU worker/AI-budget controls, canonical result-schema validation, matchup matrices, per-deck/per-position evidence, crash/draw accounting, and automatic temporary-file cleanup. A versioned opt-in fixed-order adapter makes Best-of-Three play/draw sequencing explicit while leaving ordinary rotating/random-start runs unchanged;
- a pinned GUI-managed Forge 2.0.14 adapter for lower-volume independent finalist checks, including constructed gauntlets, Commander pods, deterministic seeds, per-game clocks, cancellation, persisted evidence, and automatic temporary-file cleanup;
- an automatic version-aware matchup cache shared by HexDek and Forge: identical deterministic requests reuse only successful results, expose cached-game attribution in the GUI, and can be inspected or cleared from the engine panel;
- a format-aware gauntlet editor that legality-checks imported opponents and persists compact validation evidence. Tune promotion is game-only: common-seed HexDek selection, paired holdout, confidence/worst-matchup floors, and a Forge cross-check when fully supported decide whether the baseline is retained or replaced;
- a reusable versioned gauntlet library scoped by platform, format, and Commander/constructed mode: save, load, inspect, and delete field revisions from the GUI, with every load re-resolved and legality-checked against the current catalog;
- portable schema-1 metagame bundles with MTGA deck text, source/date/notes metadata, and explicit opponent shares; imports resolve current card IDs and legality, while constructed simulators distribute one fixed total game budget according to those shares;
- automated confidence-racing finalist ranking: every candidate starts on common seeds, a deck can stop early only when its blended 95% score ceiling falls below the best floor, the remaining field halves, survivor budgets double, crashes are penalized, and full brackets plus avoided game budgets are retained for inspection;
- confidence-aware selection with 95% Wilson intervals and GUI controls for statistical caution, worst-matchup weight, and native position-floor weight, using raw per-opponent and per-position wins/games rather than averaging away a bad pairing or seat artifact.
- GUI-triggered closed-loop evolution: structural generations produce a frontier, bounded parallel HexDek workers rank it, the validated winner becomes the next cycle's seed, and all cycle evidence remains inspectable and reloadable;
- cumulative completed-run continuation from any saved frontier candidate: the same run row, immutable Tune root, incumbent, generation/evaluation/time totals, title/notes/tags, validation evidence, and run-history identity remain attached while the deterministic seed advances through an additional user-selected generation budget;
- tournament-derived competitive fields: Spicerack is preferred, a user-keyed TopDeck adapter is second, and GetPaird is a visibly provisional fallback. Logical-card-normalized lists are deduplicated, clustered into archetypes, weighted by occurrences, and adaptively represented by 8–16 constructed decks or five Commander opponents with an inspectable field audit;
- immutable capability-aware route plans: every candidate/opponent pairing uses HexDek only when its critical actions are covered, falls back to Forge when Forge covers both decks, and blocks competitive claims rather than silently dropping unsupported field weight. Tune continuation automatically runs screen, coverage, field, holdout, and promotion stages;
- versioned combo intelligence: Commander Spellbook entries seed hypotheses, a bounded typed-state solver proves finite and resource-repeatable lines, deterministic engine observations promote or dispute proof, and only proven lines become safe HexDek action-selection priors. Combo packages participate in Tune retention and coherent package mutation;
- learned acceleration behind evidence gates: a 32-feature lineage- and field-isolated matchup ensemble activates after 5,000 trustworthy fully covered games. Burn provides CPU inference and optional bundled WGPU acceleration; GPU use requires measured 2x throughput, calibration, parity, and deck-quality-per-minute gates, while actual games remain mandatory for promotion;
- MAP-Elites quality-diversity search preserves the global best plus distinct archetype, color, curve, interaction, and victory-path niches. Typed-action, tournament-co-occurrence, validated-package, and combo relationships guide coherent substitutions without leaking opponent modules into strategy-preserving Tune runs;
- count-aware constructed initialization and mutation that favors traditional repeated-card spreads while retaining one-of exploration and leaving Commander/Brawl singleton behavior unchanged;
- a compact HexDek corpus index and format-aware coverage gate: the GUI identifies missing or partially parsed cards before simulation, rejects missing definitions, and can evolve from parser-complete, corpus-present, or unrestricted legal pools.
- a versioned conformance-evidence database and GUI importer that distinguishes single-engine scenario success, independent cross-engine agreement, failed scenarios, and conflicts for every card in the current candidate and gauntlet;
- reproducibility manifests on new conformance and matchup results, covering engine build, actual engine card-data revision, scenario schema where applicable, and a canonical complete-configuration hash;
- canonical schema-1 scenarios, shared assertion/outcome evaluation, event projection and hashing, plus production Forge and HexDek companions that apply exact state and actions. The first live copy/token/replacement scenario agrees across both engines down to its canonical event hash; broader critical-mechanics coverage is next.

The structural evaluator is intentionally a fast screening stage, not a complete implementation of Magic rules. Competitive claims require complete candidate coverage, at least 95% trusted field coverage, immutable engine routing, paired game evidence, and the configured holdout/cross-check thresholds. Results that cannot meet those requirements remain visibly provisional.

The rules preflight also reports semantic-risk hints independently of parser status. A parser-complete replacement, copy, control-changing, extra-turn, or Arena-digital card is highlighted as needing interaction-level parity evidence rather than silently receiving a conformance claim. Versioned schema-1 evidence bundles can be pasted into the desktop; compact normalized outcomes live in application data while raw engine logs and build trees remain disposable. Changed configuration or card data creates separate evidence, and migrated legacy rows remain readable but are explicitly marked as lacking complete provenance. See [`docs/CONFORMANCE.md`](docs/CONFORMANCE.md) for the bundle contract, evidence model, and current boundary.

When HexDek is installed, new workspaces default to the parser-complete evolution pool so CPU time is not spent producing finalists the game engine cannot parse. This is selectable in the GUI: `Present in HexDek` admits partially parsed cards with warnings, while `All format-legal` preserves unrestricted structural experimentation. Parser completeness only describes HexDek's Oracle parser and does not certify semantic correctness.

The focused evolution pool is independent of legality and simulator coverage. Adding cards restricts mutation to those names. A blank competitive build with no focused pool is grounded in a compatible tournament-field archetype; it never silently searches the whole catalog after field or engine failure. Focused names are persisted instead of printing IDs so saved runs survive catalog printing changes. Capacity warnings prevent starting a pool whose configured maxima cannot fill the required library, and fixed seed/locked cards excluded by hard focused-pool or exact-color settings produce an explicit error. Parser coverage and conformance exclusions remain mutation preferences: required imported/locked cards survive as narrow structural exceptions, while game validation still audits and blocks unsupported decks before spending engine budget.

Fitness preferences are configured per run in the GUI and saved with the run. Positive objective weights are normalized before scoring, while the mana-failure penalty remains an independent deduction. Candidate inspection shows the underlying measurements and a Pareto layer (`P1` is non-dominated), so a specialized deck is not hidden merely because the current weighted score favors a different tradeoff. The weights change preferences, not the measured evidence. Adaptive confidence racing is separately selectable: it computes lower and upper evidence bounds from overall, worst-opponent, and worst-position Wilson intervals, blends the fixed structural prior into both bounds, and never performs a statistical early drop while intervals overlap.

Long structural runs retain exact progress in memory, emit UI updates at most every 100 ms, keep at most 1,024 chart points, and checkpoint the visible champion after generation one, at most once per second, and at the final generation. If the desktop process exits while a run is active, startup changes that durable row from `running` to `interrupted` instead of pretending workers still exist. Resuming or extending a structural run reuses that row, carries forward cumulative counters and attached evidence, runs only the remaining/additional generation segment, and advances the deterministic seed so it does not replay the same mutation stream. This is champion-seeded continuation rather than an exact population/RNG-state snapshot; at most roughly one second of structural progress can be absent after an abrupt process loss.

Hybrid checkpoints atomically store their matching request and job snapshot at structural, validation, and cycle-complete boundaries. Recovery retains the immutable opponent-deck snapshot, normalized shares, originating saved-gauntlet ID/revision when available, current cycle, structural generation offset, cumulative unique evaluations and durable game attempts, active seed, frontier, and champion lineage. A validation-phase restart replays the deterministic halving request and reuses successful matchup-cache entries; it does not double-count an incomplete validation budget. Repeated interruptions continue from the newest segment rather than subtracting the same generations twice.

Mutation can mix independent count swaps with correlated package swaps. The package percentage and width are configured in the GUI and saved with the run; zero percent preserves the independent-only baseline. Package groups are deterministic and preindexed from nonland mechanic tags and creature subtypes, including token/copy, ramp, interaction, recursion, sacrifice, graveyard, discard, lifegain, counters, death, mill, blink, landfall, spellslinger, artifact-token, energy, Equipment/Aura, and typal themes. Existing catalog rows are enriched in memory, so adding this vocabulary does not require a bulk redownload. Candidate inspection reports the leading package name, unique-card count, and total copies instead of presenting synergy as an unexplained scalar.

On Windows x64, the release installer contains a checksum-verified HexDek runtime and rules corpus. Automatic first-run setup extracts it under the operating-system application-data `engines/hexdek` directory; it does not clone source, invoke Go/Python/Git, or leave compiler caches. Non-Windows source builds retain the managed source-install fallback.

Forge is a separate optional installation under application data. Metagene downloads the pinned 2.0.14 release archive, verifies its SHA-256 digest, extracts only the managed runtime, isolates Forge data/cache paths, and removes the archive and staging tree. Forge requires Java 17 or newer. Its stock AI-versus-AI output provides independent matchup evidence, but it does not expose the canonical event stream or starting-player breakdown required for event-level conformance and play/draw analysis; ordinary Forge games therefore never receive a cross-engine-conformance label or invented seat statistics. Metagene's bundled companion subscribes to Forge's public event bus, applies canonical exact-state fixtures, drives real spell abilities through the stack, and returns normalized evidence for shared assertions and hashing.

Commander is a first-class target. A run can hold a chosen commander fixed or evolve the commander/compatible pair and its 99-card library together. Finalists can be validated in real multiplayer pods from the same desktop workflow. HexDek is still an independent approximation of Magic rules, so crash-free output is evidence rather than proof of perfect card semantics.

Constructed finalists play every loaded gauntlet opponent in a separate head-to-head batch; Commander finalists instead play the selected 2-6 seat pod. HexDek rotates deck assignment by game index, and Metagene retains and displays its `wins_by_commander_by_seat` and `games_by_commander_by_seat` evidence under stable candidate/opponent labels. This keeps one high-throughput process per matchup while making seat-position artifacts visible. Per-opponent and per-position evidence is accumulated across halving rounds, so selection can blend overall, worst-matchup, and worst-position performance with their confidence bounds. The position weight is ignored when an engine supplies no native evidence, which keeps Forge explicitly neutral. The adapter hides a HexDek loader compatibility detail by labeling one distinct card per seat and adding one temporary extra copy; HexDek removes that copy before play, restoring the exact submitted library. Arena-only and rebalanced card behavior still needs card-by-card conformance auditing against the live client.

Closed-loop runs keep the focused mutation pool separate from the full legal validation catalog, so an opponent may use cards outside the archetype's search space. Finalist matchups execute through a bounded process pool (up to eight HexDek processes) while the configured CPU workers are divided among them. This amortizes corpus startup across concurrent candidates without oversubscribing the host. Stopping a run terminates active HexDek child processes, removes their isolated job directories, and retains the last completed cycle.

Opponent fields can be saved as immutable named revisions in `gauntlets.sqlite` under application data. The library is isolated by platform, format, and multiplayer mode, retains the latest 200 revisions globally, supports up to 32 constructed opponents or the five opponents allowed by a six-seat Commander pod, and stores compact card IDs rather than duplicating catalog records. Loading a revision fails clearly if a card disappeared or the deck is no longer legal after a catalog/format change. Schema-1 portable bundles instead carry MTGA text so they survive catalog-ID changes and can move between machines. Each opponent has a positive field weight plus optional source, observation window, and notes. For constructed HexDek, Forge, successive-halving, and closed-loop runs, weights divide the existing `games per opponent × opponent count` budget using exact largest-remainder allocation; increasing a deck's share reallocates games rather than silently increasing total work. Commander pod members remain one joint game and therefore do not use matchup shares.

Successful deterministic matchups are cached in application data by exact exported decks, simulator revision, seed, game and seat counts, and engine-specific limits. Crashed or incomplete reports are never cached. Retention is bounded to 10,000 entries and 256 MiB of report JSON with least-recently-used pruning; the GUI reports entry and disk usage and provides a one-click clear action. Cache failures are non-fatal and never prevent a fresh simulation.

Card art is downloaded only on demand from the HTTPS Scryfall image CDN and cached under application data using URL-derived keys and atomic writes. Individual responses are capped at 8 MiB, the least-recently-used cache is capped at 512 MiB, and clearing the cache invalidates in-flight writes so stale downloads cannot immediately repopulate it. The webview receives raw image bytes and creates temporary object URLs; it has no filesystem asset scope or direct remote-image permission.

## Development

Development prerequisites: Rust stable with the MSVC toolchain, Node.js, Microsoft C++ Build Tools, and WebView2. End users of the Windows installer do not need Rust, Node, Git, Go, or Python. The optional Forge cross-check needs Java 17+; its engine archive is downloaded prebuilt and checksum-verified automatically on first launch.

```powershell
npm install
npm run tauri dev
```

Verification and benchmarks:

```powershell
cargo test --workspace
npm run build
cargo run -p metagene-core --example throughput --release
cargo run -p metagene-core --example benchmark_deduplicated_population --release
cargo run -p metagene-core --example benchmark_package_mutation --release
cargo run -p metagene-app --example smoke_hexdek
cargo run -p metagene-app --example smoke_hexdek_constructed
cargo run -p metagene-app --example smoke_hexdek_best_of_three
cargo run -p metagene-app --example benchmark_hexdek_evolved
cargo run -p metagene-app --example benchmark_hexdek_halving
cargo run -p metagene-app --example audit_hexdek_coverage
cargo run -p metagene-app --example benchmark_filtered_frontier
cargo run -p metagene-app --example benchmark_sideboard_evolution --release
cargo run -p metagene-app --example benchmark_metagame_bundle --release
cargo run -p metagene-app --example benchmark_hybrid_loop
cargo run -p metagene-app --example benchmark_conformance_store
cargo run -p metagene-app --example smoke_hexdek_cancel
cargo run -p metagene-app --example install_forge
cargo run -p metagene-app --example smoke_forge
cargo run -p metagene-app --example smoke_forge_cancel
cargo run -p metagene-app --example smoke_art_cache
```

## Repository layout

- `crates/metagene-core`: deck model, constraints, deterministic evolution, evaluators, and MTGA export.
- `src-tauri`: native application shell, background job manager, and frontend commands/events.
- `src`: TypeScript user interface.
- `docs/ARCHITECTURE.md`: performance, storage, legality, and simulator-adapter design.
