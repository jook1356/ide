# ide self-update (docs/self-update.md): pull, build, wait for the user's restart, swap, start the
# new app, roll back when it does not come up. Started detached by the app (ide-app/src/self_update.rs)
# from a copy in <data>\self-update\, so a pull that rewrites this file does not change a running run.
#
# Windows PowerShell 5.1. status.json is UTF-8 without BOM, written atomically (tmp + move).

param(
    [Parameter(Mandatory)] [string] $RepoRoot,
    [Parameter(Mandatory)] [string] $AppDir,        # <target>\release
    [Parameter(Mandatory)] [string] $TargetDir,     # CARGO_TARGET_DIR
    [Parameter(Mandatory)] [string] $StatusFile,
    [Parameter(Mandatory)] [string] $LogFile,
    [Parameter(Mandatory)] [string] $ApplyFile,
    [Parameter(Mandatory)] [int] $AppPid,
    [Parameter(Mandatory)] [string] $RunId,
    [string] $AppExe = 'ide.exe',
    [string] $ServerExe = 'ide-server.exe',
    [string] $BuildCommand = 'cargo build --release -p ide-app -p ide-server',
    [switch] $SkipPull,
    [int] $QuitTimeoutSec = 60,
    [int] $HealthTimeoutSec = 120
)

$ErrorActionPreference = 'Stop'
$utf8 = New-Object System.Text.UTF8Encoding($false)
$from = ''
$to = ''

# The script must not keep the app folder as its cwd (it inherits the app's).
Set-Location -LiteralPath (Split-Path -Parent $StatusFile)

function Log([string] $line) {
    $stamp = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss')
    [IO.File]::AppendAllText($LogFile, "[$stamp] $line`r`n", $utf8)
}

function Status([string] $phase, [string] $message = '') {
    $o = [ordered]@{
        runId     = $RunId
        phase     = $phase
        at        = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        from      = $from
        to        = $to
        message   = $message
        scriptPid = $PID
    }
    $json = $o | ConvertTo-Json -Compress
    $tmp = "$StatusFile.tmp"
    [IO.File]::WriteAllText($tmp, $json, $utf8)
    Move-Item -LiteralPath $tmp -Destination $StatusFile -Force
    Log "phase $phase $message"
}

function LogTail {
    try { (Get-Content -LiteralPath $LogFile -Tail 30 -Encoding UTF8) -join "`n" } catch { '' }
}

function Fail([string] $why) {
    Status 'failed' ("$why`n" + (LogTail))
    exit 1
}

# Run a command line in $RepoRoot; its output goes to the log. Returns the exit code.
function Run([string] $commandLine) {
    Log "> $commandLine"
    $out = & cmd.exe /d /c "$commandLine 2>&1"
    $code = $LASTEXITCODE
    foreach ($l in $out) { Log "  $l" }
    return $code
}

$app = Join-Path $AppDir $AppExe
$server = Join-Path $AppDir $ServerExe
$appOld = "$app.$RunId.old"
$serverOld = "$server.$RunId.old"

# Remove leftovers of earlier runs (a daemon may still run from an old server copy: skip it).
Get-ChildItem -LiteralPath $AppDir -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '\.(old|failed)$' } | ForEach-Object {
    try { Remove-Item -LiteralPath $_.FullName -Force } catch { Log "kept $($_.Name) (in use)" }
}

try {
    # Always the original repository: a binary built in a worktree (an ide session's) names that
    # worktree; its common git dir belongs to the original checkout.
    $common = (& git -C $RepoRoot rev-parse --path-format=absolute --git-common-dir 2>$null)
    if ($LASTEXITCODE -eq 0 -and $common) {
        $main = Split-Path -Parent ([IO.Path]::GetFullPath($common.Trim()))
        if ((Test-Path -LiteralPath (Join-Path $main 'Cargo.toml')) -and $main -ne $RepoRoot) {
            Log "using the original repository $main (not $RepoRoot)"
            $RepoRoot = $main
        }
    }
    Set-Location -LiteralPath $RepoRoot
    # The environment of a fresh login (the app may have been started before tools were installed).
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')
    foreach ($k in 'CEF_PATH', 'WGPU_BACKEND') {
        $v = [Environment]::GetEnvironmentVariable($k, 'User')
        if ($v) { Set-Item -Path "env:$k" -Value $v }
    }
    $env:CARGO_TARGET_DIR = $TargetDir

    # Another worktree on the same branch (an ide session's) may have moved the branch under
    # this checkout: files that still equal an earlier commit of the branch (nothing unsaved)
    # are brought up to HEAD. Real local changes are left alone.
    & git diff --quiet HEAD 2>$null
    if ($LASTEXITCODE -ne 0) {
        foreach ($c in (& git rev-list --max-count=200 HEAD~1 2>$null)) {
            & git diff --quiet $c 2>$null
            if ($LASTEXITCODE -eq 0) {
                Log "checkout was at $c behind its branch: updating it to HEAD"
                Run 'git reset --hard HEAD' | Out-Null
                break
            }
        }
    }
    $from = (& git rev-parse --short HEAD 2>$null)
    if (-not $SkipPull) {
        Status 'pulling'
        # A pull that cannot fast-forward (local changes, conflicts, no upstream) is skipped:
        # the checkout is built as it is.
        if ((Run 'git pull --ff-only') -ne 0) { Log 'pull skipped: building the checkout as it is' }
    }
    $to = (& git rev-parse --short HEAD 2>$null)

    Status 'building'
    # A running exe cannot be overwritten but can be renamed: the running app and daemon keep
    # their (renamed) files, the build writes new ones.
    foreach ($p in @(@($app, $appOld), @($server, $serverOld))) {
        if (Test-Path -LiteralPath $p[0]) { Move-Item -LiteralPath $p[0] -Destination $p[1] -Force }
    }
    if ((Run $BuildCommand) -ne 0 -or -not (Test-Path -LiteralPath $app) -or -not (Test-Path -LiteralPath $server)) {
        foreach ($p in @(@($app, $appOld), @($server, $serverOld))) {
            if (Test-Path -LiteralPath $p[1]) {
                if (Test-Path -LiteralPath $p[0]) { Remove-Item -LiteralPath $p[0] -Force }
                Move-Item -LiteralPath $p[1] -Destination $p[0] -Force
            }
        }
        Fail 'build failed (the running ide is unchanged)'
    }
} catch {
    Fail "update error: $($_.Exception.Message)"
}

# Ready: wait for the user's restart (the app writes the apply file with this run id).
Status 'ready'
while ($true) {
    if (Test-Path -LiteralPath $ApplyFile) {
        $req = ([IO.File]::ReadAllText($ApplyFile)).Trim()
        if ($req -eq $RunId) { Remove-Item -LiteralPath $ApplyFile -Force; break }
    }
    if (-not (Get-Process -Id $AppPid -ErrorAction SilentlyContinue)) {
        # The app closed without restarting: the next start already runs the new build.
        Status 'done' 'closed-before-restart'
        exit 0
    }
    Start-Sleep -Milliseconds 500
}

# Swap: the app quits on seeing this phase; then the new one starts.
Status 'swapping'
$deadline = (Get-Date).AddSeconds($QuitTimeoutSec)
while ((Get-Process -Id $AppPid -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 300
}
if (Get-Process -Id $AppPid -ErrorAction SilentlyContinue) {
    Stop-Process -Id $AppPid -Force -ErrorAction SilentlyContinue
}
$new = Start-Process -FilePath $app -WorkingDirectory $AppDir -PassThru
Log "started the new app (pid $($new.Id))"

# The new app writes `done` with this run id once its window is up.
$deadline = (Get-Date).AddSeconds($HealthTimeoutSec)
while ((Get-Date) -lt $deadline) {
    try {
        $s = [IO.File]::ReadAllText($StatusFile).TrimStart([char]0xFEFF) | ConvertFrom-Json
        if ($s.runId -eq $RunId -and $s.phase -eq 'done') { Log 'the new app is up'; exit 0 }
    } catch {}
    if ($new.HasExited) { break }
    Start-Sleep -Milliseconds 500
}

# Roll back: the old app and server files come back and the old app starts again.
Log 'the new app did not come up: rolling back'
if (-not $new.HasExited) { Stop-Process -Id $new.Id -Force -ErrorAction SilentlyContinue }
Start-Sleep -Milliseconds 500
foreach ($p in @(@($app, $appOld), @($server, $serverOld))) {
    if (Test-Path -LiteralPath $p[1]) {
        if (Test-Path -LiteralPath $p[0]) { Move-Item -LiteralPath $p[0] -Destination "$($p[0]).$RunId.failed" -Force }
        Move-Item -LiteralPath $p[1] -Destination $p[0] -Force
    }
}
Start-Process -FilePath $app -WorkingDirectory $AppDir | Out-Null
Status 'rolledBack' ("the new build did not come up`n" + (LogTail))
exit 1
