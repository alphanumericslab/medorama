# The license terms and their acceptance, shared by the installers (dot-sourced, not run): the
# Windows counterpart of tools/license-accept.sh.
#   Confirm-License [-Accepted]   shows tools\license-notice.txt and asks the user to accept it;
#                                 stops the installer unless they type y or yes.
# Accepted without asking with -Accepted (the installers' -AcceptLicense) or
# MEDORAMA_ACCEPT_LICENSE=yes, for scripts and CI; without a console to ask and without either,
# the installer stops and says how to accept.  The acceptance is recorded in the user's settings
# folder (%APPDATA%\medorama\license-accepted): it is not asked again until the terms change.
$LicenseNotice = Join-Path $PSScriptRoot 'license-notice.txt'

function Confirm-License([switch]$Accepted) {
  $record = Join-Path $env:APPDATA 'medorama\license-accepted'
  $hash = (Get-FileHash -Algorithm SHA256 $LicenseNotice).Hash.ToLower()
  if ((Test-Path $record) -and ((Get-Content $record) -contains "notice_sha256=$hash")) {
    $date = (Get-Content $record | Where-Object { $_ -like 'date=*' }) -replace '^date=', ''
    Write-Host "License terms accepted on $date (LICENSE, COMMERCIAL.md)."
    return
  }
  Write-Host ''
  Get-Content $LicenseNotice | ForEach-Object { Write-Host $_ }
  Write-Host ''
  if ("$env:MEDORAMA_ACCEPT_LICENSE".ToLower() -in @('y', 'yes')) { $Accepted = $true }
  if ($Accepted) {
    $how = 'option'
    Write-Host 'Accepted with -AcceptLicense (or MEDORAMA_ACCEPT_LICENSE=yes).'
  } elseif ([Environment]::UserInteractive -and -not [Console]::IsInputRedirected) {
    $answer = Read-Host 'Do you accept these terms? Type y or yes to continue [no]'
    if ("$answer".Trim().ToLower() -notin @('y', 'yes')) {
      Write-Host 'Nothing was installed: the license terms were not accepted.'
      exit 1
    }
    $how = 'prompt'
  } else {
    Write-Host 'Nothing was installed: there is no console to ask whether you accept the license terms.'
    Write-Host 'Accept them with -AcceptLicense, or with MEDORAMA_ACCEPT_LICENSE=yes.'
    exit 1
  }
  $cargo = Join-Path $PSScriptRoot '..\medorama\Cargo.toml'
  $version = (Select-String -Path $cargo -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
  New-Item -ItemType Directory -Force -Path (Split-Path $record) | Out-Null
  @(
    'license=PolyForm-Noncommercial-1.0.0',
    "notice_sha256=$hash",
    "date=$((Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'))",
    "user=$env:USERNAME",
    "accepted_by=$how",
    "version=$version"
  ) | Set-Content -Encoding ascii $record
}
