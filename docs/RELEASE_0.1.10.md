# Metagene 0.1.10

Metagene 0.1.10 replaces keyword-count deck scoring with functional action evaluation and makes imported-deck evolution strategy-preserving by default.

## What changed

- Catalog model v5 stores mode-associated abilities with costs, zones, timing, targets, conditions, magnitude, confidence, and typed effects.
- Actions contribute only when they are reachable, payable, and useful. Colored Skullbomb modes, search without targets, narrow interaction, and graveyard payoffs without setup now lose value explicitly.
- Imported decks use Balanced Tune: 70% overall retention, 60% for every primary package, and a 40% nonland edit budget. The immutable root remains a separately pinned baseline. Blank starts use Rebuild.
- The visible frontier uses strict quality/diversity selection and may contain fewer than eight candidates rather than near-clones.
- Tune automatically stages structural screening, compatible recent-field reuse or 30-day GetPaird acquisition, HexDek successive halving, paired holdout, and a Forge cross-check when supported.
- Screen score is never used to promote a challenger. Results distinguish Baseline, Provisional, Validated, and Improved, with exact proof or rejection reasons.
- The UI exposes strategy retention, package evidence, functional/dead modes, role balance, closing power, narrow-card deductions, copy rationale, and paired game evidence under progressive disclosure.

## Compatibility

Catalog v5 rebuilds automatically without removing runs, gauntlets, art, feedback, settings, engine installations, or matchup caches. Older results remain inspectable. Nonblank legacy continuation reconstructs a Tune signature from the stored original seed and rescans under `functional-actions-tune-v6`; blank legacy runs remain Rebuild.

## Acceptance evidence

- The original Simic copy/token deck and all eight submitted 2,000-generation artifact shells are permanent regression fixtures.
- The real-catalog 2,000-generation Tune run evaluated 128,000 slots / 104,982 unique decks in 72.78 seconds. It preserved the exact baseline, rejected every artifact-shell fixture, retained the copy/token/blink strategy, and returned one candidate meeting the 12-copy diversity floor.
- The fixed 250-card benchmark produced 21,376–21,586 slots/s and 1.305–1.318M trials/s in three consecutive release samples.
- Formatting, 182 Rust tests, strict all-target/all-feature Clippy, the production frontend build, and Playwright 10/10 passed.
- A silent in-place install reported product version 0.1.10 and preserved the existing application-data store. In the installed app, the supplied 60-card list imported exactly, Tune detected copy/token/blink packages, one generation retained the exact root as a 100% baseline, and MTGA deck-list export succeeded.
- The live GetPaird acquisition used by that smoke did not provide a complete simulator-compatible field. Every evolved result was therefore labeled Provisional, with no improvement claim, as required.

## Installer

- Installer: `artifacts/Metagene_0.1.10_x64-setup.exe`
- Size: 47,426,182 bytes
- SHA-256: `4A6A742AB60DC5FBA7B3D7B66DB2E4E4D2E200BBE9E8DAF3277CD4D2E25B07B7`
