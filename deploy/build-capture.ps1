param(
    [switch]$SkipX86,
    [switch]$Zoo,
    [string]$BuildDir = 'target/capture-build',
    [string]$CertificateThumbprint,
    [string]$TimestampUrl = 'http://timestamp.digicert.com'
)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Push-Location $root
try {
    function Cargo-Step {
        & cargo @args
        if ($LASTEXITCODE -ne 0) { throw "cargo fallito: $args" }
    }
    $oldZooRuntime = $env:RELAY_ZOO_RUNTIME
    $targets = @{ x64 = 'x86_64-pc-windows-msvc' }
    $build = [IO.Path]::GetFullPath((Join-Path $root $BuildDir))
    $oldZooRuntime = $env:RELAY_ZOO_RUNTIME
    if (-not $SkipX86) { $targets.x86 = 'i686-pc-windows-msvc' }
    $runtime = Join-Path $root 'dist/capture-runtime'
    New-Item -ItemType Directory -Force -Path $runtime | Out-Null
    foreach ($arch in $targets.Keys) {
        $target = $targets[$arch]
        Cargo-Step @('build', '--release', '--target-dir', $build, '--target', $target, '-p', 'relay-hook', '-p', 'relay-inject', '-p', 'relay-vk-layer')
        Cargo-Step @('test', '--release', '--target-dir', $build, '--target', $target, '-p', 'relay-hook-protocol', '--lib')
        $dest = Join-Path $runtime "hooks/$arch"
        New-Item -ItemType Directory -Force -Path $dest | Out-Null
        $manifest = Get-Content -LiteralPath (Join-Path $root 'client/capture/vk-layer/relay-vk-layer.json') -Raw | ConvertFrom-Json
        $manifest.layer | Add-Member -NotePropertyName library_arch -NotePropertyValue $(if ($arch -eq 'x64') { '64' } else { '32' })
        $json = $manifest | ConvertTo-Json -Depth 5
        [IO.File]::WriteAllText((Join-Path $dest 'relay-vk-layer.json'), $json, [Text.UTF8Encoding]::new($false))
        foreach ($file in @('relay_hook.dll', 'relay-inject.exe', 'relay_vk_layer.dll')) {
            Copy-Item -LiteralPath (Join-Path $build "$target/release/$file") -Destination $dest -Force
        }
    }
    Cargo-Step @('build', '--release', '--target-dir', $build, '-p', 'relay-capture')
    Copy-Item -LiteralPath (Join-Path $build 'release/relay-capture.exe') -Destination $runtime -Force
    Copy-Item -LiteralPath (Join-Path $root 'deploy/register-vulkan-layer.ps1') -Destination $runtime -Force
    if ($CertificateThumbprint) {
        $signtool = (Get-Command signtool.exe -ErrorAction Stop).Source
        $artifacts = @((Join-Path $runtime 'relay-capture.exe'))
        foreach ($arch in $targets.Keys) {
            $artifacts += Join-Path $runtime "hooks/$arch/relay-inject.exe"
            $artifacts += Join-Path $runtime "hooks/$arch/relay_hook.dll"
            $artifacts += Join-Path $runtime "hooks/$arch/relay_vk_layer.dll"
        }
        foreach ($artifact in $artifacts) {
            & $signtool sign /sha1 $CertificateThumbprint /fd SHA256 /tr $TimestampUrl /td SHA256 $artifact
            if ($LASTEXITCODE -ne 0) { throw "Firma fallita: $artifact" }
            & $signtool verify /pa $artifact
            if ($LASTEXITCODE -ne 0) { throw "Verifica firma fallita: $artifact" }
        }
    }
    if ($Zoo) {
        # Explicitly requested visible test applications; no background windows.
        Cargo-Step @('build', '--release', '--target-dir', $build, '-p', 'relay-hook', '-p', 'relay-inject', '-p', 'relay-vk-layer')
        Cargo-Step @('build', '--release', '--target-dir', $build, '-p', 'relay-recorder', '--examples')
        $env:RELAY_ZOO_RUNTIME = Join-Path $build 'release'
        & (Join-Path $build 'release/examples/relay-zoo-check.exe')
        if ($LASTEXITCODE -ne 0) { throw 'Zoo OpenGL in finestra fallito' }
        & (Join-Path $build 'release/examples/relay-zoo-check.exe') --exclusive
        if ($LASTEXITCODE -ne 0) { throw 'Zoo OpenGL esclusivo fallito' }
        Cargo-Step @('test', '--release', '--target-dir', $build, '-p', 'relay-recorder', '--lib', 'hook_to_h264', '--', '--ignored', '--nocapture')
        Cargo-Step @('test', '--release', '--target-dir', $build, '-p', 'relay-recorder', '--lib', 'gpu_resize_and_restart', '--', '--ignored', '--nocapture')
        foreach ($arch in $targets.Keys) {
            $target = $targets[$arch]
            Cargo-Step @('build', '--release', '--target-dir', $build, '--target', $target, '-p', 'relay-recorder', '--examples')
            & (Join-Path $build "$target/release/examples/relay-zoo-check.exe") --exclusive
            if ($LASTEXITCODE -ne 0) { throw "Zoo OpenGL $arch fallito" }
            & (Join-Path $build "$target/release/examples/relay-zoo-check.exe") --exclusive --remote
            if ($LASTEXITCODE -ne 0) { throw "Zoo iniezione remota $arch fallito" }
        }
    }
    Write-Output "Runtime locale: $runtime"
    Write-Output 'La pubblicazione del solo relay-capture.exe non distribuisce gli hook: usare il runtime completo per le prove.'
} finally {
    $env:RELAY_ZOO_RUNTIME = $oldZooRuntime
    Pop-Location
}
