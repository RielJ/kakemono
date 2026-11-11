# First-logon setup of the local test VM, run once by autounattend.xml. Turns on the OpenSSH
# server with the host's key, so scripts/windows-vm/vm.sh can do everything else over SSH.
# Argument: the root of the answer-file disc (holds authorized_keys).
param([string]$Disc = 'D:\')
$ErrorActionPreference = 'Stop'
Start-Transcript -Path C:\provision-transcript.log -Append

# The virtio network adapter needs Red Hat's driver from the virtio-win disc. Windows has no
# inbox driver for it, so until this runs the VM has no network.
$virtio = Get-PSDrive -PSProvider FileSystem |
  Where-Object { Test-Path (Join-Path $_.Root 'NetKVM\w11\ARM64') } | Select-Object -First 1
if ($virtio) {
  foreach ($driver in 'NetKVM', 'vioserial', 'Balloon') {
    pnputil /add-driver (Join-Path $virtio.Root "$driver\w11\ARM64\*.inf") /install
  }
  Start-Sleep -Seconds 15
} else {
  Write-Output 'virtio-win disc not found; the network adapter will have no driver'
}

# A test box: never sleep, and private networking so the firewall allows SSH.
powercfg /change standby-timeout-ac 0
powercfg /change monitor-timeout-ac 0
powercfg /hibernate off
Get-NetConnectionProfile | Set-NetConnectionProfile -NetworkCategory Private

# OpenSSH server: the Windows feature first; GitHub's Win32-OpenSSH MSI if Windows Update is
# unreachable.
$installed = $false
try {
  Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0 | Out-Null
  $installed = (Get-Service sshd -ErrorAction SilentlyContinue) -ne $null
} catch {
  Write-Output "Add-WindowsCapability failed: $_"
}
if (-not $installed) {
  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
  $release = Invoke-RestMethod https://api.github.com/repos/PowerShell/Win32-OpenSSH/releases/latest
  $asset = $release.assets | Where-Object { $_.name -like 'OpenSSH-ARM64-*.msi' } | Select-Object -First 1
  $msi = Join-Path $env:TEMP $asset.name
  Invoke-WebRequest $asset.browser_download_url -OutFile $msi -UseBasicParsing
  Start-Process msiexec -ArgumentList '/i', $msi, '/qn' -Wait
}

# Admins' keys live in one file that only SYSTEM and Administrators may read; sshd ignores it
# otherwise.
New-Item -ItemType Directory -Force C:\ProgramData\ssh | Out-Null
$keys = 'C:\ProgramData\ssh\administrators_authorized_keys'
Copy-Item (Join-Path $Disc 'authorized_keys') $keys -Force
icacls $keys /inheritance:r /grant 'Administrators:F' /grant 'SYSTEM:F' | Out-Null

Set-Service sshd -StartupType Automatic
Start-Service sshd
if (-not (Get-NetFirewallRule -Name 'kk-sshd' -ErrorAction SilentlyContinue)) {
  New-NetFirewallRule -Name 'kk-sshd' -DisplayName 'OpenSSH (kakemono test VM)' -Direction Inbound `
    -Protocol TCP -LocalPort 22 -Action Allow | Out-Null
}

Set-Content C:\provision-done 'ok'
Stop-Transcript
