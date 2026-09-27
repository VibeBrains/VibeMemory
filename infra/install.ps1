# Installs VibeMemory on Windows in one line of PowerShell:
#
#   irm https://app.vibememory.ru/install.ps1 | iex
#
# or from cmd:
#
#   powershell -ExecutionPolicy Bypass -c "irm https://app.vibememory.ru/install.ps1 | iex"
#
# Takes the latest Windows release from the host, checks its SHA-256 against the sum the host
# publishes beside it, unpacks it into a temporary directory with the tar.exe of Windows 10 and later,
# and runs `vibememory.exe install` from there: both programs go to %USERPROFILE%\.vibememory\bin,
# which is put on the user's Path for new terminals. The temporary directory goes away. A sum that
# does not match installs nothing. $env:VIBEMEMORY_DL names another release directory, for a test.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$base = if ($env:VIBEMEMORY_DL) { $env:VIBEMEMORY_DL } else { 'https://app.vibememory.ru/dl/' }
$target = 'x86_64-pc-windows-gnu'

if (-not [Environment]::Is64BitOperatingSystem) {
  throw 'VibeMemory: only 64-bit Windows has a release'
}

# a text file may come back as bytes when the server calls it octet-stream: read it as text either way
function Get-Text([string]$uri) {
  $answer = Invoke-WebRequest -Uri $uri -UseBasicParsing
  if ($answer.Content -is [byte[]]) { return [System.Text.Encoding]::UTF8.GetString($answer.Content) }
  return [string]$answer.Content
}

$version = (Get-Text ($base + 'latest')).Trim()
if ($version -notmatch '^[0-9A-Za-z.-]+$') {
  throw 'VibeMemory: the host answered no version'
}
$archive = "vibememory-$version-$target.tar.gz"

$work = Join-Path ([System.IO.Path]::GetTempPath()) ('vibememory-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null
try {
  Write-Host "VibeMemory $version for $target"
  $file = Join-Path $work $archive
  Invoke-WebRequest -Uri "$base$version/$archive" -OutFile $file -UseBasicParsing
  $expected = ((Get-Text "$base$version/$archive.sha256").Trim() -split '\s+')[0]
  $actual = (Get-FileHash -Algorithm SHA256 -Path $file).Hash
  if (-not $expected -or $expected -ne $actual) {
    throw "VibeMemory: the SHA-256 of $archive does not match the published one: nothing is installed"
  }
  $unpacked = Join-Path $work 'unpacked'
  New-Item -ItemType Directory -Path $unpacked | Out-Null
  tar.exe -xzf $file -C $unpacked
  if ($LASTEXITCODE -ne 0) { throw 'VibeMemory: the archive could not be unpacked' }
  & (Join-Path $unpacked 'vibememory.exe') install
  if ($LASTEXITCODE -ne 0) { throw 'VibeMemory: install did not finish' }
} finally {
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
