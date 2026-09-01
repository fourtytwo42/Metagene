# Metagene 0.3.1 — Exact Combo and Trustworthy Learning

Metagene 0.3.1 replaces resource-only combo promotion with conservative typed-state proof derived from the catalog-v9 Rules IR. It keeps every existing deck, run, corpus hypothesis, conformance observation, game summary, and model for audit while requiring current exact evidence before those records can guide play or learning.

## Exact combo proof

- Combo schema v3 tracks card identity, controller, zone, colored mana, permanent types and subtypes, tokens, counters, life, tapped and summoning state, once-per-turn use, delayed triggers, replacements, and continuous modifiers.
- Every action must satisfy its timing, source zone, mana and nonmana costs, legal targets, conditions, and ordered effects.
- A repeatable line becomes **State-proven** only when its canonical post-line state can legally execute the complete line again with consumed resources restored.
- Unsupported relationships and unresolved Rules IR return **not proven**. Resource-balanced and corpus lines remain visible hypotheses but cannot create a HexDek combo plan.
- State-proven lines can be materialized as deterministic engine conformance scenarios; HexDek, Forge, and Arena observations retain their existing stronger proof tiers.

## Learning evidence

- Surrogate schema v4 accepts only completed current-route holdout labels with 100% candidate critical-clause coverage, at least 95% weighted field coverage, no unresolved or disputed high-risk mechanic, and complete engine/card/configuration provenance.
- Older labels and models are preserved for audit but are stale and excluded from the 5,000-label activation threshold.
- CPU or WGPU inference remains prioritization-only and stays disabled until the trustworthy-label, calibration, parity, throughput, and quality-per-minute gates pass. Actual games remain mandatory for promotion.

## Migration

Catalog v9 and structural revision `rules-ir-historic-v10` remain current. Combo databases migrate in place to schema v3. Existing symbolic/resource proof is relabeled as legacy evidence and must be reproved. No run, field, cache, art, feedback, alias, or evidence database is deleted.

## Release artifact

- File: `Metagene_0.3.1_x64-setup.exe`
- Size: 59,454,466 bytes
- SHA-256: `725990B62D17C81311486FE4B8EFF24410E69EFF54EC331F0234F1B244EAECCF`
- Signing: unsigned NSIS package; verify the SHA-256 before installation.
