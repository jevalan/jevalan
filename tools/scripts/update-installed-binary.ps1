# Sustituye el binario que usan el hook de Claude Code y el daemon programado.
# Cabe señalar que Windows no consiente sobrescribir un .exe en ejecución; de ahí
# que la tarea `jevalan-daemon` se detenga primero y se reanude al final.
param([string]$Source = "$PSScriptRoot\..\target\release\jevalan.exe")

$ErrorActionPreference = 'Stop'
$dest = "$env:LOCALAPPDATA\jevalan\bin\jevalan.exe"
if (-not (Test-Path $Source)) { throw "No existe $Source; compílese antes con: cargo build --release -p jevalan" }

Stop-ScheduledTask -TaskName jevalan-daemon -ErrorAction SilentlyContinue
# Por si quedara algún daemon lanzado a mano desde esa misma ruta.
Get-Process jevalan -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $dest } | Stop-Process -Force

# Un hook en curso puede retener el fichero unos instantes: se reintenta con paciencia.
foreach ($i in 1..10) {
    try { Copy-Item $Source $dest -Force; break }
    catch { if ($i -eq 10) { throw }; Start-Sleep -Milliseconds 500 }
}

Start-ScheduledTask -TaskName jevalan-daemon
Start-Sleep 3
$ok = try { (Invoke-WebRequest http://127.0.0.1:8080/healthz -UseBasicParsing).StatusCode -eq 200 } catch { $false }
if ($ok) { "Binario sustituido y daemon en marcha." } else { "Binario sustituido, pero el daemon no responde: revísese $env:LOCALAPPDATA\jevalan\serve.log" }
