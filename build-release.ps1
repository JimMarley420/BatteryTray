# Builds the release: publish\BatteryTray.exe (the Rust app: one exe, nothing else to install) and
# dist\BatteryTray-<version>-win-x64.zip with the exe, the install/uninstall scripts, the docs and the licenses of
# the crates in the exe. Needs Rust and the MSVC build tools with a Windows SDK (see rust\README.md), and cargo-about
# for the licenses (`cargo install cargo-about --locked`, or its release binary).
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$rust = Join-Path $root 'rust'
$version = (Select-String -Path (Join-Path $rust 'Cargo.toml') -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value

cargo build --release --locked --manifest-path (Join-Path $rust 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'build failed' }
$built = Join-Path $rust 'target\release\BatteryTray.exe'
# The updaters (this one and the C# one before it) only install an exe that carries the release's version
$fileVersion = (Get-Item $built).VersionInfo.FileVersion
if ($fileVersion -ne "$version.0") { throw "BatteryTray.exe says $fileVersion, Cargo.toml $version" }
# It must start on a Windows without the Visual C++ redistributable (see .cargo\config.toml). The DLLs an exe needs
# are named in it as plain text.
$text = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($built))
if ($text -match '(?i)(VCRUNTIME|MSVCP)\d+[A-Z_0-9]*\.dll') {
    throw "BatteryTray.exe needs $($Matches[0]): the C runtime isn't linked into it"
}

$publish = Join-Path $root 'publish'
New-Item -ItemType Directory -Force -Path $publish | Out-Null
Copy-Item $built $publish -Force

$dist = Join-Path $root 'dist'
$stage = Join-Path $dist "BatteryTray-$version"
if (Test-Path $dist) { Remove-Item $dist -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null
$files = 'publish\BatteryTray.exe', 'install.cmd', 'install.ps1', 'uninstall.cmd', 'uninstall.ps1', 'README.md',
    'README.ru.md', 'README.zh-CN.md', 'LICENSE', 'THIRD_PARTY_NOTICES.md'
foreach ($f in $files) { Copy-Item (Join-Path $root $f) $stage }

# The crates' licenses: MIT and the like ask for their notice to go along with the exe
$licenses = Join-Path $stage 'THIRD_PARTY_LICENSES.md'
Push-Location $rust
try {
    cargo about generate --locked about.hbs -o $licenses
    if ($LASTEXITCODE -ne 0) { throw 'cargo about failed (is cargo-about installed?)' }
    # Slint's own license, which cargo-about can't attach (see about.toml): from the slint crate in the build
    $meta = cargo metadata --format-version 1 --locked | ConvertFrom-Json
    $slint = $meta.packages | Where-Object name -eq 'slint' | Select-Object -First 1
    if (-not $slint) { throw 'slint not found in cargo metadata' }
    $text = Join-Path (Split-Path $slint.manifest_path) 'LICENSES\LicenseRef-Slint-Royalty-free-2.0.md'
    # The same crates as cargo-about lists: what the exe is built from, with the features it is built with
    $names = (cargo tree --locked -e normal --target x86_64-pc-windows-msvc --prefix none -f '{p}|{l}' |
        Where-Object { $_ -match 'LicenseRef-Slint-Royalty-free' } |
        ForEach-Object { ($_ -split '\|')[0] -replace ' v(\S+).*$', ' $1' } | Sort-Object -Unique) -join ', '
    $section = @('## Slint Royalty-free Desktop, Mobile, and Web Applications License 2.0', '',
        "Used by: $names", '', '````text',
        [IO.File]::ReadAllText($text).TrimEnd(), '````', '') -join "`n"
    [IO.File]::AppendAllText($licenses, $section, (New-Object System.Text.UTF8Encoding $false))
}
finally { Pop-Location }

$zip = Join-Path $dist "BatteryTray-$version-win-x64.zip"
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
Copy-Item (Join-Path $publish 'BatteryTray.exe') $dist
Get-ChildItem $dist -File | ForEach-Object { '{0}  {1:N1} MB' -f $_.Name, ($_.Length / 1MB) }
