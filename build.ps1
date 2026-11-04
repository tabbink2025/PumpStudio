$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot
$taskCargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $taskCargo) {
    $taskRustBin = Join-Path $env:USERPROFILE '.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin'
    if (-not (Test-Path -LiteralPath (Join-Path $taskRustBin 'cargo.exe'))) {
        throw 'Rust ontbreekt. Installeer Rust en Visual C++ build tools om zelf te bouwen.'
    }
    $env:PATH = $taskRustBin + ';' + $env:PATH
}
cargo test --locked
if ($LASTEXITCODE -ne 0) { throw 'Rust-tests mislukt' }
cargo build --release --locked
if ($LASTEXITCODE -ne 0) { throw 'Rust-build mislukt' }
Copy-Item -LiteralPath 'target\release\pump-studio.exe' -Destination 'pump-studio.exe' -Force
Write-Host 'Klaar: pump-studio.exe. Bewaar map bridge naast programma.'
