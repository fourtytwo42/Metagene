# Metagene 0.1.9

Metagene 0.1.9 fixes imported decks that validate successfully but fail before structural workers start.

## Fixed

- Required imported and locked cards survive HexDek parser-coverage and conformance-exclusion filtering as narrow structural-pool exceptions.
- The exception does not widen the general mutation pool to unrelated unsupported cards.
- Focused-pool and exact-color selections remain hard constraints.
- Importing establishes an automatic palette from the deck instead of retaining a potentially incompatible exact-color selection.
- Population, generations, trials, mutation controls, fitness weights, workers, simulator coverage, and conformance exclusions still survive import.
- Backend launch failures remain visible beside **Evolve deck**, include the exact reason, and confirm that the imported seed remains active.

## Regression evidence

- A unit regression restores required seed cards removed by engine policies while proving an unrelated unsupported card remains excluded.
- The exact user-supplied 60-card Historic list was resolved against the installed catalog, prepared under the default parser-complete/disputed/unsupported policies, and completed a bounded one-generation structural run.
- Browser coverage verifies failed imports remain blocked, generations survive successful imports, import resets exact colors to Auto, and backend start errors remain visible.

## Compatibility

- Catalog model v4 and structural revision `closed-loop-field-v5` are unchanged.
- Existing runs, gauntlets, engines, caches, art, settings, and application data remain compatible.

## Verification and installer

- Formatting, 123 application tests (one intentional bundled-runtime extraction smoke ignored), 48 core tests, strict locked all-target/all-feature Clippy, the production TypeScript/Vite build, and Playwright 9/9 passed.
- The exact installed-app workflow imported the user's 60-card list, retained one generation, completed evolution, displayed 16 candidates, and retained `Nightclub Bouncer` in the winning structural result under the default parser-complete/disputed/unsupported policy settings.
- The silent in-place upgrade returned exit code 0. The installed executable and uninstall registry both report 0.1.9.
- The upgrade preserved all 49,287 pre-test application-data files (2,644,058,922 bytes) and preserved the exact pre-test `runs.sqlite` size and SHA-256. The installed workflow then added its expected completed run.
- Computer Use was released immediately after the installed-app verification; its Metagene window was closed and the automation session reset.
- The obsolete 0.1.8 installer and Playwright traces were moved to the Recycle Bin; `cargo clean` removed 14,300 reproducible build files (13.0 GiB). Only the 0.1.9 installer remains in `artifacts`, with no Metagene/build/test process left running.
- Installer: `artifacts/Metagene_0.1.9_x64-setup.exe`
- Size: 47,335,946 bytes
- SHA-256: `F09959B3BAA538B40A374B4F26A5741E4AB13A6B07A55D2F2B822682F0105616`
