param([switch]$SkipTests)
$ErrorActionPreference = "Stop"
$repo = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $repo
if (-not $SkipTests) {
  npm ci
  cargo test --workspace --all-targets --locked
  cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
  npm run build
  npm run test:e2e
}
npm run tauri -- build --ci --no-sign
$targetRoot = if ($env:CARGO_TARGET_DIR) {
  if ([System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
    $env:CARGO_TARGET_DIR
  } else {
    Join-Path $repo $env:CARGO_TARGET_DIR
  }
} else {
  Join-Path $repo "target"
}
$bundle = Join-Path $targetRoot "release\bundle\nsis"
$artifacts = Get-ChildItem -LiteralPath $bundle -File | Sort-Object Name
if (-not $artifacts) { throw "No NSIS artifact was produced." }
$artifacts | ForEach-Object {
  $hash = Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256
  [PSCustomObject]@{ File = $_.Name; Bytes = $_.Length; SHA256 = $hash.Hash }
} | Format-Table -AutoSize
