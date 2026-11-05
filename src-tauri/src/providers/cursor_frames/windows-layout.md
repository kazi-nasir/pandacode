# Native Windows install layout (recorded)

CLI version `2026.09.15-d2fe57e`, Windows 11 (10.0.26300), recorded 2026-11-02. The user's profile path is written as `%LOCALAPPDATA%`.

## PATH

`where.exe agent` and `where.exe cursor-agent`:

```
%LOCALAPPDATA%\cursor-agent\agent.cmd
%LOCALAPPDATA%\cursor-agent\cursor-agent.cmd
```

PowerShell's `Get-Command` also finds `agent.ps1` and `cursor-agent.ps1` (as ExternalScript); `where.exe` does not list them.

## Files in `%LOCALAPPDATA%\cursor-agent\`

```
agent.cmd
agent.ps1
cursor-agent.cmd
cursor-agent.ps1
versions\
  2026.09.10-fd3934a\   node.exe: yes, index.js: yes
  2026.09.15-d2fe57e\   node.exe: yes, index.js: yes
```

There is no `node.exe` beside the shims, so the script's first branch is not taken on this install. Two versions are kept after a self-update; the newest by date is the one the script runs.

## `agent.cmd` (identical to `cursor-agent.cmd`)

```bat
@echo off
setlocal enabledelayedexpansion
set "CURSOR_INVOKED_AS=%~nx0"

REM Get the directory of this script
set "SCRIPT_DIR=%~dp0"
REM Remove trailing backslash
if "%SCRIPT_DIR:~-1%"=="\" set "SCRIPT_DIR=%SCRIPT_DIR:~0,-1%"

%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%\cursor-agent.ps1" %*
```

Both `.cmd` files run `cursor-agent.ps1`, never `agent.ps1`.

## `cursor-agent.ps1` (identical to `agent.ps1`)

```powershell
if (-not $env:CURSOR_INVOKED_AS) {
    $env:CURSOR_INVOKED_AS = Split-Path -leaf $MyInvocation.MyCommand.Name
}
$scriptPath = Split-Path -parent $MyInvocation.MyCommand.Definition

function Parse-VersionString {
    param (
        [string]$versionString
    )

    # Split on '-' to remove the commit hash part
    $datePart = $versionString.Split('-')[0]

    # Split on '.' to get year, month, day
    $parts = $datePart.Split('.')

    # Ensure we have exactly 3 parts (year, month, day)
    if ($parts.Length -ne 3) {
        throw "Invalid version format. Expected format: YYYY.MM.DD-commit"
    }

    # Pad month and day to 2 digits if needed, then concatenate
    $year = $parts[0]
    $month = $parts[1].PadLeft(2, '0')
    $day = $parts[2].PadLeft(2, '0')

    # Return as integer for proper numeric comparison
    return [int]($year + $month + $day)
}

## Enable Node.js compile cache for faster CLI startup (requires Node.js >= 22.1.0)
## Cache is automatically invalidated when source files change
if (-not $env:NODE_COMPILE_CACHE) {
    $env:NODE_COMPILE_CACHE = "$env:LOCALAPPDATA\cursor-compile-cache"
}

## Are we somehow in the same dir as the script? Just run it. Look for node.exe
if (Test-Path "$scriptPath\node.exe") {
    & "$scriptPath\node.exe" "$scriptPath\index.js" $args
    exit $LASTEXITCODE
}

## Otherwise, we're in a version directory. Find the latest version.
$versionDir = Get-ChildItem -Path "$scriptPath\versions" -Directory |
    Where-Object {
        $name = $_.Name
        # Check if directory name matches version format. Supports both the
        # legacy YYYY.MM.DD-commit form and the newer
        # YYYY.MM.DD-HH-MM-SS-commit form that adds a build timestamp between
        # the date and the commit hash.
        $name -match '^\d{4}\.\d{1,2}\.\d{1,2}(-\d{2}-\d{2}-\d{2})?-[a-f0-9]+$'
    } |
    Sort-Object { Parse-VersionString $_.Name } -Descending |
    Select-Object -First 1

if (-not $versionDir) {
    Write-Error "No version directories found in $scriptPath"
    exit 1
}

$versionName = $versionDir.Name
$nodePath = "$scriptPath\versions\$versionName\node.exe"

## Run the version's index.js
& "$nodePath" "$scriptPath\versions\$versionName\index.js" $args
exit $LASTEXITCODE
```

## Direct launch

`versions\2026.09.15-d2fe57e\node.exe versions\2026.09.15-d2fe57e\index.js acp`, started without `cmd.exe` or PowerShell and without `CURSOR_INVOKED_AS` or `NODE_COMPILE_CACHE`, answered `initialize` (`initialize.jsonl` was recorded this way) and ran the full session (`session.jsonl`, `resume.jsonl`). The same direct form printed `--version` (`2026.09.15-d2fe57e`), `--help` and `models`.
