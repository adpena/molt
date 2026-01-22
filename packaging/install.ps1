Param(
  [string]$Version = "",
  [string]$Prefix = "",
  [switch]$NoPath
)

$RepoOwner = "adpena"
$RepoName = "molt"

if ([string]::IsNullOrWhiteSpace($Prefix)) {
  $Prefix = Join-Path $env:USERPROFILE ".molt"
}

if ([string]::IsNullOrWhiteSpace($Version)) {
  $latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$RepoOwner/$RepoName/releases/latest"
  $Version = $latest.tag_name -replace '^v',''
}
else {
  $Version = $Version -replace '^v',''
}

$arch = "x86_64"
$asset = "molt-$Version-windows-$arch.zip"
$url = "https://github.com/$RepoOwner/$RepoName/releases/download/v$Version/$asset"

$workdir = New-Item -ItemType Directory -Path ([IO.Path]::Combine([IO.Path]::GetTempPath(), [IO.Path]::GetRandomFileName()))
$zipPath = Join-Path $workdir $asset

Invoke-WebRequest -Uri $url -OutFile $zipPath

if (Test-Path $Prefix) {
  Remove-Item -Recurse -Force $Prefix
}
New-Item -ItemType Directory -Path $Prefix | Out-Null

Expand-Archive -Path $zipPath -DestinationPath $workdir
$bundle = Get-ChildItem -Path $workdir -Directory | Where-Object { $_.Name -like "molt-$Version*" } | Select-Object -First 1
if (-not $bundle) {
  Write-Error "Failed to locate extracted bundle"
  exit 1
}
Copy-Item -Path (Join-Path $bundle.FullName "*") -Destination $Prefix -Recurse

$binPath = Join-Path $Prefix "bin"
if (-not $NoPath) {
  $current = [Environment]::GetEnvironmentVariable("Path", "User")
  if ($current -notlike "*$binPath*") {
    [Environment]::SetEnvironmentVariable("Path", "$binPath;$current", "User")
    Write-Output "Updated user PATH"
  }
}

Write-Output "Molt installed to $Prefix"
Write-Output "Run: molt doctor"
