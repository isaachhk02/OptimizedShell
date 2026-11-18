# Mide el consumo de recursos de OptimizedShell (o de cualquier proceso ya abierto).
#
#   .\measure.ps1 -Mode taskmgr                 # lanza "--taskmgr" y lo mide
#   .\measure.ps1 -Mode explorer -Minimize      # mide con la ventana minimizada
#   .\measure.ps1 -Mode shell -Exe .\old.exe    # compara con otro binario
#   .\measure.ps1 -Attach explorer              # mide un proceso existente (por nombre)
#
# Muestra: working set, memoria privada, CPU usada durante la muestra,
# handles y objetos GDI/USER.
param(
    [ValidateSet('shell', 'taskmgr', 'explorer')]
    [string]$Mode = 'taskmgr',
    [string]$Exe = (Join-Path $PSScriptRoot 'bin\OptimizedShellCpp.exe'),
    [string]$Attach = '',
    [int]$Settle = 10,
    [int]$Seconds = 60,
    [switch]$Minimize
)
$ErrorActionPreference = 'Stop'

Add-Type -Namespace OShell -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern uint GetGuiResources(System.IntPtr hProcess, uint uiFlags);
[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr hWnd, int nCmdShow);
'@

$launched = $null
if ($Attach) {
    $proc = Get-Process -Name $Attach | Sort-Object WorkingSet64 -Descending | Select-Object -First 1
    if (-not $proc) { throw "No se encontró el proceso '$Attach'." }
} else {
    $arg = switch ($Mode) { 'shell' { '' } 'taskmgr' { '--taskmgr' } 'explorer' { "--explorer `"$env:USERPROFILE`"" } }
    if ($arg) { $proc = Start-Process -FilePath $Exe -ArgumentList $arg -PassThru }
    else { $proc = Start-Process -FilePath $Exe -PassThru }
    $launched = $proc
}

Write-Host "Esperando $Settle s a que se estabilice (PID $($proc.Id))..."
Start-Sleep -Seconds $Settle
$proc.Refresh()
if ($Minimize -and $proc.MainWindowHandle -ne 0) { [OShell.Native]::ShowWindow($proc.MainWindowHandle, 6) | Out-Null; Start-Sleep 2 }

$cpu0 = $proc.TotalProcessorTime
$t0 = Get-Date
$ws = @(); $priv = @()
for ($i = 0; $i -lt $Seconds; $i++) {
    Start-Sleep -Seconds 1
    $proc.Refresh()
    if ($proc.HasExited) { throw 'El proceso terminó durante la medición.' }
    $ws += $proc.WorkingSet64
    $priv += $proc.PrivateMemorySize64
}
$proc.Refresh()
$elapsed = ((Get-Date) - $t0).TotalSeconds
$cpuMs = ($proc.TotalProcessorTime - $cpu0).TotalMilliseconds
$cores = [Environment]::ProcessorCount

$result = [pscustomobject]@{
    Proceso          = if ($Attach) { $proc.ProcessName } else { "$Mode ($([IO.Path]::GetFileName($Exe)))" }
    'WS medio (MB)'  = [math]::Round((($ws | Measure-Object -Average).Average) / 1MB, 1)
    'WS max (MB)'    = [math]::Round((($ws | Measure-Object -Maximum).Maximum) / 1MB, 1)
    'Privada (MB)'   = [math]::Round((($priv | Measure-Object -Average).Average) / 1MB, 1)
    'CPU (ms)'       = [math]::Round($cpuMs, 0)
    'CPU media (%)'  = [math]::Round(100 * $cpuMs / 1000 / $elapsed / $cores, 3)
    Handles          = $proc.HandleCount
    GDI              = [OShell.Native]::GetGuiResources($proc.Handle, 0)
    USER             = [OShell.Native]::GetGuiResources($proc.Handle, 1)
}
$result | Format-List

if ($launched -and -not $launched.HasExited) { Stop-Process -Id $launched.Id -Force }
