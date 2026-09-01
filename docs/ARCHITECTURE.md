# Metagene architecture

## Goals

Metagene must search thousands of deck candidates quickly without confusing proxy scores with real Magic results. It therefore separates candidate generation, staged evaluation, and validation.

```text
Arena catalog -> constrained genome -> cheap screening -> matchup engine -> frontier
                         ^                                      |             |
                         +----------- selection/mutation -------+             v
                                                                    MTGA export
```

## Components

### Card catalog

The catalog imports Scryfall bulk data and records platform availability (`arena`, `paper`, and `mtgo`), format legality, Arena ID when present, canonical name, mana/type text, color identity, and art URLs in SQLite. The raw bulk download is temporary and is deleted after a successful transactional import. Images are downloaded lazily into a size-bounded OS cache. Neither belongs in the repository.

The selected value in `games` is the platform gate. A format's `legalities` value supplies the format gate. Platform and format are independent, so the same engine can build Arena Historic, paper Modern, Magic Online Pauper, or paper/online Commander decks without hard-coded card pools.

### Deck zones and Commander

Decks have explicit command-zone, mainboard, and sideboard zones. Commander configurations use a 99-card library plus one or two command-zone cards, singleton limits with basic-land/card-text exceptions, and commander color identity as a hard pool filter. Partner, Background, Doctor's companion, and similar multi-commander rules belong in a commander compatibility policy rather than UI special cases. Multiplayer Commander evaluation is a separate simulator profile because duel win rate is not an adequate objective.

### Evolution core

Mainboards and sideboards are separate dense vectors of small integer card counts aligned to one selected card pool. Dense vectors make mutation, hashing, and validation cheap. A run can retain an imported board unchanged or evolve a configured sideboard from blank. Bounds are recomputed against the other zone before every mutation, so the format copy limit applies to their combined count. The operators enforce:

- target main-deck size;
- target sideboard size when sideboard evolution is enabled;
- per-card min/max counts and locked anchors;
- configurable singleton/copy rules;
- only IDs from the selected Arena-legal pool.

Independent mutations are count swaps, so deck size stays constant without a repair allocation. A GUI-controlled fraction instead uses correlated package swaps: mechanic and creature-subtype groups are deterministically preindexed once per run, one group is selected, and 2-8 distinct eligible additions replace removable cards of the same land class. Bounds, locked minima, deck size, land limits, commander identity, singleton rules, and format pool membership therefore remain intact by construction. A zero-percent setting retains independent-only behavior for exploration and A/B comparison. Ranked candidates include their strongest package summaries so this operator remains inspectable rather than hidden behind the aggregate synergy score.

### Evaluation stages

1. **Structural screen:** millions of deterministic opening-hand/curve trials; rejects mana failures and nonfunctional candidates.
2. **Fast matchup engine:** many games against a fixed metagame gauntlet, with confidence-aware allocation, native per-game seat rotation, per-deck/per-position evidence, and separate constructed matchups or Commander pods. Constructed opponent weights divide a fixed total game budget by largest remainder, so field modeling changes sampling density without multiplying work. Ranking can independently weight the overall record, worst opponent, and worst native position using Wilson lower bounds. Optional confidence racing stops a candidate only when its blended 95% upper score is strictly below the best blended lower score; overlapping survivors receive the next doubled game budget. Reports preserve each score interval, confidence-drop ranks, maximum budget, scheduled budget, and avoided budget.
3. **Independent matchup check:** finalists are replayed at lower volume in pinned Forge 2.0.14. This can expose engine-specific ranking changes but does not itself prove event-level conformance.
4. **Arena validation:** a small frontier is exported for real play and optional result import.

The `Evaluator` trait keeps these stages interchangeable. Fitness carries named metrics as well as an aggregate score so the UI can show why a deck survived. The cheap sideboard objective rewards diverse mechanic coverage and efficient interaction/resilience, but it is explicitly structural: current HexDek ranking is game-one evidence unless an adapter reports a sideboarding policy. Forge retains sideboard cards in its deck export.

### Parallelism

Population members are evaluated with Rayon work stealing. Each candidate uses the same generation seed set, which reduces variance when comparing small mutations. Before dispatch, the engine groups exact mainboard, sideboard, and command-zone genomes; a hash locates possible matches, but full genome equality is required before fitness is reused, so collisions cannot merge different decks. Pareto ranking and the retained frontier operate on the same unique representatives, preventing redundant finalist games and avoiding quadratic ranking work on duplicate-heavy populations. Progress distinguishes historical population slots from actual unique evaluator calls, per-generation duplicates, and cumulative evaluations avoided. The default worker count leaves two logical cores free for the UI and operating system, but users can override it.

Rules simulation is branch-heavy and starts on CPU. A GPU backend is appropriate for large batched neural policy or surrogate inference, not for the initial irregular game-state executor. GPU results must remain advisory until checked by the rules engine.

### Persistence and disk budget

The repository contains source, small fixtures, and documentation only. Runtime data uses the Tauri application-data/cache locations:

- `catalog.sqlite`: compact cross-platform card and legality metadata;
- `runs.sqlite`: configurations, generation summaries, and retained frontier candidates;
- `gauntlets.sqlite`: up to 200 immutable opponent-field revisions scoped by platform, format, and multiplayer mode, including normalized weights and source/date/notes metadata;
- `matchup-cache.sqlite`: successful deterministic HexDek and Forge reports, capped at 10,000 entries and 256 MiB of report JSON;
- `images/`: on-demand Scryfall card-art cache with atomic writes, access-time LRU pruning, an 8 MiB per-image limit, and a 512 MiB ceiling;
- `engines/`: explicitly installed external simulator builds, versioned and prunable.

HexDek is the high-throughput primary matchup adapter. Its fixed tournament path rotates original deck assignment through seats with `game_index % seat_count`, then reports wins and games by deck and seat. Metagene preserves those counters instead of launching redundant reversed-order batches. Forge is an independently implemented Java rules engine used as a slower finalist sanity check. Both run in isolated temporary jobs and report engine identity, wins/games, draws or incomplete games, Wilson intervals, and matchup matrices. Forge's stock match chooses an opening-game starting player through its own game logic and does not include that choice in quiet summaries, so Metagene reports Forge seat evidence as unavailable rather than inferring it from argument order. Forge's managed archive is checksum-pinned and its mutable user/cache directories are separated from its versioned runtime.

Both adapters consult the same bounded SQLite cache before starting an external process. A key hashes the cache schema, engine identity and revision, exact exported decks, deterministic seed, game and seat counts, and every engine option that can change an outcome. Only zero-crash/zero-incomplete reports are stored. Cache hits retain the original evidence while separately reporting `cached_games`; engine updates or changed inputs naturally miss. LRU pruning enforces both entry and logical-byte caps, and cache errors degrade to an ordinary fresh run.

Per-game traces are sampled, compressed, and capped. Aggregate counters are retained; obsolete populations and raw bulk downloads are not.

Structural jobs checkpoint their best candidate and counters at generation one, every fifth generation, and completion. Since worker threads cannot survive a desktop-process exit, startup transactionally changes leftover `running` rows to `interrupted` while preserving their last status JSON. Recovery creates a new run linked through `resumed_from`, seeds it with the durable champion, subtracts completed generations from the requested budget, and derives a new deterministic seed. The compact database intentionally does not serialize entire populations or Rayon/RNG internals; recovery is therefore a bounded-loss champion restart, not bit-identical process continuation.

Hybrid recovery adds a phase-aware payload containing the full orchestration configuration, immutable opponent snapshot and weights, optional saved-gauntlet ID/revision, cycle counters, structural generation offset, deterministic seed state, cumulative evaluation/game budgets, frontier, and winner provenance. Request-side recovery metadata and the matching job status are written by one SQLite update so a crash cannot publish a validation phase without its frontier. Structural resumes run only the remaining segment; validation resumes reuse the persisted frontier and deterministic match cache; cycle-complete resumes begin the next cycle from the validated champion. Startup reconciliation is idempotent, and every resume creates a `resumed_from`-linked row rather than mutating history.

Card-image requests cross a narrow native boundary: Rust accepts only HTTPS `cards.scryfall.io` URLs, downloads outside the filesystem lock, atomically stores the result, and returns raw IPC bytes. The webview renders a temporary blob URL and has neither direct remote-image permission nor application-data filesystem access. Cache clearing advances an in-process epoch so downloads that began before the clear operation cannot write stale files afterward.

Gauntlet revisions contain only compact deck-zone card IDs, normalized weights, and metadata; they do not copy catalog rows or art. A load resolves every stored ID through the current legal platform/format pool, rechecks library and command-zone shape, copy limits, commander pairing, and color identity, then hands the validated decks to either simulator. Backward-compatible SQLite column migration gives older rows equal shares and empty metadata. For interchange, versioned JSON bundles carry portable MTGA deck text plus platform, format, multiplayer mode, per-opponent weight, source, observation window, and notes. Bundle import resolves all names against the current catalog and validates the entire field before changing GUI state.

## Correctness boundary

The structural evaluator measures consistency, curve, and tagged synergy. It does not claim rules fidelity or matchup win rate. Any full simulator adapter must report its engine version. Unsupported cards invalidate a HexDek matchup evaluation instead of receiving a guessed effect. Forge may reject unsupported deck contents during load, and its ordinary game summaries are stored only as matchup evidence because they do not contain Metagene's canonical scenario event stream.
