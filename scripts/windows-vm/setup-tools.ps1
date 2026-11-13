# Installs what CI's Windows job builds with: the MSVC toolchain and Windows SDK from Visual
# Studio Build Tools, and Rust through rustup. Clang comes with it because ring needs it
# on Arm64 Windows. vm.sh setup pipes this file to PowerShell over SSH; it is safe to re-run.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Get-File($url, $name) {
  $path = Join-Path $env:TEMP $name
  Invoke-WebRequest $url -OutFile $path -UseBasicParsing
  return $path
}

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$hasVc = (Test-Path $vswhere) -and
  (& $vswhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.ARM64 -property installationPath)
if (-not $hasVc) {
  Write-Output 'Installing Visual Studio Build Tools (MSVC, Windows SDK, Clang)...'
  $installer = Get-File 'https://aka.ms/vs/17/release/vs_BuildTools.exe' 'vs_BuildTools.exe'
  $vsArgs = @('--quiet', '--wait', '--norestart', '--nocache',
    '--add', 'Microsoft.VisualStudio.Workload.VCTools',
    '--add', 'Microsoft.VisualStudio.Component.VC.Tools.ARM64',
    '--add', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
    '--add', 'Microsoft.VisualStudio.Component.VC.Llvm.Clang',
    '--add', 'Microsoft.VisualStudio.Component.Windows11SDK.26100',
    '--includeRecommended')
  $p = Start-Process $installer -ArgumentList $vsArgs -Wait -PassThru
  # 3010: success, restart required.
  if ($p.ExitCode -notin 0, 3010) { throw "Build Tools installer exited with $($p.ExitCode)" }
} else {
  Write-Output 'Visual Studio Build Tools: already installed'
}

$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path $cargo)) {
  Write-Output 'Installing Rust...'
  $init = Get-File 'https://static.rust-lang.org/rustup/dist/aarch64-pc-windows-msvc/rustup-init.exe' 'rustup-init.exe'
  & $init -y --profile minimal --default-toolchain stable -c clippy -c rustfmt
  if ($LASTEXITCODE -ne 0) { throw "rustup-init exited with $LASTEXITCODE" }
} else {
  Write-Output 'Rust: already installed'
  & (Join-Path $env:USERPROFILE '.cargo\bin\rustup.exe') update stable
}

# The VS LLVM component puts clang under the VS folder; ring looks for it on PATH.
$vs = & $vswhere -products * -latest -property installationPath
$llvm = Join-Path $vs 'VC\Tools\Llvm\ARM64\bin'
if (-not (Test-Path $llvm)) { $llvm = Join-Path $vs 'VC\Tools\Llvm\bin' }
$machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
if ($machinePath -notlike "*$llvm*") {
  [Environment]::SetEnvironmentVariable('Path', "$machinePath;$llvm", 'Machine')
}

& $cargo --version
& (Join-Path $llvm 'clang.exe') --version | Select-Object -First 1
Write-Output 'setup done'
