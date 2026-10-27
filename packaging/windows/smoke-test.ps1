# Checks a built fmx.exe on Windows: it opens its window and draws
# (--self-test), and it adds and removes its right-click menu. Shows FMX's
# log, and fails with it when something doesn't work.
#   packaging/windows/smoke-test.ps1 target/release/fmx.exe
param([Parameter(Mandatory)][string]$Exe)
$ErrorActionPreference = 'Stop'

$logs = Join-Path $env:APPDATA 'fmx/logs'
function Show-Log {
    Get-Content (Join-Path $logs 'fmx.log') -ErrorAction SilentlyContinue
    Get-Content (Join-Path $logs 'crash-*.txt') -ErrorAction SilentlyContinue
}

# FMX has no console window, so its exit code is the result.
function Invoke-Fmx([string]$Arguments) {
    $run = Start-Process $Exe -ArgumentList $Arguments -PassThru -Wait
    if ($run.ExitCode -ne 0) {
        Show-Log
        throw "fmx.exe $Arguments exited with $($run.ExitCode)"
    }
}

Invoke-Fmx '--self-test'
Show-Log

$menu = 'HKCU:\Software\Classes\SystemFileAssociations\.mp4\shell\FMX.Convert'
Invoke-Fmx '--register-shell'
if (-not (Test-Path "$menu\shell\00\command")) {
    Show-Log
    throw 'the right-click menu was not added'
}
Get-ItemProperty $menu | Select-Object MUIVerb, Icon | Format-List
Get-ItemProperty "$menu\shell\00\command" | Select-Object '(default)' | Format-List

Invoke-Fmx '--unregister-shell'
if (Test-Path $menu) {
    Show-Log
    throw 'the right-click menu was not removed'
}
Write-Host 'FMX starts, and its right-click menu is added and removed.'
