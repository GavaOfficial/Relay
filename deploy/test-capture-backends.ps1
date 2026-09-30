[CmdletBinding()]
param(
    [string]$Runtime = 'dist/capture-runtime',
    [string]$BuildDir = 'target/dxvk',
    [ValidateSet('all','x64','x86')][string]$Architecture = 'all',
    [ValidateSet('all','dxgi','dx10','dx12','d3d9','vulkan')][string]$Backend = 'all'
)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$Runtime = [IO.Path]::GetFullPath((Join-Path $root $Runtime))
$BuildDir = [IO.Path]::GetFullPath((Join-Path $root $BuildDir))
$previousPath = $env:VK_LAYER_PATH
$previousLayers = $env:VK_INSTANCE_LAYERS
$previousDiagnostics = $env:RELAY_CAPTURE_DIAGNOSTICS
try {
    $env:RELAY_CAPTURE_DIAGNOSTICS = '1'
    $arches = if ($Architecture -eq 'all') { @('x64','x86') } else { @($Architecture) }
    $backends = if ($Backend -eq 'all') { @('dxgi','dx10','dx12','d3d9','vulkan') } else { @($Backend) }
    foreach ($arch in $arches) {
        $examples = if ($arch -eq 'x64') { Join-Path $BuildDir 'release/examples' } else { Join-Path $BuildDir 'i686-pc-windows-msvc/release/examples' }
        foreach ($backend in $backends) {
            $zoo = Join-Path $examples "relay-zoo-$backend.exe"
            if (-not (Test-Path -LiteralPath $zoo)) { throw "Esempio mancante: $zoo. Compilare relay-recorder --examples per $arch." }
            $api = if ($backend -in @('dxgi','dx10','dx12')) { 'dxgi' } else { $backend }
            $env:VK_LAYER_PATH = if ($backend -eq 'vulkan') { Join-Path $Runtime "hooks/$arch" } else { $null }
            $env:VK_INSTANCE_LAYERS = if ($backend -eq 'vulkan') { 'VK_LAYER_RELAY_capture' } else { $null }
            Write-Output "Prova $backend $arch"
            & python (Join-Path $root 'client/capture/recorder/tests/zoo/record.py') --runtime $Runtime --zoo $zoo --api $api
            if ($LASTEXITCODE -ne 0) { throw "Prova $backend $arch fallita" }
        }
    }
} finally {
    $env:VK_LAYER_PATH = $previousPath
    $env:VK_INSTANCE_LAYERS = $previousLayers
    $env:RELAY_CAPTURE_DIAGNOSTICS = $previousDiagnostics
}
