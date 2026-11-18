# Compila OptimizedShell (C++/Win32 + GDI+ + DWM) con MSVC.
# Un solo binario con modos: shell, --taskmgr, --explorer, --logon / --lock, --install / --uninstall.
$ErrorActionPreference = 'Stop'

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'No se encontró Visual Studio con herramientas C++.' }

$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
$out = Join-Path $PSScriptRoot 'bin'
New-Item -ItemType Directory -Path $out -Force | Out-Null

$src = Join-Path $PSScriptRoot 'src'
$sources = @('shell.cpp', 'taskmgr.cpp', 'explorer.cpp', 'settings.cpp', 'logonui.cpp' | ForEach-Object { '"' + (Join-Path $src $_) + '"' }) -join ' '

$libs = @(
    'user32.lib', 'gdi32.lib', 'gdiplus.lib', 'shell32.lib', 'ole32.lib', 'oleaut32.lib',
    'shlwapi.lib', 'dwmapi.lib', 'advapi32.lib', 'powrprof.lib', 'psapi.lib',
    'netapi32.lib', 'userenv.lib', 'uxtheme.lib', 'wlanapi.lib', 'uuid.lib',
    'comctl32.lib', 'pdh.lib', 'iphlpapi.lib'
) -join ' '

# /O1 + /GL: binario pequeño (menos páginas de código que cargar); /OPT:REF,ICF quitan código sin usar.
$cl = 'cl /nologo /std:c++17 /utf-8 /O1 /GL /Gw /EHsc /MT /DUNICODE /D_UNICODE /D_WIN32_WINNT=0x0A00 /DNTDDI_VERSION=0x0A00000C /D_CRT_SECURE_NO_WARNINGS'
$link = "/link /LTCG /SUBSYSTEM:WINDOWS /MANIFEST:EMBED /OPT:REF /OPT:ICF $libs"
$cmd = 'call "{0}" >nul && {1} /Fe:"{2}\OptimizedShellCpp.exe" /Fo:"{2}\\" {3} {4}' -f $vcvars, $cl, $out, $sources, $link

cmd.exe /c $cmd
if ($LASTEXITCODE -ne 0) { throw "Compilación fallida ($LASTEXITCODE)" }
Remove-Item (Join-Path $out '*.obj') -ErrorAction SilentlyContinue
Write-Host "Compilado: $out\OptimizedShellCpp.exe"
