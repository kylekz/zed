param(
    [string]$Distro = "Ubuntu"
)

$ErrorActionPreference = "Stop"

Write-Host "==> Building Zed for Windows (cargo build --release)"
cargo build --release
if ($LASTEXITCODE -ne 0) {
    Write-Error "Windows build failed (exit $LASTEXITCODE)"
}

$repoRoot = (Resolve-Path "$PSScriptRoot\..").Path
if ($repoRoot -notmatch '^([A-Za-z]):\\(.*)$') {
    Write-Error "Repo root '$repoRoot' is not a drive-letter path; cannot translate to WSL"
}
$repoWsl = "/mnt/$($Matches[1].ToLower())/$($Matches[2].Replace('\', '/'))"

Write-Host "==> Building remote_server in WSL ($Distro) at $repoWsl"

$bashScript = @'
set -e
cargo build --release --package remote_server --target-dir target/remote_server
mkdir -p ~/.zed_server
install -m 755 target/remote_server/release/remote_server ~/.zed_server/zed-remote-server-dev-build
echo "Installed: $(~/.zed_server/zed-remote-server-dev-build version 2>/dev/null || echo '?')"
'@

& wsl.exe -d $Distro --cd $repoWsl -- bash -c $bashScript
if ($LASTEXITCODE -ne 0) {
    Write-Error "WSL build failed (exit $LASTEXITCODE)"
}
