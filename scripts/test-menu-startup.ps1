param(
    [string]$BinaryPath = 'target/release/blast.exe',
    [ValidateRange(1, 10)][int]$Seconds = 3,
    [string]$WindowSize = '3840,2160'
)
# Deliberately omit --shots: captures keep the world camera active and used
# to hide the atmosphere startup panic in the ordinary desktop launcher.
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
$binary = if ([IO.Path]::IsPathRooted($BinaryPath)) { $BinaryPath } else { Join-Path $repoRoot $BinaryPath }
if (!(Test-Path -LiteralPath $binary)) { throw "Binary not found: $binary" }
$testRoot = Join-Path $repoRoot 'out/menu-startup-check'
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
$tag = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fff')
$stdout = Join-Path $testRoot ($tag + '.out.log')
$stderr = Join-Path $testRoot ($tag + '.err.log')
$profile = Join-Path $testRoot $tag
$menuArgs = @('--weapon', 'energy', '--chassis', 'tank', '--range', 'data/range-km.ron', '--drive',
    '--main-menu', '--profile', ('"{0}"' -f $profile), '--profile-seconds', $Seconds.ToString(),
    '--profile-background', '--window', $WindowSize, '--fps-limit', '30',
    '--compute-workers', '2', '--physics-workers', '2', '--nav-workers', '2')
$env:WGPU_BACKEND = 'vulkan'
$env:RAYON_NUM_THREADS = '2'
$env:RUST_BACKTRACE = '1'
$process = Start-Process -FilePath $binary -WorkingDirectory $repoRoot -ArgumentList $menuArgs -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
# Retain the handle before exit so Windows PowerShell also retains ExitCode.
$processHandle = $process.Handle
$process.PriorityClass = 'BelowNormal'
$process.ProcessorAffinity = 5
if (!$process.WaitForExit(180000)) {
    Stop-Process -Id $process.Id -ErrorAction SilentlyContinue
    throw "Menu startup timed out; inspect $stderr"
}
$process.Refresh()
if ($process.ExitCode -ne 0 -or (Get-Content -LiteralPath $stderr -Raw) -match 'panicked|Encountered a panic|Validation Error') {
    throw "Menu startup failed (exit $($process.ExitCode)); inspect $stderr"
}
Write-Output "PASS: ordinary main-menu rendering without --shots ($WindowSize)."
Write-Output "Logs: $stdout ; $stderr"
