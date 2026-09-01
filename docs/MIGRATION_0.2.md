# Migrating to Metagene 0.2

Metagene 0.2.3 is the cumulative Windows release containing the 0.2.0 Competitive Truth, 0.2.1 Combo Intelligence, 0.2.2 Learned Acceleration, and 0.2.3 Mechanics Truth Gate milestones. Install it over an existing Metagene installation; application data is retained.

## Preserved data

Runs, immutable Tune roots, fields, art, feedback, conformance observations, settings, engine installations, and compatible matchup caches remain in application data. The logical-card catalog rebuilds as model v8 from current Scryfall bulk data. `combos.sqlite`, the bounded training-label store, and current/rollback surrogate models are managed automatically.

## Legacy results

Older results remain inspectable. Continuing or validating rescans them under `oracle-truth-gate-v9`; unsupported competitive claims are blocked instead of silently excluding cards or opponents. Schema-v1 conformance observations remain card-level audit evidence but cannot verify a clause. Old training labels and surrogate models remain stored but are ineligible until current capability-policy and Rules revisions exist. Resource-only combo proofs are shown as legacy hypotheses and cannot guide play. Blank legacy runs continue as Rebuild. Nonblank legacy runs reconstruct their original Tune signature and retain its immutable identity boundary.

## Changed evidence language

Screen Score is structural ranking, not a win-rate claim. Competitive results show Evidence Tier, field coverage, route plan, confidence interval, and the exact promotion decision. GetPaird-derived fields, incomplete engine coverage, and unverified critical mechanics remain Provisional. Existing cached games are accepted only when all engine, adapter, card-data, route, and configuration identities still match.

## Disk policy

Only the current combo corpus and normalized evidence are kept. Game summaries are capped at 250,000 or 750 MiB, current plus rollback surrogate models at 256 MiB, and raw engine jobs/staging files are removed automatically unless diagnostic retention is explicitly enabled.
