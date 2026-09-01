# Third-Party Notices

Last reviewed: 2026-08-30. This notice is informational, not legal advice.

Metagene is MIT-licensed. Its locked Rust and npm dependency graphs were reviewed with `cargo metadata --locked` and `package-lock.json`; all declared packages use permissive or file-level copyleft licenses compatible with distribution here, including MIT, Apache-2.0, BSD, ISC, Zlib, Unicode, CDLA-Permissive, MPL-2.0, and their dual-license combinations. Complete authorship and license texts remain in the Cargo/npm source packages identified by `Cargo.lock` and `package-lock.json`.

Notable components and services:

- Tauri and its Rust/JavaScript dependencies: MIT and/or Apache-2.0, with some transitive MPL-2.0/file-level components.
- React, React DOM, Vite, TypeScript, and Playwright: MIT or Apache-2.0 as declared in their packages.
- SQLite (through bundled `rusqlite`): public domain SQLite plus permissively licensed bindings.
- Burn, WGPU, CubeCL, and gpu-allocator: MIT and/or Apache-2.0. Metagene uses Burn's bundled Flex CPU and WGPU/Vulkan inference backends; the vendored `gpu-allocator` source is unmodified and only its Windows dependency version is pinned for Tauri/WGPU compatibility.
- Commander Spellbook backend corpus: AGPL-3.0 source and openly published combo data. Metagene downloads the current API corpus into user application data as seed hypotheses; it does not bundle the backend service or treat corpus entries as proof. Upstream: <https://github.com/SpaceCowMedia/commander-spellbook-backend>.
- HexDek: MIT. Windows x64 installers redistribute the pinned HexDek worker, Metagene conformance companion, MIT license, and versioned rules/coverage snapshot so end users do not need a compiler. The companion source and patches are distributed with Metagene. Upstream: <https://github.com/hexdek-labs/HexDek>.
- Card-Forge/Forge: GPL-3.0. Forge is an optional, separately installed and separately executed program. Metagene does not link Forge into its binary. The adapter source is shipped beside its compiled adapter JAR so recipients can inspect and rebuild it; Forge source and license are available at <https://github.com/Card-Forge/forge>.
- Scryfall: the bundled HexDek rules snapshot was derived from Scryfall Oracle bulk data dated in its included `VERSION` file; Metagene's current legality catalog and card images are still fetched at runtime. Metagene follows Scryfall's bulk-data guidance and image CDN usage. Card names, rules text, symbols, and artwork remain property of their respective owners.
- Magic: The Gathering, MTG Arena, card names, card text, symbols, and artwork are trademarks or copyrights of Wizards of the Coast LLC and artists. Metagene is unofficial and is not affiliated with or endorsed by Wizards of the Coast, Scryfall, Forge, or HexDek.

Metagene does not bundle card artwork, its current legality catalog, or Forge. The Windows x64 installer does bundle the checksum-verified HexDek runtime/rules snapshot; automatic first-run setup downloads the current catalog and prebuilt Forge archive without local compilation. Settings retains explicit refresh/repair controls.
