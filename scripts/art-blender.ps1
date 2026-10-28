param(
    [switch]$Render,
    [switch]$ExportOnly,
    [string]$OutDir = 'out/art-lab-401'
)

# Offline test asset only. An explicit normal Blender launch remains the user's choice.
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$taskBlender = Join-Path $env:USERPROFILE 'Tools/rts-vox-art/blender-5.2.3/blender-5.2.3-windows-x64/blender.exe'
if (-not (Test-Path -LiteralPath $taskBlender)) {
    throw "Blender 5.2.3 portable not found: $taskBlender"
}
$taskOut = [IO.Path]::GetFullPath((Join-Path $taskRoot $OutDir))
$taskProcess = [System.Diagnostics.Process]::GetCurrentProcess()
$taskPreviousPriority = $taskProcess.PriorityClass
$taskPreviousAffinity = $taskProcess.ProcessorAffinity
$taskProcess.PriorityClass = 'BelowNormal'
$taskProcess.ProcessorAffinity = [IntPtr]15
Push-Location -LiteralPath $taskRoot
try {
    if ($ExportOnly) {
        & $taskBlender --background (Join-Path $taskOut 'scout.blend') --threads 4 --python-exit-code 1 --python scripts/blender_rts_export.py -- --out (Join-Path $taskOut 'reexport-pack') --cell-mm 100 --core-size 2
    } else {
        if (-not (Test-Path -LiteralPath (Join-Path $taskOut 'scout-source.json'))) {
            & python scripts/art_pipeline.py --out $taskOut
            if ($LASTEXITCODE -ne 0) { throw 'Fixture generation failed' }
        }
        if (-not (Test-Path -LiteralPath (Join-Path $taskOut 'scout-vengi.glb'))) {
            & python scripts/art_lab_convert.py --out $taskOut
            if ($LASTEXITCODE -ne 0) { throw 'Converter checks failed' }
        }
        $taskArgs = @('--background', '--factory-startup', '--threads', '4', '--python-exit-code', '1', '--python', 'scripts/art_lab_blender.py', '--', '--out', $taskOut)
        if ($Render) { $taskArgs += '--render' }
        & $taskBlender @taskArgs
    }
    if ($LASTEXITCODE -ne 0) { throw "Blender failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
    $taskProcess.PriorityClass = $taskPreviousPriority
    $taskProcess.ProcessorAffinity = $taskPreviousAffinity
}
