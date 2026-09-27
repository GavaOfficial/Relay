# Compila relay-storage per Linux x86_64 (eseguibile statico musl) dentro Docker e lo mette in
# dist/relay-storage. Serve Docker Desktop acceso.
#
#   powershell -File deploy/build-storage.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
New-Item -ItemType Directory -Force -Path "$root\dist" | Out-Null
docker run --rm `
  -v "${root}:/src" `
  -v relay-storage-cargo:/usr/local/cargo/registry `
  -v relay-storage-target:/target `
  -w /src `
  -e CARGO_TARGET_DIR=/target `
  rust:1-alpine `
  sh -c "apk add --no-cache musl-dev >/dev/null && cargo build --release -p relay-storage && cp /target/release/relay-storage /src/dist/relay-storage"
if ($LASTEXITCODE -ne 0) { throw "compilazione fallita" }
Get-Item "$root\dist\relay-storage" | Select-Object Name, Length
