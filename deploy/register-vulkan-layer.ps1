[CmdletBinding(SupportsShouldProcess)]
param(
    [string]$Runtime,
    [switch]$Uninstall,
    [switch]$SkipX86
)
$ErrorActionPreference = 'Stop'
if (-not $Runtime) {
    $Runtime = if (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'hooks')) { $PSScriptRoot } else { Join-Path $PSScriptRoot '../dist/capture-runtime' }
}
$Runtime = [IO.Path]::GetFullPath($Runtime)
$entries = @(@{ Arch = 'x64'; Key = 'Software\Khronos\Vulkan\ImplicitLayers' })
if (-not $SkipX86) { $entries += @{ Arch = 'x86'; Key = 'Software\WOW6432Node\Khronos\Vulkan\ImplicitLayers' } }
# Validate the complete package before modifying either registry location.
foreach ($entry in $entries) {
    $entry.Manifest = Join-Path $Runtime "hooks/$($entry.Arch)/relay-vk-layer.json"
    if (-not $Uninstall) {
        $manifest = Get-Content -LiteralPath $entry.Manifest -Raw | ConvertFrom-Json
        if ($manifest.layer.name -ne 'VK_LAYER_RELAY_capture') { throw "Manifest non valido: $($entry.Manifest)" }
        if (-not (Test-Path -LiteralPath (Join-Path $Runtime "hooks/$($entry.Arch)/relay_vk_layer.dll"))) { throw "DLL Vulkan $($entry.Arch) mancante" }
    }
}
$base = [Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::CurrentUser, [Microsoft.Win32.RegistryView]::Registry64)
try {
    foreach ($entry in $entries) {
        $action = if ($Uninstall) { 'Rimuovi registrazione Relay Vulkan' } else { 'Registra Relay Vulkan' }
        if ($PSCmdlet.ShouldProcess("HKCU\$($entry.Key): $($entry.Manifest)", $action)) {
            $key = if ($Uninstall) { $base.OpenSubKey($entry.Key, $true) } else { $base.CreateSubKey($entry.Key) }
            if ($null -ne $key) {
                try {
                    if ($Uninstall) { $key.DeleteValue($entry.Manifest, $false) }
                    else { $key.SetValue($entry.Manifest, 0, [Microsoft.Win32.RegistryValueKind]::DWord) }
                } finally { $key.Dispose() }
            }
        }
    }
} finally { $base.Dispose() }
Write-Output 'La modifica vale per i giochi avviati successivamente. HKCU non abilita il layer nei processi amministratore.'
