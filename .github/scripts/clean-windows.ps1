# Starts BatteryTray.exe on a Windows that has nothing installed: Server Core in a container, which has no Visual C++
# redistributable. The exe is run with --dump, which polls every provider once and exits, so it has to load and run.
#   clean-windows.ps1 <exe>              fails unless the exe starts and exits with 0
#   clean-windows.ps1 <exe> -MustFail    fails unless the exe can't start: the check checked on an exe known not to
param(
    [Parameter(Mandatory)][string]$Exe,
    [switch]$MustFail,
    [string]$Image = 'mcr.microsoft.com/windows/servercore:ltsc2022'
)
$ErrorActionPreference = 'Stop'

$share = Join-Path ([IO.Path]::GetTempPath()) ("bt-clean-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $share | Out-Null
Copy-Item $Exe (Join-Path $share 'BatteryTray.exe')

# Runs in the container. The DLL names an exe imports stand in it as plain text, which is enough to tell which of
# them this Windows doesn't have.
Set-Content -Path (Join-Path $share 'run.ps1') -Encoding ASCII -Value @'
$exe = 'C:\bt\BatteryTray.exe'
$text = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($exe))
$names = [regex]::Matches($text, '[A-Za-z0-9_\-]+\.dll') | ForEach-Object { $_.Value.ToLowerInvariant() } |
    Sort-Object -Unique | Where-Object { $_ -notmatch '^(api|ext)-ms-' }
$missing = @($names | Where-Object { -not (Test-Path (Join-Path $env:windir "System32\$_")) })
"DLLs named in the exe that this Windows doesn't have: " + $(if ($missing) { $missing -join ', ' } else { 'none' })
$p = Start-Process $exe -ArgumentList '--dump', 'C:\bt\dump.txt' -PassThru
if (-not $p.WaitForExit(120000)) { $p.Kill(); 'did not exit in 2 minutes'; exit 3 }
'exit code: 0x{0:X8}' -f $p.ExitCode
if (Test-Path C:\bt\dump.txt) { Get-Content C:\bt\dump.txt | Select-Object -First 30 }
if ($p.ExitCode -ne 0) { exit 1 }
if (-not (Test-Path C:\bt\dump.txt)) { 'no dump written'; exit 2 }
exit 0
'@

docker run --rm -v "${share}:C:\bt" $Image powershell -NoProfile -ExecutionPolicy Bypass -File C:\bt\run.ps1
$code = $LASTEXITCODE
Remove-Item $share -Recurse -Force -ErrorAction SilentlyContinue

if ($MustFail) {
    if ($code -eq 0) { throw "$Exe started, though it is known to need the Visual C++ redistributable: this check proves nothing" }
    "As expected, $Exe doesn't start here (exit $code)."
    exit 0
}
if ($code -ne 0) { throw "$Exe doesn't start on a clean Windows (exit $code)" }
"$Exe starts on a clean Windows."
