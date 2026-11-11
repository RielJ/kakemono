#!/usr/bin/env bash
# A local Windows 11 on Arm VM for running the Windows tests on an Apple silicon Mac, with QEMU
# and Apple's Hypervisor.framework. No account or licence key is needed; Windows runs
# unactivated, which is fine for tests.
#
#   vm.sh install <win11-arm64.iso>   create the disk and install Windows unattended (~20 min)
#   vm.sh setup                       install the build tools CI's Windows job uses (~30 min)
#   vm.sh start | stop | status       run the installed VM headless
#   vm.sh ssh [command...]            a shell, or one command, in the VM
#   vm.sh screen [out.png]            save a screenshot of the VM's display
#   vm.sh destroy                     delete the VM and everything under its folder
#
# Everything lives in $KK_VM_DIR (default ~/VMs/kakemono-win); deleting that folder removes it.
# The Windows ISO comes from https://www.microsoft.com/software-download/windows11arm64
# (no sign-in). Needs `brew install qemu`. While the VM runs, VNC on 127.0.0.1:5905 shows its
# screen and SSH listens on 127.0.0.1:$KK_VM_SSH_PORT (default 2225). The VNC password is
# "kakemono" (macOS Screen Sharing refuses a server without one) and the Windows account is kk
# with password "kakemono."; both are fixed because the VM only listens on 127.0.0.1.
set -euo pipefail
{ # Parsed whole before it runs, so editing this file mid-run is safe (scripts/check-scripts.sh).

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dir="${KK_VM_DIR:-$HOME/VMs/kakemono-win}"
port="${KK_VM_SSH_PORT:-2225}"
cpus="${KK_VM_CPUS:-6}"
mem="${KK_VM_MEM:-8G}"
disk_size="${KK_VM_DISK:-96G}"
virtio_url=https://fedorapeople.org/groups/virt/virtio-win/direct-downloads/stable-virtio/virtio-win.iso

vm_password='kakemono.'

die() { echo "vm: $*" >&2; exit 1; }
log() { echo "vm: $*" >&2; }

qemu_share() { echo "$(brew --prefix qemu)/share/qemu"; }
pid() { [[ -f "$dir/qemu.pid" ]] && kill -0 "$(cat "$dir/qemu.pid")" 2>/dev/null && cat "$dir/qemu.pid"; }
monitor() { printf '%s\n' "$1" | nc -U -w 1 "$dir/monitor.sock" >/dev/null 2>&1 || true; }

ssh_vm() {
  ssh -i "$dir/id_ed25519" -p "$port" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o LogLevel=ERROR -o ConnectTimeout=5 -o ServerAliveInterval=15 kk@127.0.0.1 "$@"
}

wait_for_ssh() { # timeout in seconds
  local deadline=$((SECONDS + $1))
  until ssh_vm 'exit 0' 2>/dev/null; do
    pid >/dev/null || die "QEMU exited; see $dir/qemu.log"
    ((SECONDS < deadline)) || die "no SSH after $1 s; watch the screen with: open vnc://127.0.0.1:5905"
    sleep 10
  done
}

# Boots QEMU in the background. Extra arguments add drives (the install discs).
boot() {
  [[ -z "$(pid)" ]] || die "already running (pid $(pid))"
  local share; share="$(qemu_share)"
  rm -f "$dir/monitor.sock"
  qemu-system-aarch64 \
    -name kakemono-win -machine virt,highmem=on -accel hvf -cpu host -smp "$cpus" -m "$mem" \
    -drive "if=pflash,format=raw,readonly=on,file=$share/edk2-aarch64-code.fd" \
    -drive "if=pflash,format=raw,file=$dir/efivars.fd" \
    -device ramfb -device qemu-xhci -device usb-kbd -device usb-tablet \
    -drive "if=none,id=disk,format=qcow2,discard=unmap,file=$dir/disk.qcow2" \
    -device nvme,drive=disk,serial=kakemono,bootindex=1 \
    -netdev "user,id=net,hostfwd=tcp:127.0.0.1:$port-:22" -device virtio-net-pci,netdev=net \
    -rtc base=utc -display none -vnc 127.0.0.1:5,password=on \
    -monitor "unix:$dir/monitor.sock,server,nowait" \
    -pidfile "$dir/qemu.pid" -daemonize "$@" >"$dir/qemu.log" 2>&1 \
    || die "QEMU failed to start: $(cat "$dir/qemu.log")"
  monitor 'set_password vnc kakemono'
}

cdrom() { # id path [bootindex]
  printf -- '-drive\0if=none,id=%s,media=cdrom,readonly=on,file=%s\0-device\0usb-storage,drive=%s%s\0' \
    "$1" "$2" "$1" "${3:+,bootindex=$3}"
}

cmd_install() {
  local iso="${1:-}"
  [[ -f "$iso" ]] || die "usage: vm.sh install <win11-arm64.iso>"
  command -v qemu-system-aarch64 >/dev/null || die "QEMU is missing: brew install qemu"
  [[ ! -f "$dir/disk.qcow2" ]] || die "$dir/disk.qcow2 exists; run 'vm.sh destroy' first"
  mkdir -p "$dir"
  iso="$(cd "$(dirname "$iso")" && pwd)/$(basename "$iso")"

  [[ -f "$dir/virtio-win.iso" ]] || { log "downloading virtio-win drivers"; curl -fL -o "$dir/virtio-win.iso" "$virtio_url"; }
  [[ -f "$dir/id_ed25519" ]] || ssh-keygen -q -t ed25519 -N '' -C kakemono-win-vm -f "$dir/id_ed25519"

  # The answer-file disc: Windows Setup looks for autounattend.xml at the root of every drive.
  local answer="$dir/answer"
  rm -rf "$answer" "$dir/answer.iso"; mkdir -p "$answer"
  sed "s/@PASSWORD@/$vm_password/g" "$here/autounattend.xml" >"$answer/autounattend.xml"
  cp "$here/provision.ps1" "$answer/"
  cp "$dir/id_ed25519.pub" "$answer/authorized_keys"
  hdiutil makehybrid -quiet -iso -joliet -default-volume-name KKANSWER -o "$dir/answer.iso" "$answer"
  rm -rf "$answer"

  qemu-img create -q -f qcow2 "$dir/disk.qcow2" "$disk_size"
  cp "$(qemu_share)/edk2-arm-vars.fd" "$dir/efivars.fd"

  local discs=()
  while IFS= read -r -d '' a; do discs+=("$a"); done < <(
    cdrom win "$iso" 0; cdrom answer "$dir/answer.iso"; cdrom virtio "$dir/virtio-win.iso"
  )
  log "booting the installer; watch with: open vnc://127.0.0.1:5905"
  boot "${discs[@]}"
  # The Windows disc waits for a key press before it boots ("Press any key to boot from CD").
  for _ in $(seq 1 30); do monitor 'sendkey spc'; sleep 1; done

  log "installing Windows; this takes 15-30 min"
  wait_for_ssh 5400
  ssh_vm 'type C:\provision-done' >/dev/null || die "provisioning did not finish; see C:\\provision.log in the VM"
  log "installed; Windows is up and reachable with 'vm.sh ssh'"
}

cmd_setup() {
  [[ -n "$(pid)" ]] || cmd_start
  log "installing the build tools (Visual Studio Build Tools, Rust, Node, Git); this takes a while"
  ssh_vm 'powershell -NoProfile -ExecutionPolicy Bypass -Command -' <"$here/setup-tools.ps1"
}

cmd_start() {
  [[ -f "$dir/disk.qcow2" ]] || die "no VM yet; run 'vm.sh install <iso>' first"
  if [[ -z "$(pid)" ]]; then boot; fi
  wait_for_ssh 600
  log "up; ssh with 'vm.sh ssh'"
}

cmd_stop() {
  [[ -n "$(pid)" ]] || { log "not running"; return; }
  ssh_vm 'shutdown /s /t 0' 2>/dev/null || monitor system_powerdown
  local deadline=$((SECONDS + 180))
  while [[ -n "$(pid)" ]] && ((SECONDS < deadline)); do sleep 3; done
  if [[ -n "$(pid)" ]]; then log "no clean shutdown after 3 min; stopping QEMU"; monitor quit; fi
  log "stopped"
}

cmd_status() {
  if [[ -n "$(pid)" ]]; then echo "running (pid $(pid)), ssh on 127.0.0.1:$port"
  elif [[ -f "$dir/disk.qcow2" ]]; then echo "stopped"
  else echo "not installed"; fi
  [[ -d "$dir" ]] && du -sh "$dir"
}

cmd_screen() {
  [[ -n "$(pid)" ]] || die "not running"
  local out="${1:-$dir/screen.png}" ppm="$dir/screen.ppm"
  rm -f "$ppm"
  monitor "screendump $ppm"
  sleep 1
  [[ -f "$ppm" ]] || die "QEMU wrote no screenshot"
  sips -s format png "$ppm" --out "$out" >/dev/null && rm -f "$ppm"
  echo "$out"
}

cmd_destroy() {
  [[ -d "$dir" ]] || { log "nothing at $dir"; return; }
  if [[ -n "$(pid)" ]]; then monitor quit; sleep 2; fi
  rm -rf "$dir"
  log "removed $dir"
}

case "${1:-}" in
  install) shift; cmd_install "$@" ;;
  setup) cmd_setup ;;
  start) cmd_start ;;
  stop) cmd_stop ;;
  status) cmd_status ;;
  ssh) shift; ssh_vm "$@" ;;
  screen) shift; cmd_screen "$@" ;;
  destroy) cmd_destroy ;;
  -h|--help|help|'') awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "$0" ;;
  *) die "unknown command: $1 (see vm.sh --help)" ;;
esac
exit; }
