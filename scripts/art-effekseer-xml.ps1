param(
    [Parameter(Mandatory=$true)][string]$InputFile,
    [Parameter(Mandatory=$true)][string]$OutputFile,
    [string]$Tools = (Join-Path $env:USERPROFILE 'Tools/rts-vox-art')
)

$ErrorActionPreference = 'Stop'
$taskCore = Join-Path $Tools 'effekseer-1.80.7/Effekseer1.80.7Win/Tool/bin/EffekseerCore.dll'
[System.Reflection.Assembly]::LoadFrom($taskCore) | Out-Null
[Effekseer.Core]::Initialize('en')
if (-not [Effekseer.Core]::LoadFrom([IO.Path]::GetFullPath($InputFile))) {
    throw 'Effekseer refused the input project'
}
$taskDoc = [Effekseer.Core]::SaveAsXmlDocument([Effekseer.Core]::Root)
$taskDoc.Save([IO.Path]::GetFullPath($OutputFile))
Write-Output "Effekseer nodes: $($taskDoc.SelectNodes('//Node').Count)"
# This is a short-lived data probe, not the GUI host. Core.Dispose also calls
# SaveOption relative to the entry assembly (PowerShell); do not write there.
# Process exit releases the probe's managed resources.
