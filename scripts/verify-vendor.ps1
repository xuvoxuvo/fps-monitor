# Verifies the vendored PresentMon binary against vendor/SHA256SUMS.
# Run from CI and before a release: build.rs already does this, but an
# independent check in the workflow makes a corrupted checkout impossible to
# turn into an artifact.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$manifest = Join-Path (Get-Location) "vendor\SHA256SUMS"
if (-not (Test-Path $manifest)) {
    throw "vendor\SHA256SUMS is missing"
}

$checked = 0
$failed = $false
foreach ($line in Get-Content $manifest) {
    $line = $line.Trim()
    if (-not $line -or $line.StartsWith("#")) { continue }
    $parts = $line -split "\s+", 2
    if ($parts.Count -ne 2) { throw "malformed line in vendor\SHA256SUMS: '$line'" }
    $expected = $parts[0].ToLowerInvariant()
    $name = $parts[1].TrimStart("*")
    $path = Join-Path "vendor" $name
    if (-not (Test-Path $path)) {
        Write-Host "MISSING  $name"
        $failed = $true
        continue
    }
    $actual = (Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        Write-Host "MISMATCH $name"
        Write-Host "  expected $expected"
        Write-Host "  actual   $actual"
        $failed = $true
    } else {
        Write-Host "ok       $name"
    }
    $checked++
}

if ($checked -eq 0) { throw "vendor\SHA256SUMS contains no entries" }
if ($failed) { throw "vendor verification failed ($checked files checked)" }
Write-Host "vendor verification passed ($checked files)"
