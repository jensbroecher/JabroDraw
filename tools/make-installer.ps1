# Builds Jabro Draw and its installer: dist\Jabro-Draw-<version>-Setup.exe
# Run from the project folder:  powershell -ExecutionPolicy Bypass -File tools\make-installer.ps1
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'building Jabro Draw failed' }
cargo build --release -p jabro-setup
if ($LASTEXITCODE -ne 0) { throw 'building the setup failed' }
New-Item -ItemType Directory -Force dist | Out-Null
$out = "dist\Jabro-Draw-$version-Setup.exe"
Copy-Item target\release\jabro-draw-setup.exe $out -Force
Copy-Item tools\INSTALL.txt "dist\How to install Jabro Draw.txt" -Force
Write-Host "Done: $out ($([math]::Round((Get-Item $out).Length / 1MB)) MB)"
