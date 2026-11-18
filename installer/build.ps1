[CmdletBinding()]
param(
    [string]$MakeNsis = 'makensis'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$compiler = Get-Command $MakeNsis -ErrorAction Stop
Push-Location $root
try {
    $metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed.' }
    $version = ($metadata.packages | Where-Object name -eq 'aiallowance').version
    if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Installer requires a three-part numeric version.' }

    if (-not (Test-Path -LiteralPath 'target\installer-build\release\ai-allowance.exe' -PathType Leaf)) {
        throw 'Prebuilt ai-allowance.exe is missing. Run the release build command in README.md before packaging.'
    }

    New-Item -ItemType Directory -Force target\installer | Out-Null
    & $compiler.Source /V3 /WX "/DVERSION=$version" installer\ai-allowance.nsi
    if ($LASTEXITCODE -ne 0) { throw 'NSIS compilation failed.' }
    Write-Output "Installer: $(Join-Path $root 'target\installer\ai-allowance-windows-x64-setup.exe')"
}
finally {
    Pop-Location
}
