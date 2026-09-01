# Metagene 0.2.0 — Competitive Truth

Metagene 0.2.0 replaces generic public-list testing with an auditable tournament-field pipeline. It prefers Spicerack occurrence data, supports TopDeck with a request-only API key, and uses GetPaird only as a clearly provisional fallback. Exact logical-card deck fingerprints are merged before near-duplicate archetype clustering; constructed fields adapt from eight to sixteen representatives until they cover at least 90% of observed weight.

Catalog model v7 and structural revision `capability-routing-v8` inventory mechanics per card mode and action. Candidate critical actions require 100% route coverage, while trusted support must cover at least 95% of opponent-field weight. An immutable route plan chooses HexDek first, Forge when needed and supported, or blocks the competitive claim. Unsupported opponents remain visible in the Field Audit.

Tune continuation is cumulative and validated by default. **Continue +N** preserves the tuning root, incumbent, field revision, cumulative generations, and prior evidence, then runs `Screened → Coverage checked → Field tested → Holdout → Promoted/Retained`. Selection and holdout use paired engines, configurations, starting positions, and seeds. Crashes, timeouts, unsupported cards, and incomplete games are failed evidence.

Compatibility: existing runs, fields, art, feedback, caches, and settings remain readable. A legacy run is rescanned under catalog v7 before continuation or validation. Matchup cache entries remain reusable only when engine, adapter, card-data, route, and configuration identities match.
