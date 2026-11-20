# Instalador del entorno OptimizedShell.
#
# Copia los cuatro ejecutables a Program Files, crea los accesos del menú
# Inicio, registra las rutas de aplicación (para poder escribir
# "OptimizedExplorer" en Ejecutar) y la entrada de "Programas instalados", y
# —si se pide— deja OptimizedShell como shell del usuario en lugar del
# Explorador de Windows.
#
#   .\install.ps1                      # instala el entorno
#   .\install.ps1 -AsShell             # ... y lo pone como shell del usuario
#   .\install.ps1 -Dir "D:\OShell"     # en otra carpeta
#   .\install.ps1 -Source ..\RustShell\target\x86_64-pc-windows-msvc\release
#
# Pide elevación sola: escribir en Program Files y en HKLM la necesita.

[CmdletBinding()]
param(
    # Carpeta de instalación.
    [string]$Dir = (Join-Path $env:ProgramFiles 'OptimizedShell'),
    # Carpeta con los .exe ya compilados. Por omisión, la del script y, si no
    # están ahí, la carpeta release del repositorio.
    [string]$Source,
    # Deja OptimizedShell como shell del usuario actual (Winlogon).
    [switch]$AsShell,
    # No crear accesos en el menú Inicio.
    [switch]$NoShortcuts,
    # Versión que se enseña en "Programas instalados" (se lee de Cargo.toml).
    [string]$Version
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSCommandPath
$repo = Split-Path -Parent $root

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

# --------------------------------------------------------- de dónde se copia
$exeNames = @('OptimizedShell.exe', 'OptimizedExplorer.exe', 'OptimizedTaskmgr.exe',
              'OptimizedLogonUI.exe')

function Test-Source([string]$path) {
    if (-not $path) { return $false }
    if (-not (Test-Path $path)) { return $false }
    foreach ($n in $exeNames) {
        if (-not (Test-Path (Join-Path $path $n))) { return $false }
    }
    return $true
}

if (-not (Test-Source $Source)) {
    $candidates = @(
        $root,
        (Join-Path $repo 'RustShell\target\x86_64-pc-windows-msvc\release'),
        (Join-Path $repo 'RustShell\target\release')
    )
    $Source = $candidates | Where-Object { Test-Source $_ } | Select-Object -First 1
}
if (-not $Source) {
    throw ("No encuentro los ejecutables ($($exeNames -join ', ')). " +
           'Compila con "cargo build --release" en RustShell o indica -Source.')
}
Write-Host "Origen:  $Source"
Write-Host "Destino: $Dir"

if (-not $Version) {
    $toml = Join-Path $repo 'RustShell\Cargo.toml'
    if (Test-Path $toml) {
        $m = Select-String -Path $toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1
        if ($m) { $Version = $m.Matches[0].Groups[1].Value }
    }
}
if (-not $Version) { $Version = '0.0.0' }

# ------------------------------------------------------------------- copiar
# Un ejecutable en uso no se puede sobrescribir: se aparta a .old y Windows lo
# borra en el siguiente arranque (es lo mismo que hace --update).
New-Item -ItemType Directory -Force $Dir | Out-Null
foreach ($n in $exeNames) {
    $dest = Join-Path $Dir $n
    if (Test-Path $dest) {
        try {
            Remove-Item $dest -Force
        } catch {
            $old = "$dest.old"
            if (Test-Path $old) { Remove-Item $old -Force -ErrorAction SilentlyContinue }
            Rename-Item $dest $old -Force
            Write-Host "  $n estaba en uso: la versión anterior queda como $n.old"
        }
    }
    Copy-Item (Join-Path $Source $n) $dest -Force
    Write-Host "  copiado $n"
}
foreach ($extra in @('README.md', 'LICENSE')) {
    $src = Join-Path $repo $extra
    if (-not (Test-Path $src)) { $src = Join-Path $Source $extra }
    if (Test-Path $src) { Copy-Item $src (Join-Path $Dir $extra) -Force }
}
# El desinstalador viaja con la instalación.
Copy-Item (Join-Path $root 'uninstall.ps1') (Join-Path $Dir 'uninstall.ps1') -Force

$shellExe   = Join-Path $Dir 'OptimizedShell.exe'
$explorExe  = Join-Path $Dir 'OptimizedExplorer.exe'
$taskmgrExe = Join-Path $Dir 'OptimizedTaskmgr.exe'
$logonExe   = Join-Path $Dir 'OptimizedLogonUI.exe'

# ------------------------------------------------------ accesos del menú Inicio
if (-not $NoShortcuts) {
    $programs = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\OptimizedShell'
    New-Item -ItemType Directory -Force $programs | Out-Null
    $wsh = New-Object -ComObject WScript.Shell
    $links = @(
        @{ Name = 'OptimizedShell (barra de tareas)'; Target = $shellExe;   Args = '' },
        @{ Name = 'Explorador de archivos';           Target = $explorExe;  Args = '' },
        @{ Name = 'Administrador de tareas';          Target = $taskmgrExe; Args = '' },
        @{ Name = 'Configuración';                    Target = $shellExe;   Args = '--settings' },
        @{ Name = 'Bloquear la sesión';               Target = $logonExe;   Args = '--lock' },
        @{ Name = 'Buscar actualizaciones';           Target = $shellExe;   Args = '--update' },
        @{ Name = 'Desinstalar OptimizedShell';       Target = 'powershell.exe';
           Args = "-NoProfile -ExecutionPolicy Bypass -File `"$(Join-Path $Dir 'uninstall.ps1')`"" }
    )
    foreach ($l in $links) {
        $lnk = $wsh.CreateShortcut((Join-Path $programs ($l.Name + '.lnk')))
        $lnk.TargetPath = $l.Target
        if ($l.Args) { $lnk.Arguments = $l.Args }
        $lnk.WorkingDirectory = $Dir
        $lnk.IconLocation = "$($l.Target),0"
        $lnk.Save()
    }
    Write-Host "  accesos en el menú Inicio: $programs"
}

# ------------------------------------------------- rutas de aplicación (Ejecutar)
$appPaths = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths'
foreach ($n in $exeNames) {
    $key = Join-Path $appPaths $n
    New-Item -Path $key -Force | Out-Null
    Set-ItemProperty -Path $key -Name '(Default)' -Value (Join-Path $Dir $n)
    Set-ItemProperty -Path $key -Name 'Path' -Value $Dir
}
Write-Host '  registradas las rutas de aplicación (Win+R: OptimizedExplorer)'

# ------------------------------------------------------ programas instalados
$uninstKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\OptimizedShell'
New-Item -Path $uninstKey -Force | Out-Null
$size = [int](((Get-ChildItem $Dir -File | Measure-Object Length -Sum).Sum) / 1KB)
$props = @{
    DisplayName     = 'OptimizedShell'
    DisplayVersion  = $Version
    Publisher       = 'isaachhk02'
    DisplayIcon     = $shellExe
    InstallLocation = $Dir
    EstimatedSize   = $size
    NoModify        = 1
    NoRepair        = 1
    URLInfoAbout    = 'https://github.com/isaachhk02/OptimizedShell'
    UninstallString = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$(Join-Path $Dir 'uninstall.ps1')`""
}
foreach ($k in $props.Keys) { Set-ItemProperty -Path $uninstKey -Name $k -Value $props[$k] }
Write-Host '  registrado en Programas instalados'

# --------------------------------------------------------------- shell del usuario
if ($AsShell) {
    $winlogon = 'HKCU:\Software\Microsoft\Windows NT\CurrentVersion\Winlogon'
    New-Item -Path $winlogon -Force | Out-Null
    Set-ItemProperty -Path $winlogon -Name 'Shell' -Value $shellExe
    Write-Host ''
    Write-Host 'OptimizedShell queda como shell del usuario actual.' -ForegroundColor Yellow
    Write-Host 'Cierra sesión y vuelve a entrar para usar el entorno nuevo.' -ForegroundColor Yellow
    Write-Host 'Para deshacerlo: OptimizedShell.exe --uninstall (o uninstall.ps1).'
}

Write-Host ''
Write-Host "Instalado en $Dir" -ForegroundColor Green
Write-Host 'Ejecutables: OptimizedShell (barra), OptimizedExplorer, OptimizedTaskmgr, OptimizedLogonUI.'
if (-not $AsShell) {
    Write-Host 'Para usarlo como entorno completo: .\install.ps1 -AsShell (o OptimizedShell.exe --install).'
}
