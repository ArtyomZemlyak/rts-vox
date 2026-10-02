$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/range-profile-tools.ps1"
function Assert-Profile($Condition, $Message) { if (-not $Condition) { throw $Message } }
$log = 'PROFILE tick_rate=30 state=Fighting
PROFILE adapter_name="GPU" adapter_backend=Vulkan adapter_device_type=DiscreteGpu
PROFILE state_hash=0123456789abcdef
PROFILE physics_workers=8 navigation_workers=8'
$rows = @([pscustomobject]@{frame_ms='4.5';simulation_ticks='30';seconds='1.0'})
$valid = Get-RangeValidity $rows -Scenario world-battle -Log $log
Assert-Profile ($valid.ValidForComparison -and $valid.AcceptanceEligible -and $valid.MeetsSimulationRate) 'Good run rejected'
$busy = Get-RangeValidity $rows -Scenario world-battle -Log $log -Reasons @('competing_process_during_run')
Assert-Profile (-not $busy.ValidForComparison -and -not $busy.AcceptanceEligible) 'Contaminated run accepted'
$stopped = Get-RangeValidity @([pscustomobject]@{frame_ms='4.5';simulation_ticks='0';seconds='1.0'}) -Scenario world-battle -Log $log
Assert-Profile ($stopped.Reasons -contains 'simulation_did_not_advance') 'Zero-tick fast frame accepted'
$slow = Get-RangeValidity @([pscustomobject]@{frame_ms='4.5';simulation_ticks='10';seconds='1.0'}) -Scenario world-battle -Log $log
Assert-Profile (-not $slow.MeetsSimulationRate) 'Slow simulation accepted as real-time'
$software = Get-RangeValidity $rows -Scenario world-battle -Log ($log.Replace('DiscreteGpu','Cpu'))
Assert-Profile ($software.Reasons -contains 'software_adapter') 'Software adapter accepted'
$diagnostic = Get-RangeValidity $rows -Scenario world-battle -Log $log -CorePhases
Assert-Profile ($diagnostic.ValidForComparison -and -not $diagnostic.AcceptanceEligible) 'Detailed trace used for FPS acceptance'
$missing = Get-RangeValidity $rows -Scenario world-battle -Log ''
Assert-Profile ($missing.Reasons -contains 'state_hash_missing' -and $missing.Reasons -contains 'actual_adapter_missing') 'Unknown provenance accepted'
$nan = Get-RangeValidity @([pscustomobject]@{frame_ms='NaN';simulation_ticks='30';seconds='1.0'}) -Scenario world-battle -Log $log
Assert-Profile ($nan.Reasons -contains 'invalid_frame_time') 'NaN accepted'
foreach ($file in 'profile-range.ps1','profile-cpu-threads.ps1','range-profile-tools.ps1') {
    $errors = $null; $tokens = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $file), [ref]$tokens, [ref]$errors)
    Assert-Profile ($errors.Count -eq 0) "Invalid PowerShell syntax: $file $errors"
}
'PASS: 8 validity cases and parser checks.'
