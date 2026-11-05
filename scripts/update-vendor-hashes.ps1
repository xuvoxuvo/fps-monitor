# Rewrites vendor/SHA256SUMS after intentionally updating a vendored binary.
# Never use this to silence a verification failure caused by a corrupted file.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$files = Get-ChildItem "vendor" -File |
    Where-Object { $_.Name -notin @("SHA256SUMS", "README.md") } |
    Sort-Object Name

if ($files.Count -eq 0) { throw "no files found in vendor\" }

$lines = foreach ($file in $files) {
    $hash = (Get-FileHash $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    "{0}  {1}" -f $hash, $file.Name
}

$target = Join-Path "vendor" "SHA256SUMS"
$lines | Set-Content $target -Encoding ascii
Write-Host "Wrote $target with $($files.Count) entries:"
$lines | ForEach-Object { Write-Host "  $_" }
