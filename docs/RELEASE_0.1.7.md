# Metagene 0.1.7

Metagene 0.1.7 makes deck color an explicit construction decision and replaces the six-view technical console with a compact, progressively disclosed product workflow.

## Deck colors

- **Auto** retains mode-aware palette inference for unrestricted exploration.
- Manual W/U/B/R/G/C chips define an exact palette. C can stand alone for a zero-color deck or accompany any colored selection.
- Generic-cost colorless spells require the C choice; colored, hybrid, and explicit `{C}` costs require a valid selected route.
- Initially playable spell/land modes and dynamic mana lands are filtered before evolution.
- Blank seeds use the exact palette. Locked cards, mutation, Commander selection, and continuation cannot silently restore an off-palette card.
- Legacy saved requests deserialize as Auto.

## Product interface

- Four destinations replace six overlapping pages: **Build**, **Results**, **Card Library**, and **Settings**.
- Build opens with deck colors, a Quick/Balanced/Deep search preset, import/focused-pool entry points, and one launch action.
- Optimizer internals, fitness weights, mana guidance, pool constraints, and competitive validation use progressive disclosure.
- Results combines the current frontier with collapsible run history, Hall of Fame, and real-world feedback.
- Engines, catalog, resource limits, caches, backup, recovery, and diagnostics now appear only in Settings.
- The visual system changes from dense dark green to a restrained graphite-and-indigo workspace with stronger hierarchy, spacing, control states, and accessible navigation labels.

## Compatibility and verification

- Structural revision remains `closed-loop-field-v5`; the new field constrains search space rather than changing stored fitness semantics.
- Catalog model remains v4 and no catalog rebuild is required.
- Runs, gauntlets, match caches, art, feedback, Hall of Fame, calibration, engines, and settings remain in place.
- Regression coverage includes colorless-only, mono-color, multicolor-plus-colorless, hybrid routing, explicit colorless pips, palette bounds, responsive controls, four-view navigation, legacy continuation, and system recovery.

## Windows release

- Installer: `artifacts/Metagene_0.1.7_x64-setup.exe`
- Size: 47,333,469 bytes
- SHA-256: `4E3F33E83D7823929383170B80D2EF82BB77EB3D34E42F411807D209DA811671`
- The locked release gate passed 122 application tests, 47 core tests, strict all-feature Clippy, formatting, the production frontend build, and all seven Edge/Playwright workflows. One 289 MiB bundled-runtime extraction smoke remains intentionally ignored in the routine gate.
- A silent in-place upgrade from 0.1.6 registered 0.1.7, preserved all existing application-data files and bytes, and reached the installed desktop window's input-idle state successfully.
