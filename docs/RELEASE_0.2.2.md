# Metagene 0.2.2 — Learned Acceleration

Metagene 0.2.2 adds evidence-gated matchup learning and quality-diversity search. Training starts only after 5,000 trustworthy labels from completed, fully covered, current-provenance games with no crash, timeout, or disputed critical mechanic. Labels are bounded to 250,000 summaries or 750 MiB.

The 32-feature matchup ensemble represents both decks, format, play/draw position, functional roles, strategy packages, curve, mana, and combo evidence. Train/validation partitions are isolated by deck lineage and field revision; the active run and its holdout cannot enter training until that run completes. Uncertain and out-of-distribution candidates still receive real games, and predictions only prioritize candidates or allocate budget—promotion always requires played evidence.

Burn supplies a universal CPU backend plus bundled WGPU acceleration. Device benchmarking enables GPU inference only when it is at least 2x faster after transfer overhead, numerically agrees with CPU, improves calibration/log loss, and estimates at least 15% more closed-loop quality per minute. Device loss, weak hardware, transfer overhead, or model drift immediately returns inference to CPU.

MAP-Elites preserves the global best and distinct niches across archetype, colors, curve, interaction, and victory path. Mutation neighborhoods combine typed-action similarity, tournament co-occurrence in Rebuild, validated packages, and proven combo relationships. Opponent modules never leak into strategy-preserving Tune mutations.

## Verification and Windows installer

Metagene 0.2.2 is the cumulative Windows release containing the 0.2.0 Competitive Truth, 0.2.1 Combo Intelligence, and 0.2.2 Learned Acceleration milestones.

- Windows x64 installer: `artifacts/Metagene_0.2.2_x64-setup.exe`
- Size: 59,256,816 bytes
- SHA-256: `2C2F37379C415AAD37130851A33E6B4DE342D01A28D50DD75ACC43A1BBA99288`
- Installed executable and uninstall registration report ProductVersion 0.2.2.
- The in-place upgrade preserved runs, fields, feedback, and matchup caches. Catalog model v7 rebuilt automatically to 38,184 unique logical cards and 117,608 printing aliases.
- The bundled HexDek runtime upgraded an installed v2 adapter to `metagene-fixed-order-commander-metrics-combo-plan-v3` without Git, Go, Python, or a compiler.
- Release gates passed 154 application tests (plus the separate 289 MiB extraction/upgrade smoke), 55 core tests, strict all-target/all-feature Clippy, formatting, the production frontend build, and all 11 Playwright workflows.
- The fixed structural benchmark measured 21,322–21,555 candidate slots/s. The comparable 64-game constructed profile completed in 2.621 seconds (24.4 games/s end-to-end), and the Commander smoke completed all 16 games across four seat rotations with no partial games.

The installer is intentionally unsigned until a protected signing credential and rollback-safe distribution channel are configured. Verify the SHA-256 before running it.
