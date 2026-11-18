[CmdletBinding()]
param(
    [string]$MakeNsis = 'makensis'
)

$ErrorActionPreference = 'Stop'
$compiler = Get-Command $MakeNsis -ErrorAction Stop
$root = Split-Path $PSScriptRoot -Parent
$id = [guid]::NewGuid().ToString('N')
$sandbox = Join-Path ([System.IO.Path]::GetTempPath()) "AIAllowanceInstallerTest-$id"
$registryPath = "Software\AIAllowance\InstallerTests\$id"
$registry = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
    [Microsoft.Win32.RegistryHive]::CurrentUser,
    [Microsoft.Win32.RegistryView]::Registry64
)
$installDir = Join-Path $sandbox 'app'
$setup = Join-Path $sandbox 'ai-allowance-test-setup.exe'
$testMutex = "Local\AIAllowance.InstallerTest.$id"

function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

function Invoke-Setup {
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process $setup -ArgumentList '/S' -Wait -PassThru
    $timer.Stop()
    Assert-True ($process.ExitCode -eq 0) "Install failed: $($process.ExitCode)"
    Write-Output ('Silent install: {0:F2} seconds' -f $timer.Elapsed.TotalSeconds)
}

function Invoke-Uninstall([int]$ExpectedCode = 0) {
    # Avoid NSIS's detached temporary uninstaller so Wait observes the actual result.
    $copy = Join-Path $sandbox 'uninstall-test.exe'
    Copy-Item (Join-Path $installDir 'Uninstall.exe') $copy -Force
    $process = Start-Process $copy -ArgumentList "/S _?=$installDir" -Wait -PassThru
    Assert-True ($process.ExitCode -eq $ExpectedCode) "Uninstall returned $($process.ExitCode), expected $ExpectedCode"
}

try {
    New-Item -ItemType Directory $sandbox | Out-Null
    $source = Get-Content (Join-Path $PSScriptRoot 'ai-allowance.nsi') -Raw
    # Exercise the production script, substituting only its external resource locations.
    $replacements = [ordered]@{
        'Software\Microsoft\Windows\CurrentVersion\Run' = "$registryPath\Run"
        'Software\Microsoft\Windows\CurrentVersion\Uninstall\AIAllowance' = "$registryPath\Uninstall"
        '$SMPROGRAMS\AI Allowance' = "$sandbox\shortcuts"
        '$LOCALAPPDATA\Programs\AI Allowance' = $installDir
        'Local\AIAllowance.SingleInstance' = $testMutex
        '"..\' = "`"$root\"
    }
    foreach ($entry in $replacements.GetEnumerator()) {
        Assert-True ($source.Contains($entry.Key)) "Missing isolation substitution: $($entry.Key)"
        $source = $source.Replace($entry.Key, $entry.Value)
    }
    $script = Join-Path $sandbox 'test.nsi'
    Set-Content $script $source -Encoding utf8
    & $compiler.Source /V2 /WX /DVERSION=0.1.0 "/DOUTPUT_FILE=$setup" $script
    Assert-True ($LASTEXITCODE -eq 0) 'Test installer compilation failed.'

    $mutex = [System.Threading.Mutex]::new($false, $testMutex)
    try {
        $process = Start-Process $setup -ArgumentList '/S' -Wait -PassThru
        Assert-True ($process.ExitCode -eq 1) 'Installer did not reject a running tray instance.'
        Assert-True (-not (Test-Path $installDir)) 'Blocked install wrote application files.'
    }
    finally { $mutex.Dispose() }

    $run = $registry.CreateSubKey("$registryPath\Run")
    try {
        $run.SetValue('UnrelatedApp', 'preserve me')
        Invoke-Setup
        $expected = '"' + (Join-Path $installDir 'ai-allowance.exe') + '"'
        Assert-True ($run.GetValue('AIAllowance') -ceq $expected) 'Default startup path is not correctly quoted.'
        Assert-True ($run.GetValueKind('AIAllowance') -eq [Microsoft.Win32.RegistryValueKind]::String) 'Startup must be REG_SZ.'
        $files = @(Get-ChildItem -LiteralPath $installDir -Recurse -File -Force)
        $unexpected = @($files | Where-Object Name -NotIn @('ai-allowance.exe', 'Uninstall.exe', 'LICENSE', 'README.md'))
        Assert-True ($files.Count -eq 4 -and $unexpected.Count -eq 0) 'Fresh setup installed unexpected files.'
        $prebuilt = Join-Path $root 'target\installer-build\release\ai-allowance.exe'
        Assert-True ((Get-FileHash "$installDir\ai-allowance.exe").Hash -eq (Get-FileHash $prebuilt).Hash) 'Installed executable differs from the prebuilt release.'
        $config = Join-Path $installDir 'config.toml'
        Set-Content $config '# preserve preferences'
        Set-Content (Join-Path $installDir 'unrelated.txt') 'preserve file'

        # A malformed value must stop uninstall, not silently leave a broken startup entry.
        $run.SetValue('AIAllowance', 42, [Microsoft.Win32.RegistryValueKind]::DWord)
        Invoke-Uninstall 1
        Assert-True (Test-Path "$installDir\ai-allowance.exe") 'Failed registry check removed the executable.'
        $run.SetValue('AIAllowance', $expected, [Microsoft.Win32.RegistryValueKind]::String)
        Invoke-Uninstall
        Assert-True ($null -eq $run.GetValue('AIAllowance')) 'Owned startup value survived uninstall.'
        Assert-True ($run.GetValue('UnrelatedApp') -ceq 'preserve me') 'Uninstall modified another startup value.'
        Assert-True (Test-Path $config) 'Uninstall removed preferences.'
        Assert-True (Test-Path "$installDir\unrelated.txt") 'Uninstall removed an unknown file.'
        Assert-True (-not (Test-Path "$installDir\ai-allowance.exe")) 'Uninstall left the executable.'
        $uninstall = $registry.OpenSubKey("$registryPath\Uninstall")
        try { Assert-True ($null -eq $uninstall) 'Uninstall registration survived removal.' }
        finally { if ($null -ne $uninstall) { $uninstall.Dispose() } }

        Invoke-Setup
        $run.SetValue('AIAllowance', '"C:\Other Install\ai-allowance.exe"')
        Invoke-Uninstall
        Assert-True ($run.GetValue('AIAllowance') -ceq '"C:\Other Install\ai-allowance.exe"') 'Uninstall removed a different installation startup entry.'

        Invoke-Setup
        $run.DeleteValue('AIAllowance')
        Invoke-Uninstall
        Assert-True ($run.GetValue('UnrelatedApp') -ceq 'preserve me') 'Uninstall with startup disabled modified another value.'
    }
    finally { $run.Dispose() }
    Write-Output 'Installer smoke tests passed. Real startup keys and Start menu were not modified.'
}
finally {
    $registry.DeleteSubKeyTree($registryPath, $false)
    $registry.Dispose()
    if (Test-Path $sandbox) {
        Remove-Item -LiteralPath $sandbox -Recurse -Force
    }
}
