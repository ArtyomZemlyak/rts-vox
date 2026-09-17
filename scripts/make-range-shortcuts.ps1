# Desktop shortcuts to the range window: "Полигон" runs the built window
# (building it first only when it is missing), "Полигон — собрать и
# запустить" rebuilds it and runs. Rerunning rewrites the links. The file
# carries a UTF-8 BOM so that Windows PowerShell 5.1 reads the names right.
# WScript.Shell cannot save a link under a non-ANSI name, so each link is
# saved under a Latin name and renamed.
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$desktop = [Environment]::GetFolderPath('Desktop')
$cmd = Join-Path $PSScriptRoot 'play-range.cmd'
$shell = New-Object -ComObject WScript.Shell
foreach ($link in @(
    @{ Name = 'Полигон.lnk'; Tag = 'run'; Args = ''; Note = 'rts-vox range: run the built window' },
    @{ Name = 'Полигон — собрать и запустить.lnk'; Tag = 'build'; Args = '-Build'; Note = 'rts-vox range: rebuild, then run' }
)) {
    $tmp = Join-Path $desktop ('rts-vox-range-{0}.lnk' -f $link.Tag)
    $s = $shell.CreateShortcut($tmp)
    $s.TargetPath = $cmd
    $s.Arguments = $link.Args
    $s.WorkingDirectory = $repo
    $s.Description = $link.Note
    $s.IconLocation = '%SystemRoot%\System32\shell32.dll,76'
    $s.Save()
    $path = Join-Path $desktop $link.Name
    Move-Item -LiteralPath $tmp -Destination $path -Force
    $back = $shell.CreateShortcut($tmp)
    '{0} -> {1} {2}' -f $link.Name, $s.TargetPath, $s.Arguments
}
