# Performance and Resource Policy

Last verified 2026-08-31 on the local Windows development host. These are local measurements, not portable hardware claims. “Completed” excludes crashes; “cached” is reported separately.

## Profiled pipeline

`cargo run -p metagene-app --example profile_pipeline` emits schema-versioned JSON and obtains a genuinely cold seed before immediately replaying it warm.

| Work | Cold | Warm | Notes |
|---|---:|---:|---|
| Arena Historic legal catalog | 188,418 ms | 87 ms | Cold value is the decoded-pool load from the preserved pre-migration store; the installed app's automatic v7 catalog rebuild is a separate first-run setup operation |
| HexDek coverage filter | 223 ms | memory-indexed | Parser-complete pool |
| Structural evolution | 473 ms | — | 244 unique evaluations in the debug end-to-end profiler; the release benchmark below is the structural throughput gate |
| Full diagnostic JSON serialization | 854 ms | — | 41,985,238 bytes; diagnostic only, not engine IPC |
| Constructed BO1, 64 games | 2,621 ms | 28 ms | Engine 2,582 ms cold; 64/64 completed and cached warm; one versus zero process starts |

Process RSS moved from about 17 MiB to 420 MiB while retaining the decoded catalog and complete validation pool. The user-configured memory limit guards this tradeoff.

## Workload matrix

| Workload | Measured result |
|---|---|
| Constructed BO1 cold | 64 completed in 2.621 s (24.4 games/s end-to-end; 24.8 engine games/s), zero crashes |
| Constructed BO1 warm | 64 cached in 28 ms, zero process starts |
| Constructed BO3 cold | Four matches/eight games in 7.698 s |
| Constructed BO3 warm | Same four matches/eight games in 47 ms, all eight cached |
| Commander pod | Release smoke completed 16/16 across four four-seat rotations, zero partial games, native metrics, 92 directed targets, and an exact 16-game cache replay; the prior 384-game halving profile remains 21.4 games/s |
| Structural | 249 unique evaluations in 28 ms; duplicate reuse is reported separately |

Commands:

```text
cargo run -p metagene-app --example profile_pipeline
cargo run -p metagene-app --example smoke_hexdek_best_of_three
cargo run -p metagene-app --example benchmark_hexdek_halving
```

## Restricted-source structural benchmark

Metagene 0.1.5 runs the fixed 250-card benchmark with unrestricted, subtype-only, unknown-restricted, and Commander-identity lands in the same pool. On the release host it processed 10,240 population slots (9,780 unique evaluations) and 625,920 trials in 0.44 seconds: 23,433 slots/s, 22,381 unique evaluations/s, and 1.43M trials/s. This remains above 10,000 candidates/s and is 1.82× slower than the 0.1.4 baseline of approximately 42,570 slots/s, within the 2× regression ceiling.

Metagene 0.1.6 reuses per-candidate library, hand, and source buffers and uses stack-backed mana-payment probes. The same 250-card/10,240-slot/64-trial workload processed 9,762 unique evaluations and 624,768 trials at 23,885–24,256 slots/s and 1.46–1.48M trials/s on the release host. It remains above 10,000 slots/s and within the 2× 0.1.4 ceiling after closed-loop field hardening.

Metagene 0.1.10 adds functional actions, archetype roles, closing power, contextual copy targets, and Tune evidence. Verbose action/copy/role explanations are deferred until the visible finalists, trial buffers are reused, payment probes roll back on the stack, and trials without search sample only the observable library prefix. Three consecutive release samples processed 10,240 slots (9,771 unique evaluations) and 625,344 trials at 21,376–21,586 slots/s and 1.305–1.318M trials/s. This remains above 10,000 slots/s and within 2× of the 42,570-slot/s v0.1.4 reference.

Metagene 0.2.2 adds typed substitution neighborhoods and bounded MAP-Elites archiving while keeping those operations outside the inner structural trial loop. Three direct optimized samples of the fixed 250-card restricted-source benchmark processed 10,240 slots (9,762 unique evaluations) and 624,768 trials at 21,322–21,555 slots/s and 1.301–1.315M trials/s. The slowest retained sample exceeds the 21,285-slot/s half-baseline boundary, so the release remains above 10,000 slots/s and no more than 2× slower than the 42,570-slot/s v0.1.4 reference. An immediately preceding cold sample under simultaneous host compilation measured 20,938 slots/s and is retained as host-contention context rather than hidden.

Metagene 0.2.3 adds lossless Oracle-clause accounting and claim-critical residual evidence while keeping catalog parsing and verbose explanations outside the structural hot loop. The fixed 250-card release sample processed 10,240 slots (9,762 unique evaluations) and 624,768 trials in 0.49 seconds: 20,992 slots/s, 20,012 unique evaluations/s, and 1.281M trials/s. It remains above 10,000 slots/s, is about 1.5% below the recorded 0.2.2 minimum, and remains well inside the 2× ceiling.

## Worker decision

HexDek’s tournament executable keeps its configured Go worker goroutines alive throughout each multi-game batch. Metagene keeps a bounded outer Rayon process pool across candidates and divides the CPU budget among child workers to prevent nested oversubscription.

Upstream also ships a long-lived HTTP server, but it is a stateful product server with SQLite, sessions, live showmatches, and rate-limited API behavior—not a stateless tournament request protocol. Reusing it would weaken isolation and deterministic reset guarantees. The measured cold BO1 breakdown attributes 2,527/2,567 ms to engine execution and only 7 ms to deck serialization, so Metagene retains isolated tournament batches until upstream exposes a resettable local worker protocol. Warm deterministic results avoid process startup entirely through the versioned cache.

## Resource governance

The GUI persists CPU-thread, maximum-engine-process, memory, run-time, and disk budgets in application data. Hybrid runs apply the smaller CPU/process cap, check memory and disk before work and each cycle, stop recoverably at the time boundary, and reject work that exceeds memory or disk. Simulation job directories older than 24 hours are treated as orphaned and removed; active/recent jobs are preserved.

Retention limits are explicit:

| Data | Limit |
|---|---:|
| Match cache | 10,000 entries and 256 MiB logical JSON |
| Card art | 512 MiB LRU; 8 MiB per image |
| Runs | 200 |
| Validations / BO3 reports | 500 each |
| Halving reports | 100 |
| Sideboard evolutions | 250 |
| Gauntlet revisions | 200 |
| Hall of Fame | 64 per scope, 512 total |
| Calibration observations | 2,000 |
| Trustworthy game summaries | 250,000 and 750 MiB |
| Current + rollback surrogate models | 256 MiB |
| Conformance observations | 100,000 |

Catalog and installed engine artifacts are single current revisions, not accumulating build trees. Installer staging, Go module caches, and source checkouts are removed before installation becomes visible.

## Learned-acceleration policy

The matchup ensemble remains dormant below 5,000 trustworthy labels. Accepted labels must be completed, fully covered games with current engine/card provenance and no crash, timeout, or disputed critical mechanic. Its 32-feature context represents both decks, format, play/draw position, card functions, strategy packages, curve, mana, and combo evidence. Training and validation split by deck lineage plus field revision, never by near-clone row; the active run and its holdout are excluded until completion.

Burn Flex provides the universal CPU backend and a WGPU backend without requiring CUDA, Python, or a compiler. Local device benchmarking includes transfer overhead and enables WGPU only when throughput is at least 2× CPU, maximum CPU/GPU prediction divergence is at most `1e-4`, validation calibration/log loss beats the structural baseline, and estimated closed-loop quality per minute improves by at least 15%. Device initialization/loss, insufficient speed, drift, or out-of-distribution input falls back automatically. The model prioritizes candidates and allocates game budgets only; promotion always requires real paired simulator evidence.

MAP-Elites archives the strongest feasible candidate per archetype/color/curve/interaction/victory-path descriptor, preserves the global best, and seeds later generations from distinct niches. The visible archive is bounded to 32 cells and the internal search archive to 128 cells. Typed-action neighborhoods are bounded per card, preventing an all-pairs catalog expansion while still allowing tournament, validated-package, and combo co-occurrence to connect complementary roles.
