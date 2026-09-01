# Metagene 0.1.8

Metagene 0.1.8 is an import-safety hotfix for Arena deck lists containing double-faced cards.

## Fixed

- Arena front-face entries such as `Glasspool Mimic (ZNR) 60` and `Bala Ged Recovery (ZNR) 180` resolve to their canonical `front // back` catalog cards.
- Full canonical card names remain authoritative, and both aliased and full names support printing-aware resolution.
- Pasting or editing a deck list disables evolution until that exact text validates.
- A failed import remains visibly failed and cannot silently evolve the prior blank workspace. Clearing the paste explicitly abandons the attempt.
- A successful import displays the active seed's total card count.
- Importing no longer resets population, generations, trials, mutation settings, fitness weights, worker count, simulator coverage, conformance exclusions, or exact deck colors.

## Compatibility

- Catalog model v4 and structural revision `closed-loop-field-v5` are unchanged.
- Existing runs, gauntlets, engines, caches, art, settings, and application data remain compatible.
- Regression coverage includes printing-aware/front-only DFC resolution, failed-import blocking, successful seed activation, and preservation of a one-generation exact-color configuration.

## Verification and installer

- Formatting, 122 application tests (one intentional bundled-runtime extraction smoke ignored), 48 core tests, strict locked all-target/all-feature Clippy, the production TypeScript/Vite build, and Playwright 8/8 passed.
- The silent in-place upgrade returned exit code 0. The installed executable and uninstall registry both report 0.1.8, and the installed app reached input idle with a valid main window.
- The upgrade preserved all 49,291 application-data files (2,644,124,458 bytes) and preserved the exact `runs.sqlite` size and SHA-256.
- The obsolete 0.1.7 installer and Playwright traces were moved to the Recycle Bin; `cargo clean` removed 13,374 reproducible build files (10.5 GiB). Only the 0.1.8 installer remains in `artifacts`.
- Installer: `artifacts/Metagene_0.1.8_x64-setup.exe`
- Size: 47,334,996 bytes
- SHA-256: `F2B951F55B795904AED7F97DB7A9722DCFE49634AE9DAE0CC43C1BCB46A5B653`
