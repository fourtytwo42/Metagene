# Metagene 0.1.11

Metagene 0.1.11 fixes completed-run continuation and removes printing duplication from the logical card catalog.

## Continuation

- **Continue +N generations** restarts the same saved run rather than creating an empty child experiment.
- The selected deck is pinned as the current incumbent, while the original imported Tune root and strategy boundary remain immutable.
- Completed generation, evaluation, avoided-evaluation, and elapsed-time totals are cumulative.
- Run title, notes, tags, validation rows, halving reports, and history identity remain attached.
- An interrupted continuation consumes only the remaining part of its current segment.
- Previous game evidence is matched by exact deck genome rather than rank, preventing an old rank from being applied to a different new candidate.

## Logical catalog

- Catalog model v6 stores exactly one case-insensitive logical row per card name.
- All Scryfall printing IDs remain as compact aliases to the logical card, preserving legacy saved runs without retaining duplicate card records.
- Arena-supported cards prefer an Arena set/collector representative for valid MTGA export.
- The optimizer rejects any externally supplied pool containing two IDs for the same name.
- The current live migration reduced 117,608 printing rows to 38,184 logical cards with zero duplicate names. All 312 card IDs referenced by existing saved runs resolved through v6 aliases.
- Post-migration compaction reduced `catalog.sqlite` from 667,922,432 to 180,867,072 bytes.

## Structural compatibility

The structural revision is `logical-cards-continuation-v7`. Older results remain inspectable and are rescored before continuation or validation. Nonblank Tune runs keep their persisted original root; blank runs remain Rebuild.

## Verification

- `cargo fmt --all -- --check` passed.
- `cargo test --workspace --all-targets --locked` passed: 52 core tests and 139 backend tests, with one intentional bundled-runtime extraction smoke ignored.
- Strict locked all-target/all-feature Clippy passed with warnings denied.
- The production TypeScript/Vite build passed.
- Playwright passed 10/10 workflows, including a cumulative-continuation regression.
- The current live catalog migrated to 38,184 unique logical cards and 117,608 printing aliases with zero duplicate names; every card ID referenced by saved runs resolved.
- The installed 0.1.11 app continued the latest imported Historic Tune run from generation 1 to cumulative generation 2. It retained the same run ID and four-row history, pinned the chosen deck as `incumbent`, and preserved the immutable 60-card `Imported Historic Deck` root.
- Computer Use was released immediately after the smoke test, the app was closed, and no Metagene process remained.
- Superseded/test artifacts were recycled and `cargo clean` removed 15,242 reproducible files / 12.9 GiB. Only the 0.1.11 installer remains in `artifacts/`.

## Installer

- Installer: `artifacts/Metagene_0.1.11_x64-setup.exe`
- Size: 47,446,395 bytes
- SHA-256: `AE8A5FFD1C9E486356651DF9F0CBCC8BB82C75065334DBE1D193DEB1D3A4F6E6`
