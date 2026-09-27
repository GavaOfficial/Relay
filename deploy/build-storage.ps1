# Compila relay-storage e relay-ops per Linux x86_64 (eseguibili statici musl) dentro Docker e li
# mette in dist/relay-storage e dist/relay-ops. Serve Docker Desktop acceso.
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
  sh -c "apk add --no-cache musl-dev >/dev/null && cargo build --release -p relay-storage -p relay-ops && cp /target/release/relay-storage /target/release/relay-ops /src/dist/"
if ($LASTEXITCODE -ne 0) { throw "compilazione fallita" }
Get-Item "$root\dist\relay-storage", "$root\dist\relay-ops" | Select-Object Name, Length
