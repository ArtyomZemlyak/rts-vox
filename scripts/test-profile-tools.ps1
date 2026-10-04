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
$pressure = Get-RangeValidity $rows -Scenario world-battle -Log $log -OsPressure
Assert-Profile ($pressure.ValidForComparison -and -not $pressure.AcceptanceEligible -and $pressure.Mode -eq 'os_pressure_diagnostic') 'OS pressure capture accepted as FPS benchmark'
Assert-Profile ($valid.ValidForComparison -and $valid.AcceptanceEligible -and $valid.MeetsSimulationRate) 'Good run rejected'
$tourLog = $log + "`nPROFILE camera_tour_schema=1 camera_tour_clock=wall camera_tour_route=bases_pan_zoom_v1"
$tour = Get-RangeValidity $rows -Scenario world-battle -Log $tourLog -CameraTour
Assert-Profile $tour.ValidForComparison 'Declared camera tour rejected'
$undeclaredTour = Get-RangeValidity $rows -Scenario world-battle -Log $tourLog
Assert-Profile ($undeclaredTour.Reasons -contains 'camera_tour_mode_mismatch') 'Undeclared camera tour accepted'
$missingTour = Get-RangeValidity $rows -Scenario world-battle -Log $log -CameraTour
Assert-Profile ($missingTour.Reasons -contains 'camera_tour_mode_mismatch') 'Camera tour without runtime mode accepted'
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
Assert-Profile ((Get-RangeRenderSpanLimit -Seconds 5) -eq 100000 -and (Get-RangeRenderSpanLimit -Seconds 20) -eq 109200 -and (Get-RangeRenderSpanLimit -Seconds 120) -eq 629200 -and (Get-RangeRenderSpanLimit -Seconds 3600) -eq 1000000 -and (Get-RangeRenderSpanLimit -Seconds 120 -Requested 3) -eq 3) 'Duration budget diverged from native contract'
$renderV4Log = $renderV2Log.Replace('render_timeline_schema=2 ', 'render_timeline_schema=4 ').Replace('render_span_limit=100000 ', 'render_span_limit=629200 ') + ' render_span_bytes=256'
$renderV4 = Get-RangeValidity $rows -Scenario world-battle -Log $renderV4Log -RenderTimeline -RenderSpanLimit 629200
Assert-Profile ($renderV4.ValidForComparison -and -not $renderV4.AcceptanceEligible) 'Duration budget rejected or used for FPS acceptance'
$wrongBudget = Get-RangeValidity $rows -Scenario world-battle -Log $renderV4Log -RenderTimeline -RenderSpanLimit 100000
Assert-Profile ($wrongBudget.Reasons -contains 'render_timeline_budget_manifest_mismatch') 'Requested/actual trace budget mismatch hidden'
$truncatedLog = $renderV4Log.Replace('render_span_limit=629200 ', 'render_span_limit=7 ').Replace('render_spans_started=7 ', 'render_spans_started=8 ').Replace('render_spans_dropped=0 ', 'render_spans_dropped=1 ')
$truncated = Get-RangeValidity $rows -Scenario world-battle -Log $truncatedLog -RenderTimeline -RenderSpanLimit 7
Assert-Profile (-not $truncated.ValidForComparison -and $truncated.Reasons -contains 'render_timeline_truncated') 'Dropped late trace accepted for comparison'
$badCount = Get-RangeValidity $rows -Scenario world-battle -Log ($renderV4Log.Replace('render_spans_started=7 ', 'render_spans_started=8 ')) -RenderTimeline
Assert-Profile ($badCount.Reasons -contains 'render_timeline_budget_count_mismatch') 'Inconsistent trace counts accepted'
$badLimit = Get-RangeValidity $rows -Scenario world-battle -Log ($renderV4Log.Replace('render_span_limit=629200 ', 'render_span_limit=1000001 ')) -RenderTimeline
Assert-Profile ($badLimit.Reasons -contains 'render_timeline_budget_count_mismatch') 'Unbounded memory limit accepted'
$badBytes = Get-RangeValidity $rows -Scenario world-battle -Log ($renderV4Log.Replace('render_span_bytes=256', 'render_span_bytes=1025')) -RenderTimeline
Assert-Profile ($badBytes.Reasons -contains 'render_timeline_budget_count_mismatch') 'Unbounded record size accepted'
$noBytes = Get-RangeValidity $rows -Scenario world-battle -Log ($renderV4Log.Replace(' render_span_bytes=256', '')) -RenderTimeline
Assert-Profile ($noBytes.Reasons -contains 'render_timeline_budget_count_mismatch') 'New schema accepted without memory accounting'
$duplicateRender = Get-RangeValidity $rows -Scenario world-battle -Log ($renderV4Log + "`n" + $renderV4Log) -RenderTimeline
Assert-Profile ($duplicateRender.Reasons -contains 'render_timeline_metadata_missing') 'Duplicate render metadata accepted'
$taskLog = $renderV4Log + "`nPROFILE render_tasks_build=true`nPROFILE render_tasks_schema=1 render_task_limit=2000000 render_tasks_started=3 render_tasks_retained=3 render_tasks_dropped=0 render_tasks_incomplete=0 render_task_cutoff_ns=1000000001 render_task_names=2 render_task_excluded_names=1 render_task_bytes=224"
$tasks = Get-RangeValidity $rows -Scenario world-battle -Log $taskLog -RenderTimeline -RenderTasks
Assert-Profile ($tasks.ValidForComparison -and -not $tasks.AcceptanceEligible -and $tasks.Mode -eq 'render_task_diagnostic') 'Actual-task capture rejected or used for FPS acceptance'
$taskBuild = Get-RangeValidity $rows -Scenario world-battle -Log ($log + "`nPROFILE render_tasks_build=true")
Assert-Profile ($taskBuild.ValidForComparison -and -not $taskBuild.AcceptanceEligible) 'Feature-only build used for FPS acceptance'
$missingTasks = Get-RangeValidity $rows -Scenario world-battle -Log ($renderV4Log + "`nPROFILE render_tasks_build=true") -RenderTimeline -RenderTasks
Assert-Profile ($missingTasks.Reasons -contains 'render_tasks_metadata_missing') 'Missing tasks capture accepted'
$missingBuild = Get-RangeValidity $rows -Scenario world-battle -Log ($taskLog.Replace('render_tasks_build=true','render_tasks_build=false')) -RenderTimeline -RenderTasks
Assert-Profile ($missingBuild.Reasons -contains 'render_tasks_build_or_mode_missing') 'Non-instrumented build claimed task coverage'
$taskCounts = Get-RangeValidity $rows -Scenario world-battle -Log ($taskLog.Replace('render_tasks_started=3 ', 'render_tasks_started=4 ')) -RenderTimeline -RenderTasks
Assert-Profile ($taskCounts.Reasons -contains 'render_tasks_budget_count_mismatch') 'Task counts mismatch accepted'
$taskEmpty = Get-RangeValidity $rows -Scenario world-battle -Log ($taskLog.Replace('render_tasks_started=3 ', 'render_tasks_started=0 ').Replace('render_tasks_retained=3 ', 'render_tasks_retained=0 ')) -RenderTimeline -RenderTasks
Assert-Profile ($taskEmpty.Reasons -contains 'render_tasks_empty_capture') 'Empty task capture called evidence'
$taskOverflow = Get-RangeValidity $rows -Scenario world-battle -Log ($taskLog.Replace('render_tasks_started=3 ', 'render_tasks_started=2000001 ').Replace('render_tasks_retained=3 ', 'render_tasks_retained=2000000 ').Replace('render_tasks_dropped=0 ', 'render_tasks_dropped=1 ')) -RenderTimeline -RenderTasks
Assert-Profile ($taskOverflow.Reasons -contains 'render_tasks_truncated' -and -not $taskOverflow.ValidForComparison) 'Task overflow accepted for comparison'
$taskBytes = Get-RangeValidity $rows -Scenario world-battle -Log ($taskLog.Replace('render_task_bytes=224', 'render_task_bytes=1025')) -RenderTimeline -RenderTasks
Assert-Profile ($taskBytes.Reasons -contains 'render_tasks_budget_count_mismatch') 'Invalid task memory accounting accepted'
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
Assert-Profile ((Get-RangeTaskLogFilter '') -eq 'bevy_ecs::system::function_system=info') 'Empty task log filter not enabled'
Assert-Profile ((Get-RangeTaskLogFilter 'warn') -eq 'warn,bevy_ecs::system::function_system=info') 'Inherited warn silenced tasks'
Assert-Profile ((Get-RangeTaskLogFilter 'error,bevy_ecs::system::function_system=off') -eq 'error,bevy_ecs::system::function_system=off,bevy_ecs::system::function_system=info') 'Existing target filter not overridden'
Assert-Profile ((Get-RangeValidity $rows -Scenario world-battle -Log ($taskLog.TrimEnd() + ' render_task_stride=8') -RenderTimeline -RenderTasks).ValidForComparison) 'Declared whole-frame sampling rejected'
Assert-Profile (-not (Get-RangeValidity $rows -Scenario world-battle -Log ($taskLog.TrimEnd() + ' render_task_stride=2') -RenderTimeline -RenderTasks).ValidForComparison) 'Unknown task stride accepted'
$computeLog = $log + "`nPROFILE compute_workers=8`nPROFILE compute_override=Some(8)"
Assert-Profile ((Get-RangeValidity $rows -Scenario world-battle -Log $computeLog -ComputeWorkers 8).ValidForComparison) 'Actual compute override rejected'
Assert-Profile ((Get-RangeValidity $rows -Scenario world-battle -Log ($computeLog.Replace('compute_workers=8','compute_workers=4')) -ComputeWorkers 8).Reasons -contains 'compute_workers_override_mismatch') 'Ignored worker override accepted'
Assert-Profile ((Get-RangeValidity $rows -Scenario world-battle -Log ($computeLog.Replace('Some(8)','None')) -ComputeWorkers 8).Reasons -contains 'compute_workers_override_mismatch') 'Missing compute CLI identity accepted'
Assert-Profile ((Get-RangeValidity $rows -Scenario world-battle -Log $log -ComputeWorkers 8).Reasons -contains 'compute_workers_override_mismatch') 'Old frozen binary silently accepted override'
$evidenceTestBinary = Join-Path ([IO.Path]::GetTempPath()) ("rts-build-evidence-" + [guid]::NewGuid().ToString('N') + '.exe')
$evidenceTestPath = "$evidenceTestBinary.build.json"
try {
    Assert-Profile ($null -eq (Get-RangeBuildEvidence $evidenceTestBinary 'AA')) 'Legacy binary given invented build evidence'
    @{Schema=1; BinarySha256='AA'; RefreshedSources=@(@{Path='source.rs'; Sha256='BB'})} |
        ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $evidenceTestPath -Encoding utf8
    $evidence = Get-RangeBuildEvidence $evidenceTestBinary 'AA'
    Assert-Profile ($evidence.Evidence.RefreshedSources[0].Sha256 -eq 'BB') 'Build inputs lost in profile evidence'
    $rejected = $false
    try { $null = Get-RangeBuildEvidence $evidenceTestBinary 'CC' } catch { $rejected = $true }
    Assert-Profile $rejected 'Build evidence accepted for a different binary'
    @{Schema=2; BinarySha256='AA'} | ConvertTo-Json | Set-Content -LiteralPath $evidenceTestPath -Encoding utf8
    $rejected = $false
    try { $null = Get-RangeBuildEvidence $evidenceTestBinary 'AA' } catch { $rejected = $true }
    Assert-Profile $rejected 'Unknown build evidence schema accepted'
    'invalid JSON' | Set-Content -LiteralPath $evidenceTestPath -Encoding utf8
    $rejected = $false
    try { $null = Get-RangeBuildEvidence $evidenceTestBinary 'AA' } catch { $rejected = $true }
    Assert-Profile $rejected 'Broken build evidence accepted'
} finally {
    if (Test-Path -LiteralPath $evidenceTestPath) { Remove-Item -LiteralPath $evidenceTestPath }
}
$parserErrors = $null; $parserTokens = $null
$null = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'build-range-variant.ps1'), [ref]$parserTokens, [ref]$parserErrors)
Assert-Profile ($parserErrors.Count -eq 0) "Invalid variant builder syntax: $parserErrors"
'PASS: validity/timing/context/camera/budget/task/log-filter/compute/build-evidence cases and parser checks.'
