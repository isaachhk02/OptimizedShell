# Desinstalador del entorno OptimizedShell.
#
# Devuelve el shell del usuario al Explorador de Windows, cierra las ventanas
# del entorno, y quita los accesos, las claves del registro y los archivos.
# Los ajustes de %APPDATA%\OptimizedShell se conservan salvo que se pida
# -BorrarAjustes.
#
#   .\uninstall.ps1
#   .\uninstall.ps1 -BorrarAjustes

[CmdletBinding()]
param(
    [string]$Dir,
    [switch]$BorrarAjustes
)

$ErrorActionPreference = 'Stop'

# ----------------------------------------------------------------- elevación
$admin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
         ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
    Write-Host 'Hace falta permiso de administrador: pidiendo elevación...'
    $argsList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"")
    foreach ($kv in $PSBoundParameters.GetEnumerator()) {
        if ($kv.Value -is [switch]) {
            if ($kv.Value.IsPresent) { $argsList += "-$($kv.Key)" }
        } else {
            $argsList += @("-$($kv.Key)", "`"$($kv.Value)`"")
        }
    }
    Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList $argsList -Verb RunAs
    return
}

$uninstKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\OptimizedShell'
if (-not $Dir) {
    if (Test-Path $uninstKey) {
        $Dir = (Get-ItemProperty $uninstKey -Name InstallLocation -ErrorAction SilentlyContinue).InstallLocation
    }
}
if (-not $Dir) { $Dir = Split-Path -Parent $PSCommandPath }
Write-Host "Desinstalando de $Dir"

# -------------------------------------------- el Explorador vuelve a ser el shell
$winlogon = 'HKCU:\Software\Microsoft\Windows NT\CurrentVersion\Winlogon'
if (Test-Path $winlogon) {
    $cur = (Get-ItemProperty $winlogon -Name Shell -ErrorAction SilentlyContinue).Shell
    if ($cur -and $cur -like '*OptimizedShell*') {
        Set-ItemProperty -Path $winlogon -Name 'Shell' -Value 'explorer.exe'
        Write-Host '  shell del usuario: explorer.exe (cierra sesión para aplicarlo)'
    }
}

# ------------------------------------------------------------ cerrar el entorno
foreach ($n in @('OptimizedShell', 'OptimizedExplorer', 'OptimizedTaskmgr', 'OptimizedLogonUI')) {
    Get-Process -Name $n -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}

# -------------------------------------------------------------------- registro
$appPaths = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths'
foreach ($n in @('OptimizedShell.exe', 'OptimizedExplorer.exe', 'OptimizedTaskmgr.exe',
                 'OptimizedLogonUI.exe')) {
    $key = Join-Path $appPaths $n
    if (Test-Path $key) { Remove-Item $key -Recurse -Force }
}
if (Test-Path $uninstKey) { Remove-Item $uninstKey -Recurse -Force }
Write-Host '  claves del registro quitadas'

# --------------------------------------------------------------------- accesos
$programs = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\OptimizedShell'
if (Test-Path $programs) {
    Remove-Item $programs -Recurse -Force
    Write-Host '  accesos del menú Inicio quitados'
}

# -------------------------------------------------------------------- archivos
if (Test-Path $Dir) {
    # El propio uninstall.ps1 está dentro: se borra el resto y la carpeta queda
    # para el siguiente arranque si Windows la tiene tomada.
    Get-ChildItem $Dir -Force | Where-Object { $_.Name -ne 'uninstall.ps1' } |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "  archivos borrados de $Dir"
}

if ($BorrarAjustes) {
    $cfg = Join-Path $env:APPDATA 'OptimizedShell'
    if (Test-Path $cfg) {
        Remove-Item $cfg -Recurse -Force -ErrorAction SilentlyContinue
        Write-Host "  ajustes borrados de $cfg"
    }
} else {
    Write-Host "  los ajustes se conservan en $env:APPDATA\OptimizedShell (-BorrarAjustes para quitarlos)"
}

Write-Host ''
Write-Host 'OptimizedShell desinstalado.' -ForegroundColor Green
