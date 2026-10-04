param(
    [string]$Package = 'render',
    [string]$Binary = 'blast',
    [Parameter(Mandatory)][string]$OutputBinary,
    # List every source restored/copied for this variant, including reverted
    # experiments. Copy-Item can preserve a timestamp older than Cargo's output.
    [Parameter(Mandatory)][string[]]$RefreshSources
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Push-Location $repo
try {
    $outputPath = if ([IO.Path]::IsPathRooted($OutputBinary)) {
        [IO.Path]::GetFullPath($OutputBinary)
    } else {
        [IO.Path]::GetFullPath((Join-Path $repo $OutputBinary))
    }
    $manifestPath = "$outputPath.build.json"
    if ((Test-Path -LiteralPath $outputPath) -or (Test-Path -LiteralPath $manifestPath)) {
        throw "Existing variant retained: $outputPath. Choose a new output path."
    }
    $inputs = @($RefreshSources | ForEach-Object {
        $path = (Resolve-Path -LiteralPath $_).Path
        if (-not $path.StartsWith("$repo\", [StringComparison]::OrdinalIgnoreCase)) {
            throw "Refresh source must be inside the repository: $path"
        }
        $item = Get-Item -LiteralPath $path
        if ($item.PSIsContainer) { throw "Expected source file: $path" }
        [pscustomobject]@{ Path = $path; Sha256 = (Get-FileHash -LiteralPath $path).Hash }
    })
    if ($inputs.Count -eq 0) { throw 'RefreshSources must contain at least one file.' }
    foreach ($inputFile in $inputs) {
        (Get-Item -LiteralPath $inputFile.Path).LastWriteTimeUtc = [DateTime]::UtcNow
    }
    $gitHead = & git rev-parse HEAD
    $sourceAtLaunch = @(& git status --short)
    & "$PSScriptRoot/cargo-local.ps1" build --release -p $Package --bin $Binary
    if ($LASTEXITCODE -ne 0) { throw "Build failed: $Package/$Binary" }
    foreach ($inputFile in $inputs) {
        if ((Get-FileHash -LiteralPath $inputFile.Path).Hash -ne $inputFile.Sha256) {
            throw "Source changed during build: $($inputFile.Path)"
        }
    }
    $builtPath = Join-Path $repo "target/release/$Binary.exe"
    $builtHash = (Get-FileHash -LiteralPath $builtPath).Hash
    New-Item -ItemType Directory -Path (Split-Path $outputPath -Parent) -Force | Out-Null
    Copy-Item -LiteralPath $builtPath -Destination $outputPath
    if ((Get-FileHash -LiteralPath $outputPath).Hash -ne $builtHash) { throw 'Binary copy hash mismatch.' }
    [ordered]@{
        Schema = 1
        GitHead = $gitHead
        SourceAtLaunch = $sourceAtLaunch
        Package = $Package
        Binary = $Binary
        RefreshedSources = $inputs
        BinaryPath = $outputPath
        BinarySha256 = $builtHash
        SourceProvenance = 'Listed sources had fresh timestamps and unchanged content through the build. Callers must list every copied/restored source; this is not a complete dependency or clean-checkout attestation.'
    } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $manifestPath -Encoding utf8
    Write-Output "Frozen variant: $outputPath SHA256=$builtHash"
} finally { Pop-Location }
