# Builds the Windows release archive on the GPD and sends it to the host's incoming directory, where
# infra/hostRelease.sh checks and publishes it with the Mac's archives.
#
# The same shape as infra/releaseBuild.sh: vibememory-<version>-x86_64-pc-windows-msvc.tar.gz with
# vibememory.exe and vibememory-mcp.exe at its root, and <archive>.sha256 in sha256sum's format — one
# line, LF — which `sha256sum -c` in Git Bash reads. tar is the one in System32 (Windows 10 and
# later); ssh and scp are OpenSSH's, over the owner's `vibememory` alias.
#
#   powershell -ExecutionPolicy Bypass -File infra\releaseBuild.ps1 [-Alias vibememory] [-NoUpload]
param(
  [string]$Alias = 'vibememory',
  [switch]$NoUpload
)
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$target = 'x86_64-pc-windows-msvc'
$cache = Join-Path $env:USERPROFILE 'vibememory-releases'

$manifest = Get-Content (Join-Path $root 'Cargo.toml') -Raw
$section = [regex]::Match($manifest, '(?ms)^\[workspace\.package\]\s*(.*?)(^\[|\z)').Groups[1].Value
$version = [regex]::Match($section, '(?m)^version = "([^"]+)"').Groups[1].Value
if (-not $version) { throw 'The version is not in Cargo.toml' }

cargo build --release -p vibememory-cli -p vibememory-mcp --target $target --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$out = Join-Path $cache $version
New-Item -ItemType Directory -Force -Path $out | Out-Null
$staging = Join-Path ([System.IO.Path]::GetTempPath()) ("vibememory-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $staging | Out-Null
foreach ($binary in 'vibememory.exe', 'vibememory-mcp.exe') {
  Copy-Item (Join-Path $root "target\$target\release\$binary") $staging
}
$archive = "vibememory-$version-$target.tar.gz"
tar -czf (Join-Path $out $archive) -C $staging vibememory.exe vibememory-mcp.exe
if ($LASTEXITCODE -ne 0) { throw 'tar failed' }
Remove-Item -Recurse -Force $staging

$hash = (Get-FileHash -Algorithm SHA256 (Join-Path $out $archive)).Hash.ToLowerInvariant()
[System.IO.File]::WriteAllText((Join-Path $out "$archive.sha256"), "$hash  $archive`n", [System.Text.Encoding]::ASCII)
Write-Output "Built $archive"

if (-not $NoUpload) {
  ssh -n $Alias "mkdir -p releases/incoming/$version"
  if ($LASTEXITCODE -ne 0) { throw 'ssh failed' }
  scp (Join-Path $out $archive) (Join-Path $out "$archive.sha256") "${Alias}:releases/incoming/$version/"
  if ($LASTEXITCODE -ne 0) { throw 'scp failed' }
  Write-Output "Sent to ~/releases/incoming/$version/ - then ./infra/hostRelease.sh $version on the Mac"
}
