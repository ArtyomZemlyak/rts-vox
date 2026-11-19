# Run in a child interpreter: cargo-local.ps1 exits its own PowerShell host.
$ErrorActionPreference = 'Stop'
& python (Join-Path $PSScriptRoot 'test-fast.py') @args
exit $LASTEXITCODE
