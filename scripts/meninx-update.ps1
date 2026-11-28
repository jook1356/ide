# Meninx self-update (docs/self-update.md): fetch, build the branch, wait for the user's restart, swap, start the
# new app, roll back when it does not come up. Started detached by the app (meninx-app/src/self_update.rs)
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
    [string] $AppExe = 'meninx.exe',
    [string] $ServerExe = 'meninx-server.exe',
    [string] $BuildCommand = 'cargo build --release -p meninx-app -p meninx-server',
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

# Run a command line in the current folder; its output goes to the log. Returns the exit code.
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
    # The environment of a fresh login (the app may have been started before tools were installed).
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')
    foreach ($k in 'CEF_PATH', 'WGPU_BACKEND') {
        $v = [Environment]::GetEnvironmentVariable($k, 'User')
        if ($v) { Set-Item -Path "env:$k" -Value $v }
    }
    $env:CARGO_TARGET_DIR = $TargetDir

    # Always the original repository: a binary built in a worktree (this script's own build
    # checkout, or a Meninx session's) names that worktree; its common git dir is the original's.
    $common = (& git -C $RepoRoot rev-parse --path-format=absolute --git-common-dir 2>$null)
    if ($LASTEXITCODE -eq 0 -and $common) {
        $orig = Split-Path -Parent ([IO.Path]::GetFullPath($common.Trim()))
        if (Test-Path -LiteralPath (Join-Path $orig 'Cargo.toml')) { $RepoRoot = $orig }
    }
    Log "repository $RepoRoot"
    Set-Location -LiteralPath $RepoRoot

    # The commit to build: the branch as merged, never the original checkout's files (other
    # worktrees share its branch, so those files can lag behind it). origin/<branch> after a
    # fetch, or the local branch when it is ahead of it (merged, not pushed yet).
    $branch = (& git symbolic-ref --quiet --short HEAD 2>$null)
    if (-not $branch) { $branch = 'main' }
    $branch = $branch.Trim()
    $commit = $null
    if (-not $SkipPull) {
        Status 'pulling'
        if ((Run 'git fetch --quiet origin') -ne 0) { Log 'fetch failed: building the local branch' }
        $remote = (& git rev-parse --verify --quiet "refs/remotes/origin/$branch" 2>$null)
        if ($remote) {
            $commit = $remote.Trim()
            & git merge-base --is-ancestor "refs/remotes/origin/$branch" "refs/heads/$branch" 2>$null
            if ($LASTEXITCODE -eq 0) { $commit = (& git rev-parse "refs/heads/$branch").Trim() }
        }
    }
    if (-not $commit) { $commit = (& git rev-parse "refs/heads/$branch").Trim() }
    $to = (& git rev-parse --short $commit).Trim()
    $builtFile = Join-Path (Split-Path -Parent $StatusFile) 'built-commit'
    if (Test-Path -LiteralPath $builtFile) { $from = ([IO.File]::ReadAllText($builtFile)).Trim() }
    Log "building $branch at $to"

    # The build checkout: a worktree of the original repository next to the target dir (short
    # ASCII path), set to exactly that commit.
    $src = "$TargetDir-src"
    if (Test-Path -LiteralPath (Join-Path $src '.git')) {
        Set-Location -LiteralPath $src
        if ((Run "git checkout --quiet --force --detach $commit") -ne 0) { throw 'checkout of the build tree failed' }
        Run 'git clean -fdq' | Out-Null
    } else {
        Run 'git worktree prune' | Out-Null
        if (Test-Path -LiteralPath $src) { Remove-Item -LiteralPath $src -Recurse -Force }
        if ((Run "git worktree add --detach `"$src`" $commit") -ne 0) { throw 'creating the build tree failed' }
        Set-Location -LiteralPath $src
    }

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
        Fail 'build failed (the running Meninx is unchanged)'
    }
    [IO.File]::WriteAllText($builtFile, $to, $utf8)
    # The Linux servers for SSH installs (docs/remote-install.md): a failure only leaves them out.
    $remoteBuild = Join-Path $src 'scripts\build-remote-servers.ps1'
    if (Test-Path -LiteralPath $remoteBuild) {
        $code = Run "powershell -NoProfile -ExecutionPolicy Bypass -File `"$remoteBuild`" -AppDir `"$AppDir`" -TargetDir `"$TargetDir`""
        if ($code -ne 0) { Log "Linux servers not built (exit $code)" }
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
