<#
    Install or remove the pickup patch.

    The patch is written against one exact build of logic.dll. A game update
    replaces that file, and every address and structure offset in the patch
    would have to be re-derived, so this refuses to install over a DLL it does
    not recognise rather than producing a crash on launch.

    The stock DLL is kept beside the target as logic.dll.stock and is never
    overwritten once taken, so -Revert always restores the shipped file.
    Verifying the installation through GOG Galaxy also restores it.
#>
[CmdletBinding()]
param(
    [string]$GameDir = 'C:\Games\GOG Galaxy\Terminator Dark Fate - Defiance',
    [string]$Manifest = (Join-Path $PSScriptRoot '..\out\manifest.json'),
    [switch]$Revert,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
$target = Join-Path $GameDir 'bin\logic.dll'
$stock = "$target.stock"

if (-not (Test-Path $target)) { throw "no logic.dll at $target" }
if (-not (Test-Path $Manifest)) { throw "no manifest at $Manifest; run tools\build.py first" }

$built = Get-Content $Manifest -Raw | ConvertFrom-Json
$patched = Join-Path (Split-Path $Manifest -Parent) 'logic.dll'
if (-not (Test-Path $patched)) { throw "no patched DLL at $patched" }

function Get-Sha($path) { (Get-FileHash -Algorithm SHA256 $path).Hash.ToLower() }

function Show-Dll($label, $path) {
    if (Test-Path $path) {
        Write-Host ("  {0,-9} {1} {2} bytes" -f $label, (Get-Sha $path), (Get-Item $path).Length)
    } else {
        Write-Host ("  {0,-9} (absent)" -f $label)
    }
}

# The game holds logic.dll open, so say so rather than failing on a locked file.
if (Get-Process -Name 'trm' -ErrorAction SilentlyContinue) {
    throw 'the game is running; close it first'
}

if ($Revert) {
    if (-not (Test-Path $stock)) { throw "no backup at $stock" }
    if ((Get-Sha $stock) -ne $built.source_sha256) {
        Write-Warning "the backup is not the build this patch was made from; restoring it anyway"
    }
    Copy-Item $stock $target -Force
    Write-Host 'Reverted to the shipped logic.dll.'
    Show-Dll 'installed' $target
    return
}

$current = Get-Sha $target
if ($current -eq $built.output_sha256) {
    Write-Host 'This patch is already installed.'
    Show-Dll 'stock' $stock
    Show-Dll 'installed' $target
    return
}

if ($current -ne $built.source_sha256 -and -not $Force) {
    Write-Host "The game's logic.dll is not the build this patch was made from." -ForegroundColor Yellow
    Write-Host "  in the game   $current"
    Write-Host "  patch built from $($built.source_sha256)"
    Write-Host ''
    Write-Host 'The game has most likely been updated. Installing anyway would put code'
    Write-Host 'built for the old layout into the new binary, which will crash. To rebuild:'
    Write-Host ''
    Write-Host '  1. copy the new bin\logic.dll over bin\gog\2025-12-23\logic.dll in this workspace'
    Write-Host '  2. re-derive the addresses and offsets listed in notes\pickup.md'
    Write-Host '  3. update EXPECT_SOURCE_SHA in tools\build.py, then rebuild and retest'
    Write-Host ''
    Write-Host 'Delete the stale logic.dll.stock as well, so a revert cannot restore an old DLL.'
    throw 'refusing to install over an unrecognised logic.dll (use -Force to override)'
}

if (-not (Test-Path $stock)) {
    Copy-Item $target $stock
    Write-Host "Backed up the shipped DLL to $stock"
} elseif ((Get-Sha $stock) -ne $built.source_sha256) {
    Write-Warning "$stock is not the build this patch was made from; leaving it as it is"
}

Copy-Item $patched $target -Force
Write-Host 'Installed the patched logic.dll.'
Show-Dll 'stock' $stock
Show-Dll 'installed' $target
Write-Host ''
Write-Host 'Revert with:  pwsh -File tools\install.ps1 -Revert'
