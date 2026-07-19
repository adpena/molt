Param(
  [string]$Version = "",
  [string]$Prefix = "",
  [switch]$NoPath
)

$ErrorActionPreference = "Stop"
$RepoOwner = "adpena"
$RepoName = "molt"

if ([string]::IsNullOrWhiteSpace($Prefix)) {
  $Prefix = Join-Path $env:USERPROFILE ".molt"
}
$Prefix = [IO.Path]::GetFullPath($Prefix)
$PrefixParent = Split-Path -Parent $Prefix
if ([string]::IsNullOrWhiteSpace($PrefixParent)) {
  throw "Install prefix must have a parent directory: $Prefix"
}

if ([string]::IsNullOrWhiteSpace($Version)) {
  $latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$RepoOwner/$RepoName/releases/latest"
  $Version = $latest.tag_name -replace '^v',''
} else {
  $Version = $Version -replace '^v',''
}

$Architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
$arch = switch ($Architecture) {
  "X64" { "x86_64" }
  "Arm64" { "arm64" }
  default { throw "Unsupported Windows architecture: $Architecture" }
}
$asset = "molt-$Version-windows-$arch.zip"
$releaseRoot = "https://github.com/$RepoOwner/$RepoName/releases/download/v$Version"
$workdir = New-Item -ItemType Directory -Path ([IO.Path]::Combine([IO.Path]::GetTempPath(), [IO.Path]::GetRandomFileName()))

try {
  $zipPath = Join-Path $workdir $asset
  $checksumsPath = Join-Path $workdir "SHA256SUMS"
  Invoke-WebRequest -Uri "$releaseRoot/$asset" -OutFile $zipPath
  Invoke-WebRequest -Uri "$releaseRoot/SHA256SUMS" -OutFile $checksumsPath

  $escapedAsset = [Regex]::Escape($asset)
  $checksumLines = @(Get-Content -LiteralPath $checksumsPath -Encoding utf8 | Where-Object {
      $_ -match "^([0-9a-f]{64})  $escapedAsset$"
    })
  if ($checksumLines.Count -ne 1) {
    throw "SHA256SUMS must contain exactly one digest for $asset"
  }
  $expected = $checksumLines[0].Substring(0, 64).ToLowerInvariant()
  $actual = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($actual -ne $expected) {
    throw "Release digest mismatch for ${asset}: expected $expected, got $actual"
  }

  $extractRoot = Join-Path $workdir "extract"
  Expand-Archive -LiteralPath $zipPath -DestinationPath $extractRoot
  $bundles = @(Get-ChildItem -LiteralPath $extractRoot -Directory | Where-Object {
      $_.Name -eq "molt-$Version"
    })
  if ($bundles.Count -ne 1) {
    throw "Release archive must contain exactly one molt-$Version root"
  }

  New-Item -ItemType Directory -Path $PrefixParent -Force | Out-Null
  $staged = "$Prefix.new-$PID"
  $backup = "$Prefix.old-$PID"
  if (Test-Path -LiteralPath $staged) { Remove-Item -LiteralPath $staged -Recurse -Force }
  if (Test-Path -LiteralPath $backup) { Remove-Item -LiteralPath $backup -Recurse -Force }
  New-Item -ItemType Directory -Path $staged | Out-Null
  Copy-Item -Path (Join-Path $bundles[0].FullName "*") -Destination $staged -Recurse
  if (Test-Path -LiteralPath $Prefix) {
    Move-Item -LiteralPath $Prefix -Destination $backup
  }
  try {
    Move-Item -LiteralPath $staged -Destination $Prefix
  } catch {
    if (Test-Path -LiteralPath $backup) {
      Move-Item -LiteralPath $backup -Destination $Prefix
    }
    throw
  }
  if (Test-Path -LiteralPath $backup) {
    Remove-Item -LiteralPath $backup -Recurse -Force
  }

  $binPath = Join-Path $Prefix "bin"
  if (-not $NoPath) {
    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($current -notlike "*$binPath*") {
      [Environment]::SetEnvironmentVariable("Path", "$binPath;$current", "User")
      Write-Output "Updated user PATH"
    }
  }

  $moltCommand = Join-Path $binPath "molt.cmd"
  if (-not (Test-Path -LiteralPath $moltCommand)) {
    throw "Installed bundle is missing executable: $moltCommand"
  }
  Write-Output "Molt $Version installed to $Prefix from verified $arch release"
  & $moltCommand setup --strict
} finally {
  if (Test-Path -LiteralPath $workdir) {
    Remove-Item -LiteralPath $workdir -Recurse -Force
  }
}
