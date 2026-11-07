<#
.SYNOPSIS
  Download the C/C++ dependencies of viewers\vitalstream\cpp/ into viewers\vitalstream\cpp\deps\ (Windows counterpart of
  tools/fetch-cpp-deps.sh), checking each archive's SHA-256 as pinned in viewers\vitalstream\cpp\cmake\deps.cmake.
  A build then needs no internet access.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\fetch-cpp-deps.ps1
#>
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Deps = Join-Path $Root 'viewers\vitalstream\cpp\deps'
New-Item -ItemType Directory -Force $Deps | Out-Null
$text = (Get-Content (Join-Path $Root 'viewers\vitalstream\cpp\cmake\deps.cmake') -Raw) -replace '\s+', ' '
$found = [regex]::Matches($text, 'vs_dependency\([a-z0-9_]+ +(\S+) +(https://\S+) +([0-9a-f]{64})')
foreach ($m in $found) {
  $file, $url, $hash = $m.Groups[1].Value, $m.Groups[2].Value, $m.Groups[3].Value
  $out = Join-Path $Deps $file
  if ((Test-Path $out) -and ((Get-FileHash $out -Algorithm SHA256).Hash.ToLower() -eq $hash)) {
    Write-Host "ok        $file"; continue
  }
  Write-Host "download  $file"
  Invoke-WebRequest -UseBasicParsing $url -OutFile "$out.part"
  $got = (Get-FileHash "$out.part" -Algorithm SHA256).Hash.ToLower()
  if ($got -ne $hash) { Remove-Item "$out.part"; throw "SHA-256 mismatch for ${file}: expected $hash, got $got" }
  Move-Item -Force "$out.part" $out
}
Write-Host "viewers\vitalstream\cpp\deps\ is complete: builds now work without internet access."
