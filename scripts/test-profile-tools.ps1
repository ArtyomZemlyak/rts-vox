$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/range-profile-tools.ps1"
function Assert-Profile($Condition, $Message) { if (-not $Condition) { throw $Message } }
$log = 'PROFILE tick_rate=30 state=Fighting
PROFILE adapter_name="GPU" adapter_backend=Vulkan adapter_device_type=DiscreteGpu
PROFILE state_hash=0123456789abcdef
PROFILE physics_workers=8 navigation_workers=8
PROFILE render_context_schema=2
PROFILE render_context=RenderContext { physical_size: [1600, 900], focused: true, view: 1 }
PROFILE render_context_step=0 value=RenderContext { physical_size: [1600, 900], focused: true, view: 1 }
PROFILE render_context_changed=false'
$rows = @([pscustomobject]@{frame_ms='4.5';simulation_ticks='30';seconds='1.0';frame_start_ns='0';frame_end_ns='1000000000';frame_gap_ns='0';observed_frame_ms='4.5'})
$valid = Get-RangeValidity $rows -Scenario world-battle -Log $log
Assert-Profile ($valid.ValidForComparison -and $valid.AcceptanceEligible -and $valid.MeetsSimulationRate) 'Good run rejected'
$busy = Get-RangeValidity $rows -Scenario world-battle -Log $log -Reasons @('competing_process_during_run')
Assert-Profile (-not $busy.ValidForComparison -and -not $busy.AcceptanceEligible) 'Contaminated run accepted'
$stopped = Get-RangeValidity @([pscustomobject]@{frame_ms='4.5';simulation_ticks='0';seconds='1.0';frame_start_ns='0';frame_end_ns='1000000000';frame_gap_ns='0';observed_frame_ms='4.5'}) -Scenario world-battle -Log $log
Assert-Profile ($stopped.Reasons -contains 'simulation_did_not_advance') 'Zero-tick fast frame accepted'
$slow = Get-RangeValidity @([pscustomobject]@{frame_ms='4.5';simulation_ticks='10';seconds='1.0';frame_start_ns='0';frame_end_ns='1000000000';frame_gap_ns='0';observed_frame_ms='4.5'}) -Scenario world-battle -Log $log
Assert-Profile ($slow.ValidForComparison -and -not $slow.MeetsSimulationRate -and -not $slow.AcceptanceEligible) 'Slow simulation accepted as real-time or its diagnostic evidence discarded'
$software = Get-RangeValidity $rows -Scenario world-battle -Log ($log.Replace('DiscreteGpu','Cpu'))
Assert-Profile ($software.Reasons -contains 'software_adapter') 'Software adapter accepted'
$diagnostic = Get-RangeValidity $rows -Scenario world-battle -Log $log -CorePhases
Assert-Profile ($diagnostic.ValidForComparison -and -not $diagnostic.AcceptanceEligible) 'Detailed trace used for FPS acceptance'
$trigger = Get-RangeValidity $rows -Scenario world-battle -Log $log -TriggerPhases
Assert-Profile ($trigger.ValidForComparison -and -not $trigger.AcceptanceEligible -and $trigger.Mode -eq 'trigger_diagnostic') 'Trigger trace used for FPS acceptance'
$renderLog = $log + "`nPROFILE render_timeline_schema=1 render_span_limit=100000 render_spans_started=7 render_spans_retained=7 render_spans_dropped=0 render_spans_incomplete=0 render_cutoff_ns=1000000001 render_pipelined=true"
$render = Get-RangeValidity $rows -Scenario world-battle -Log $renderLog -RenderTimeline
Assert-Profile ($render.ValidForComparison -and -not $render.AcceptanceEligible -and $render.Mode -eq 'render_diagnostic') 'Render timeline used for FPS acceptance'
$noRender = Get-RangeValidity $rows -Scenario world-battle -Log $log -RenderTimeline
Assert-Profile (-not $noRender.ValidForComparison -and $noRender.Reasons -contains 'render_timeline_metadata_missing') 'Missing render timeline metadata accepted'
$renderV2Log = $renderLog.Replace('render_timeline_schema=1 ', 'render_timeline_schema=2 ') + ' render_gas_details=true'
$renderV2 = Get-RangeValidity $rows -Scenario world-battle -Log $renderV2Log -RenderTimeline
Assert-Profile ($renderV2.ValidForComparison -and -not $renderV2.AcceptanceEligible) 'Gas detail schema used for FPS acceptance or rejected'
$noGasMode = Get-RangeValidity $rows -Scenario world-battle -Log ($renderLog.Replace('render_timeline_schema=1 ', 'render_timeline_schema=2 ')) -RenderTimeline
Assert-Profile ($noGasMode.Reasons -contains 'render_timeline_detail_mode_missing') 'Gas detail schema accepted without mode'
$renderV3 = Get-RangeValidity $rows -Scenario world-battle -Log ($renderV2Log.Replace('render_timeline_schema=2 ', 'render_timeline_schema=3 ')) -RenderTimeline
Assert-Profile ($renderV3.ValidForComparison -and -not $renderV3.AcceptanceEligible) 'Extract phase schema used for FPS acceptance or rejected'
$noPhaseMode = Get-RangeValidity $rows -Scenario world-battle -Log ($renderLog.Replace('render_timeline_schema=1 ', 'render_timeline_schema=3 ')) -RenderTimeline
Assert-Profile ($noPhaseMode.Reasons -contains 'render_timeline_detail_mode_missing') 'Extract phase schema accepted without mode'
$missing = Get-RangeValidity $rows -Scenario world-battle -Log ''
Assert-Profile ($missing.Reasons -contains 'state_hash_missing' -and $missing.Reasons -contains 'actual_adapter_missing') 'Unknown provenance accepted'
$nan = Get-RangeValidity @([pscustomobject]@{frame_ms='NaN';simulation_ticks='30';seconds='1.0'}) -Scenario world-battle -Log $log
Assert-Profile ($nan.Reasons -contains 'invalid_frame_time') 'NaN accepted'
$pipeline = @(
    [pscustomobject]@{frame_ms='4';simulation_ticks='1';seconds='0.001';frame_start_ns='0';frame_end_ns='30000000';frame_gap_ns='0';observed_frame_ms=''},
    [pscustomobject]@{frame_ms='4';simulation_ticks='0';seconds='0.032';frame_start_ns='32000000';frame_end_ns='33000000';frame_gap_ns='2000000';observed_frame_ms='32'},
    [pscustomobject]@{frame_ms='32';simulation_ticks='0';seconds='0.034';frame_start_ns='34000000';frame_end_ns='35000000';frame_gap_ns='1000000';observed_frame_ms='2'})
$timing = Get-RangeFrameTimes $pipeline
Assert-Profile ($timing.Reasons.Count -eq 0 -and $timing.Values[0] -eq 32 -and $timing.Values[1] -eq 2) 'Delayed Bevy time shifted measured frame stall'
$pipeline[1].observed_frame_ms = '4'
$mismatch = Get-RangeValidity $pipeline -Scenario world-battle -Log $log
Assert-Profile ($mismatch.Reasons -contains 'observed_frame_clock_mismatch' -and -not $mismatch.AcceptanceEligible) 'Contradictory timing passed FPS acceptance'
$legacy = Get-RangeValidity @([pscustomobject]@{frame_ms='4.5';simulation_ticks='30';seconds='1.0'}) -Scenario world-battle -Log $log
Assert-Profile ($legacy.Reasons -contains 'monotonic_frame_clock_missing' -and -not $legacy.AcceptanceEligible) 'Legacy Bevy-only timing passed FPS acceptance'
$changed = Get-RangeValidity $rows -Scenario world-battle -Log ($log.Replace('render_context_changed=false','render_context_changed=true'))
Assert-Profile ($changed.Reasons -contains 'render_context_changed_during_run' -and -not $changed.AcceptanceEligible) 'Resized/panned run passed FPS acceptance'
$noContext = Get-RangeValidity $rows -Scenario world-battle -Log ($log.Replace('render_context_schema=2','render_context_schema=0'))
Assert-Profile ($noContext.Reasons -contains 'render_context_missing' -and -not $noContext.ValidForComparison) 'Unknown render conditions accepted'
$noFocus = Get-RangeValidity $rows -Scenario world-battle -Log ($log.Replace('focused: true, ',''))
Assert-Profile ($noFocus.Reasons -contains 'render_focus_missing' -and -not $noFocus.ValidForComparison) 'Unknown focus accepted'
$restoredFocusLog = $log + "`nPROFILE render_context_step=3 value=RenderContext { focused: false }`nPROFILE render_context_step=4 value=RenderContext { focused: true }"
$restoredFocus = Get-RangeValidity $rows -Scenario world-battle -Log $restoredFocusLog
Assert-Profile ($restoredFocus.Reasons -contains 'render_focus_changed_during_run' -and -not $restoredFocus.ValidForComparison) 'Restored focus hid a transient change'
$oldContext = Get-RangeValidity $rows -Scenario world-battle -Log ($log.Replace('render_context_schema=2','render_context_schema=1'))
Assert-Profile ($oldContext.Reasons -contains 'render_context_missing' -and -not $oldContext.ValidForComparison) 'Legacy context accepted'
foreach ($file in 'profile-range.ps1','profile-cpu-threads.ps1','range-profile-tools.ps1') {
    $errors = $null; $tokens = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $file), [ref]$tokens, [ref]$errors)
    Assert-Profile ($errors.Count -eq 0) "Invalid PowerShell syntax: $file $errors"
}
'PASS: 23 validity/timing/context cases and parser checks.'
