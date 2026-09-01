# Metagene User Guide

## First run

Launch Metagene and let automatic first-run setup finish. It downloads/indexes the current card catalog, extracts the bundled Windows HexDek runtime without a compiler, and downloads the checksum-pinned prebuilt Forge cross-check when Java 17+ is available. The normal deck defaults appear immediately but remain clearly disabled until the catalog creates a legal workspace. Settings retains repair/refresh buttons for recovery. Set CPU, process, memory, run-time, and disk limits before a long run.

All catalogs, engines, art, simulations, histories, and backups live in the operating-system application-data directory, not in the repository. Metagene does not require an account or cloud service.

## Views

- **Build** is the single creation workflow. Its compact state shows platform, format, exact deck colors, search depth, starting deck, and one launch action. Advanced optimizer controls, focused pools, Commander setup, imports, and competitive validation expand only when needed.
- **Results** combines the current frontier and deck inspector with collapsible run history, Hall of Fame, and real-world match feedback. Saved experiments and matchup evidence no longer occupy duplicate destinations.
- **Card Library** searches only cards legal for the selected platform/format, shows art and conformance state, and adds cards to a focused mutation pool.
- **Settings** is the only system-management destination for engines, catalog, resources, caches, backups, integrity checks, repair, and privacy-safe diagnostics.

## Constructed workflow

1. Select Arena, paper, or Magic Online and a constructed format.
2. Choose **Auto** colors or an exact W/U/B/R/G/C palette. Select only **C** for a zero-color deck; select one color for mono-color; combine colors freely, including combinations such as U+C. An exact selection constrains every initialized and mutated card rather than merely adding a score preference.
3. Import a deck or leave the genome blank. Imported decks default to **Tune this strategy**; blank starts default to **Rebuild**. A blank competitive build automatically acquires or reuses a tournament field and starts from its highest-weight compatible legal archetype. It stops clearly if no compatible field and rules-engine route exists instead of returning an ungrounded all-catalog pile. Set sideboard size/evolution for Best-of-Three. Open Advanced optimizer controls only when the Quick, Balanced, or Deep preset is not enough.
4. Choose legal-pool and conformance exclusions. Unsupported/disputed cards are visible; parser coverage is not treated as rules correctness.
5. Run Tune or Rebuild for fast structural screening. Catalog v7 stores one logical card per case-insensitive name, inventories mechanics per mode and action, chooses initial card modes, matches each physical source to the exact spell it may pay for, and evaluates typed actions only when their costs, zones, timing, and targets work. Screen score is not a competitive claim. Inspect dedicated/modal lands, unrestricted/conditional/unusable W/U/B/R/G/C sources, per-card castability, dead modes, role/closing evidence, copy rationale, legendary dead draws, and fulfilled requirement/supply ratios.
6. To keep searching from a completed result, select a frontier candidate, set **Generations** to the additional budget, and click **Continue +N generations**. The selected deck stays pinned as the incumbent, the same saved run advances cumulatively, and its original Tune root, completed generations/evaluations, validation evidence, title, notes, tags, and history remain attached.
7. Expand **Competitive validation** in Build, acquire or load a dated tournament field, and validate finalists with confidence racing. Metagene prefers Spicerack, uses TopDeck when you provide a request-only key, and falls back to GetPaird with a permanent Provisional label. Best-of-Three separately reports pre-board, post-board, and full-match evidence.
8. Copy one deck or bulk-copy the top eight. Arena exports register immutable revision identity for later match feedback.

Tune automatically validates the pinned baseline and up to seven genuinely diverse finalists. It reuses a compatible field acquired within seven days or requests the most recent 30-day tournament field. The Field Audit exposes source, event window, occurrence weight, archetype cluster, merge decisions, represented weight, and every quarantine reason. If no field or fully covering simulator route is available, the run still finishes but is labeled **Provisional** and never claims improvement. A valid diverse frontier may contain fewer than eight decks.

Constructed blank starts and mutations use a count-aware prior: repeated copies of active cards are favored while one-of discoveries remain possible. This produces familiar 4/3/2/1 spreads without requiring a fixed copy count. Commander and Brawl bypass this prior and preserve singleton construction rules.

For intentional offline experimentation, choose a focused pool under Advanced. Exact colors constrain both tournament seed compatibility and mutations: if the current field has no legal archetype in the selected colors, import a seed or provide a focused pool rather than weakening the competitive grounding rule.

## 0.3.2 closed-loop quality migration

Catalog model v11 and structural revision `closed-loop-quality-v11` rebuild automatically while preserving application data. Nonland faces without printed mana costs are no longer treated as free `{0}` spells. Reminder-text Treasure abilities, triggered mana rewards, trigger conditions, duplicate clause summaries, and unmet conditional activations no longer masquerade as functional effects or unconditional sources. Shared basic-search targets, narrow mainboard copies, excess copies, and turn-by-turn closing pressure now affect screening.

Long-run charts retain at most 1,024 deduplicated samples, while progress events and checkpoints are time-throttled. Exact final counters and resumable state remain preserved. Older results stay inspectable but require the v11 rescore before continuation or validation.

Core nonlegendary engines and interaction naturally trend toward three or four copies; expensive or narrow cards trend toward one or two; legendary permanents receive diminishing preference after the second copy. Per-card Min/Max remains the authoritative advanced override, including legal four-of legends.

## 0.1.4 structural-model migration

Catalog model v3 rebuilds automatically from the current Scryfall bulk dataset without removing saved runs, gauntlets, caches, or settings. Transforming cards expose only their front face initially; modal DFCs and other legally selectable faces retain their initial modes. Mana sources distinguish free early production from late or cast-dependent fixing. Results created before the `modes-colored-mana-v3` revision remain inspectable and show a legacy warning. Continuing one starts a new run, re-evaluates it under v3, and applies mandatory mana repair; severe failures cannot enter the frontier or consume game-validation budget.

## 0.1.5 restricted-mana migration

Catalog model v4 rebuilds automatically and retains runs, gauntlets, settings, caches, art, and engine installations. Source rules now distinguish unrestricted mana from legendary-, subtype-, chosen-type-, Commander-identity-, and other restricted spending. Command Tower produces no mana in constructed formats. Unknown colored restrictions are ignored conservatively while a separate unconditional colorless ability remains usable. Hand-based land search is valued only when the deck contains an eligible unconsumed target and can pay the activation without using the search card as its own initial colored source.

Results created before `restricted-mana-synergy-v4` remain inspectable but are marked legacy. Continuing launches a v4 rescore and mandatory mana repair. Every structural, hybrid, successive-halving, sideboard, HexDek, Best-of-Three, and Forge validation path rescans entrants in Rust before spending game budget. A legal deck rejected by structural screening remains exportable. The visible frontier keeps the strongest feasible result and uses copy-vector distance to avoid filling the remaining slots with nearly identical decks.

## 0.1.6 closed-loop migration

Revision `closed-loop-field-v5` separates broad structural exploration from competitive closed-loop evolution. A blank closed loop starts from the highest-weight compatible legal gauntlet deck. Candidate mutations use the seed plus field lists with at least 25% nonland-name overlap, so unrelated archetypes remain opponents without leaking isolated modules into the deck. The exact incumbent is always a finalist. A challenger must improve evidence by at least three percentage points, preserve confidence and worst-matchup floors, and—when Forge is installed—win a bounded independent incumbent cross-check by at least five percentage points before it becomes the next cycle's seed. Older results remain inspectable but are rescored before continuation or validation.

## 0.1.7 color and interface migration

Saved requests may now contain `allowed_colors`. Older requests deserialize as **Auto** and retain their previous behavior. A manual selection is exact and mode-aware: colorless generic-cost spells require the explicit C choice, colored and hybrid modes must have a selected payment route, and lands must provide a selected fixed or dynamic source. The optimizer uses the chosen set as its blank-start palette and cannot re-enable an off-palette card through Min/Max locks or Commander mutation.

Navigation is consolidated from six overlapping destinations to Build, Results, Card Library, and Settings. System engines, storage, caches, and resource limits appear only in Settings. Build begins with a compact five-decision surface and progressively reveals optimizer, pool, Commander, import, and competitive-validation controls. Results keeps secondary archives and real-world feedback collapsed beneath the current frontier.

## 0.1.8 import-safety migration

Arena deck text may name a modal or transforming double-faced card by only its front face. The importer now resolves that front-face alias—including set and collector number—to the catalog's canonical `front // back` card. A pasted list must validate before **Evolve deck** becomes available; an error stays visible and the previous blank workspace cannot run in its place. Clearing the paste explicitly abandons the attempt. A successful import shows its total card count as the active seed and preserves the configured optimizer and policy settings. The 0.1.9 palette behavior below supersedes 0.1.8's exact-color preservation.

## 0.1.9 imported-seed launch migration

Imported and locked cards are now narrow exceptions to HexDek coverage and conformance mutation preferences. This lets a legal deck run structural evolution when one of its existing cards is absent from HexDek or has disputed semantics, without adding every unsupported card to the general mutation pool. Rules-engine validation still performs its normal coverage audit and cannot spend game budget on an unsupported candidate. Focused-pool and exact-color selections remain hard constraints.

Importing a deck resets **Deck colors** to **Auto**, allowing the seed's actual palette to control mutation instead of inheriting a potentially incompatible blank-workspace selection. Population, generation, trial, mutation, fitness, worker, coverage, and exclusion settings remain unchanged. If backend preflight still rejects a run, a persistent **Evolution did not start** panel appears next to the launch control with the exact reason; the imported seed remains active.

## 0.1.10 functional Tune migration

Catalog model v5 rebuilds from current Scryfall data while retaining runs, fields, art, feedback, simulator caches, settings, and installed engines. Revision `functional-actions-tune-v6` replaces whole-card keyword credit with mode-associated actions. Unknown text remains searchable but contributes at most 10% and is explicitly unverified. Older structural results remain inspectable; continuing or validating a nonblank legacy run first reconstructs an immutable Tune root/signature and rescans it under v6. Blank legacy runs remain Rebuild.

Balanced Tune requires 70% weighted strategy retention, 60% support for every primary typed package, and no more than 40% nonland copy edits from the immutable imported root. Lands are outside the edit budget so mana repair remains free. Exact card names are not protected unless Min/Max locks say so. The baseline is pinned at rank zero and cannot mutate; continuation keeps that original identity boundary even when a validated champion becomes the current incumbent.

Structural ranking is shown as **Screen score**. Result statuses are **Baseline**, **Provisional**, **Validated**, and **Improved**. Promotion ignores screen score: HexDek selection must improve by at least three points with non-worse confidence and worst-matchup floors, the paired 64-game-per-opponent holdout must repeat that improvement, and fully supported Forge evidence must improve by five points. Crashes, timeouts, missing definitions, and disputed critical mechanics are failed proof rather than wins or silent exclusions.

## 0.1.11 logical-card and cumulative-continuation migration

Catalog model v6 replaces printing-level card rows with one logical row per case-insensitive card name. Every original Scryfall printing ID remains in a compact alias table so saved runs, fields, and imported set/collector references can migrate to the canonical logical ID. An Arena printing is preferred whenever one exists. The migration retains all user databases and compacts the catalog after rebuilding; alternate sets no longer give a card extra mutation or statistical weight.

Structural revision `logical-cards-continuation-v7` makes **Continue +N generations** an in-place cumulative operation. The selected candidate is pinned as the incumbent while the original imported Tune root remains immutable. Generation, unique-evaluation, avoided-evaluation, and elapsed-time totals continue from their prior values. The run keeps the same history row and retains its title, notes, tags, individual validations, halving brackets, and previous reports. Interrupted continuation also runs only the unconsumed portion of its current segment.

## 0.2.0 competitive-truth migration

Catalog model v7 and structural revision `capability-routing-v8` replace best-effort simulator filtering with explicit mechanic capability and immutable per-matchup routes. Candidate critical actions require complete coverage; trusted engine support must cover at least 95% of the weighted opponent field. HexDek is chosen when it covers both decks, Forge is used when HexDek does not and Forge does, and an uncovered pairing remains visible and blocks the claim. GetPaird fields are always Provisional.

**Continue +N** on a Tune run now performs additional structural generations and automatic tournament validation in the same cumulative run. Follow the results pipeline: **Screened → Coverage checked → Field tested → Holdout → Promoted/Retained**. Screen Score never promotes a deck. Selection, holdout, and any Forge cross-check use paired routes and seeds; failure, timeout, unsupported-card, or incomplete-game evidence cannot count as a win.

## 0.2.1 combo-intelligence migration

The combo corpus is a versioned local knowledge base seeded from logically resolved Commander Spellbook hypotheses. Corpus membership is not proof. The generic solver tracks typed costs, zones, resources, targets, timing, and loop prerequisites under bounded search. The combo inspector distinguishes Corpus only, Symbolically proven, HexDek executed, Cross-engine verified, Arena verified, Disputed, and Unsupported states.

Starting in 0.2.3, the bounded resource solver reports **Resource-balanced hypothesis**, not exact proof. Legacy **Symbolically proven** rows are shown as **Legacy resource hypothesis—reprove required**. Neither tier can guide HexDek or support an Improved claim; only current executed or stronger proof remains eligible until the exact-state solver arrives.

## 0.2.2 learned-acceleration migration

Learned prioritization remains off until at least 5,000 completed, fully covered, current-provenance game labels exist. Training and validation are isolated by deck lineage plus field revision, and the active run's holdout is unavailable until the run finishes. Predictions allocate game attention only; every promotion still requires actual paired games. Uncertain or unfamiliar decks bypass the model and receive rules-engine evaluation.

Burn runs the 32-feature matchup ensemble on CPU everywhere. WGPU is enabled only after local measurement proves at least 2× CPU inference throughput after transfer overhead, CPU parity, stronger calibration than the structural baseline, and at least 15% more estimated closed-loop quality per minute. Device loss or drift returns to CPU automatically. MAP-Elites separately preserves the global best and distinct archetype/color/curve/interaction/victory-path niches so long searches do not collapse into one family.

## 0.2.3 mechanics-truth migration

Catalog model v8 and structural revision `oracle-truth-gate-v9` account for every playable-face Oracle clause after separating reminder text. The candidate and route views show **Oracle accounted → Structurally parsed → Engine executable → Scenario verified → AI decision-covered**. Exact unresolved text remains legal and exportable but contributes no functional value and keeps the result Provisional.

## 0.3.0 Historic Rules Intelligence

Catalog model v9 interprets accounted clauses as ordered typed rules instead of one coarse keyword action. Card modes retain timing, zones, payable costs, targets, conditions, choices, quantities, token characteristics, counters, durations, replacements, and delayed effects. Strategy chips and mutation neighborhoods are derived from those rules. Unsupported or unresolved Historic cards remain available for deck management, but the result stays **Provisional**.

The capability audit can build a full format ledger and a prioritized scenario backlog. The queue puts cards in the active candidate first, then high-weight tournament cards, critical/high-risk clauses, and missing AI decisions. A competitive claim still requires 100% candidate-critical coverage and at least 95% weighted opponent-field execution coverage.

Whole-card conformance from older releases remains visible but cannot verify an unrelated action. A schema-v2 scenario names the exact clause/action claims its assertions exercise. Forge script presence is not treated as execution proof, high-risk copy/replacement/control/digital/turn clauses need scoped scenario evidence, and primary strategy decisions need separate AI-decision coverage. Existing training rows are retained for audit, but only labels with the current capability-policy and Rules revisions count toward learning.

## 0.3.1 Exact Combo and Learning Evidence

Combo results now distinguish a corpus or resource hypothesis from a **State-proven** line. State proof checks the typed Rules IR against exact colored mana, sources and targets, zones, tapped and summoning state, counters, tokens, replacement effects, consumed resources, and once-per-turn restrictions. Repeatable lines must successfully replay a complete second iteration. Only State-proven or stronger lines may guide the bundled game AI.

Learned acceleration also starts from a clean trust boundary. Legacy game labels remain inspectable, but only current route-v2 evidence with complete candidate critical-clause coverage, at least 95% weighted field coverage, and no unresolved or disputed critical mechanic counts toward the 5,000-label threshold. Until every activation gate passes, games run on the normal CPU rules engines and the UI reports why the learned model is unavailable.

## Commander workflow

Select paper or Magic Online **Commander**. Choose **Evolve commander + 99** to explore the whole command zone/library, or search a fixed commander and evolve its 99. Partner, Partner With, Friends Forever, Choose a Background, and Doctor's companion use a versioned compatibility policy. Singleton, basic-land exceptions, color identity, and command-zone legality are repaired after every mutation. Blank singleton structural seeds prefer identity-producing basics and commander-relevant packages, but remain proxy exploration. For competitive claims, use closed-loop evolution with legal pod lists; a completely blank run starts from and protects the highest-weight field deck, while a selected fixed commander starts from a generated shell that must earn rules-engine evidence.

Commander validation uses 2–6 player pods with complete deterministic seat rotations. Ranking blends pod wins, seat floor, threat, interaction, recovery, mana reliability, and closing ability, then subtracts stalls, crashes, concessions, seat bias, and conservative kingmaking signals. Political AI remains an approximation and is disclosed in every report.

## Confidence and evidence labels

- **Screened** is a fast deck-feature estimate, useful for ranking but not a played game.
- **Provisional** means field provenance or critical-mechanic coverage cannot support a competitive claim.
- **Field tested / Validated** is paired rules-engine evidence under an immutable route plan. Inspect engine/card-data revision, configuration hash, field coverage, crashes, cache state, and seat evidence.
- **Improved** means the challenger passed selection, paired holdout, confidence, worst-matchup, and any applicable independent cross-check thresholds.
- **Cross-engine verified** is exact deterministic scenario agreement between independent engines. Disputed and unsupported mechanics remain explicit blockers for critical actions.
- **Real-world** is imported user evidence tied to an exact exported revision. It never silently changes stored experiments.

Wilson intervals quantify sampling uncertainty. Successive halving can eliminate a candidate only when configured confidence bounds separate; otherwise normal budgeted halving applies. Calibration can automatically disable statistical elimination when held-out intervals under-cover. The displayed lower bound is intentionally conservative; a high point estimate with a low lower bound means more games are needed.

## Recovery and privacy

Runs checkpoint automatically and can resume from durable phase boundaries. In Settings, create a bounded local backup before upgrades or large experiments. Restore validates every contained database and creates a safety backup first. A failed integrity check can be quarantined; the original corrupt file is retained while the feature recreates clean storage.

Safe diagnostics contain only app/schema versions, database filenames, byte sizes, and integrity outcomes. They exclude filesystem paths, deck lists, match rows, raw Arena logs, account data, and tokens. Real-match history has individual/all-delete plus portable export controls.

## Installation and updates

The Windows NSIS installer includes the app, notices, and the checksum-verified HexDek Windows runtime/rules corpus. No Go, Git, Python, source checkout, or local compilation is required. The current card catalog is downloaded on first launch so legality is not frozen at release time. Forge is downloaded prebuilt on first launch rather than inflating every installer by another 288 MiB; it remains optional and requires Java 17+. Windows may warn that current development builds are unsigned. Verify the published SHA-256 before running them. Automatic update delivery and signing are disabled until a release channel and credentials are chosen; install a newer verified build manually over the existing version. Local data remains in application data and should be backed up first.
