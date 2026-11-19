# Compila OptimizedShell (Rust + windows-sys) optimizado para el menor tamaño/uso
# de recursos. Requiere la toolchain de Rust con el target x86_64-pc-windows-msvc.
#
#   .\build.ps1            # compila en release
#   .\build.ps1 -Run       # compila y lanza el shell (barra de tareas)
param([switch]$Run)
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "Compilación fallida ($LASTEXITCODE)" }
    $exe = Join-Path $PSScriptRoot 'target\x86_64-pc-windows-msvc\release\OptimizedShell.exe'
    Write-Host "Compilado: $exe"
    if ($Run) { & $exe }
}
finally { Pop-Location }
