# Releasing Metagene on Windows

Run `powershell -ExecutionPolicy Bypass -File scripts/release.ps1`. The script installs exactly `package-lock.json`, runs locked Rust tests and Clippy, builds the frontend, runs Edge E2E tests, creates an NSIS installer, and prints size plus SHA-256. By default the artifact is `target/release/bundle/nsis/Metagene_<version>_x64-setup.exe`; when `CARGO_TARGET_DIR` is set, the script resolves the bundle beneath that target instead. The release workstation currently uses the task-specific `D:\MetageneTarget` to keep reproducible build output off the repository volume.

Release inputs are pinned by `Cargo.lock`, `package-lock.json`, the Rust toolchain, Tauri configuration, bundled adapter source, and the checked-in notices. The script provides a reproducible procedure and verifiable artifact hash; it does not claim bit-for-bit deterministic PE/NSIS output across Windows SDK or toolchain revisions.

Before publishing:

1. Run the full script on a clean Windows host with Edge WebView2 and NSIS.
2. Perform the isolated silent install/launch/uninstall smoke and record the installer hash.
3. Review `THIRD_PARTY_NOTICES.md` against both lockfiles and upstream Forge/HexDek licenses.
4. Publish the installer hash beside the artifact.

Current builds are deliberately unsigned. When the owner chooses a distribution channel, configure a protected code-signing certificate in that channel, sign in CI without exposing credentials, verify Authenticode and SmartScreen reputation, then configure a signed Tauri updater endpoint. Do not enable update checks or embed a public updater key until that endpoint and rollback policy exist.
