#!/usr/bin/env bash
# Builds weefy and boots it in QEMU under edk2 firmware, as the removable-media
# default loader (\EFI\BOOT\BOOT<arch>.EFI) of a small FAT disk.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
ARCH=aarch64
DISPLAY_MODE=gtk
FAKE_OS=0
SECURE_BOOT=0
KEEP_VARS=0
KEYS="$REPO/keys"
MONITOR=""
TIMEOUT_CONF=""
DEFAULT_CONF=""
INTRO_CONF=""
MEM=1G
OUT="$REPO/target/qemu"
FW_DIR=/usr/share/edk2

usage() {
    cat <<EOF
Usage: $0 [options]
  --arch <aarch64|x86_64>  machine to emulate                 (default: $ARCH)
  --display <gtk|none|serial>
                           gtk: a window; none: no window (drive it through
                           --monitor); serial: the console on this terminal
                                                              (default: $DISPLAY_MODE)
  --fake-os                add a second disk whose loaders are the UEFI Shell,
                           standing in for an installed OS
  --secure-boot            firmware with Secure Boot, starting in Setup Mode,
                           and weefy signed with its lab files beside it
  --keys <dir>             the Secure Boot keys; made on first use
                                                              (default: $KEYS)
  --keep-vars              start with the firmware variables the last run left
                           (enrolled keys included) instead of a blank set
  --monitor <socket>       put the QEMU monitor on this unix socket
  --conf-timeout <secs>    write weefy.conf with timeout=<secs>
  --conf-default <text>    write weefy.conf with default=<text>
  --conf-intro <auto|off|always>
                           write weefy.conf with intro=<when>: off to go
                           straight to the menu, always to play the intro
                           every time                         (default: no line; the
                                                              intro plays until skipped)
  --mem <size>             guest memory                       (default: $MEM)
  --out <dir>              where the disk contents and firmware variables go
                                                              (default: $OUT)
  --firmware-dir <dir>     where the distribution's edk2 images are
                                                              (default: $FW_DIR)
  -h, --help               this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch) ARCH="$2"; shift 2 ;;
        --display) DISPLAY_MODE="$2"; shift 2 ;;
        --fake-os) FAKE_OS=1; shift ;;
        --secure-boot) SECURE_BOOT=1; shift ;;
        --keys) KEYS="$2"; shift 2 ;;
        --keep-vars) KEEP_VARS=1; shift ;;
        --monitor) MONITOR="$2"; shift 2 ;;
        --conf-timeout) TIMEOUT_CONF="$2"; shift 2 ;;
        --conf-default) DEFAULT_CONF="$2"; shift 2 ;;
        --conf-intro) INTRO_CONF="$2"; shift 2 ;;
        --mem) MEM="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --firmware-dir) FW_DIR="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage; exit 1 ;;
    esac
done

die() { echo "error: $*" >&2; exit 1; }

# Which firmware. With Secure Boot: on x86_64 the build that keeps variables
# in flash only SMM can write; on aarch64 the build whose variables are kept
# (and their signatures checked) by QEMU's uefi-vars device, in a JSON file.
BIOS=""
case "$ARCH" in
    aarch64) SUFFIX=AA64; CODE="$FW_DIR/aarch64/QEMU_EFI-pflash.raw"; VARS="$FW_DIR/aarch64/vars-template-pflash.raw"
             SHELL_EFI="$FW_DIR/aarch64/Shell.efi"
             if [[ "$SECURE_BOOT" == 1 ]]; then
                 BIOS="$FW_DIR/aarch64/QEMU_EFI.qemuvars.fd"; CODE="$BIOS"; VARS="$FW_DIR/aarch64/vars.blank.json"
             fi ;;
    x86_64)  SUFFIX=X64;  CODE="$FW_DIR/ovmf/OVMF_CODE.fd"; VARS="$FW_DIR/ovmf/OVMF_VARS.fd"
             SHELL_EFI="$FW_DIR/ovmf/Shell.efi"
             [[ "$SECURE_BOOT" == 1 ]] && CODE="$FW_DIR/ovmf/OVMF_CODE.secboot.fd" ;;
    *) die "--arch must be aarch64 or x86_64" ;;
esac
[[ "$DISPLAY_MODE" =~ ^(gtk|none|serial)$ ]] || die "--display must be gtk, none or serial"
[[ -z "$INTRO_CONF" || "$INTRO_CONF" =~ ^(auto|off|always)$ ]] || die "--conf-intro must be auto, off or always"
for f in "$CODE" "$VARS"; do
    [[ -f "$f" ]] || die "$f not found (install edk2-ovmf / edk2-aarch64, or pass --firmware-dir)"
done
command -v "qemu-system-$ARCH" >/dev/null || die "qemu-system-$ARCH not found"

(cd "$REPO" && cargo build --release --target "$ARCH-unknown-uefi")

work="$OUT/$ARCH"
rm -rf "$work/esp" "$work/os"
mkdir -p "$work/esp/EFI/BOOT"
EFI="$REPO/target/$ARCH-unknown-uefi/release/weefy.efi"
if [[ "$SECURE_BOOT" == 1 ]]; then
    python3 -c 'import cryptography' 2>/dev/null || die "the python3 'cryptography' package is needed for --secure-boot"
    rm -rf "$work/lab"
    python3 "$REPO/tools/sbtool.py" lab --dir "$KEYS" --in "$EFI" --out "$work/lab"
    cp -r "$work/lab/keys" "$work/lab/lab" "$work/esp/EFI/BOOT/"
    EFI="$work/lab/weefy-signed.efi"
fi
cp "$EFI" "$work/esp/EFI/BOOT/BOOT$SUFFIX.EFI"
# Sample capsules for the Capsules screen to take apart; a signed one too if
# the library that signs is there.
if command -v python3 >/dev/null; then
    capsule_args=(--out "$work/esp/EFI/BOOT/capsules")
    python3 -c 'import cryptography' 2>/dev/null && capsule_args+=(--sign)
    python3 "$REPO/tools/mkcapsule.py" "${capsule_args[@]}"
fi
if [[ -n "$TIMEOUT_CONF$DEFAULT_CONF$INTRO_CONF" ]]; then
    {
        [[ -z "$TIMEOUT_CONF" ]] || echo "timeout=$TIMEOUT_CONF"
        [[ -z "$DEFAULT_CONF" ]] || echo "default=$DEFAULT_CONF"
        [[ -z "$INTRO_CONF" ]] || echo "intro=$INTRO_CONF"
    } > "$work/esp/EFI/BOOT/weefy.conf"
fi
# The variable store: a blank one unless the last run's is wanted. One per
# firmware kind, since their formats differ.
vars="$work/vars.fd"
[[ "$SECURE_BOOT" == 1 ]] && vars="$work/vars-secboot.${VARS##*.}"
if [[ "$KEEP_VARS" != 1 || ! -f "$vars" ]]; then
    cp -f "$VARS" "$vars"
fi
chmod u+w "$vars"

args=(-m "$MEM"
      -drive "if=none,id=esp,format=raw,file=fat:rw:$work/esp"
      -device virtio-blk-pci,drive=esp,bootindex=0)
if [[ -n "$BIOS" ]]; then
    args+=(-bios "$BIOS" -device "uefi-vars-sysbus,jsonfile=$vars")
else
    args+=(-drive "if=pflash,format=raw,readonly=on,file=$CODE"
           -drive "if=pflash,format=raw,file=$vars")
fi
# Without Secure Boot a restart ends the run, which suits a quick look. The
# lab needs restarts to come back up with the keys just enrolled.
[[ "$SECURE_BOOT" == 1 ]] || args+=(-no-reboot)
if [[ "$ARCH" == aarch64 ]]; then
    args=(-machine virt -cpu cortex-a72 "${args[@]}")
elif [[ "$SECURE_BOOT" == 1 ]]; then
    args=(-machine q35,smm=on -global driver=cfi.pflash01,property=secure,value=on "${args[@]}")
    [[ -w /dev/kvm ]] && args+=(-enable-kvm -cpu host)
else
    args=(-machine q35 "${args[@]}")
    [[ -w /dev/kvm ]] && args+=(-enable-kvm -cpu host)
fi

if [[ "$FAKE_OS" == 1 ]]; then
    [[ -f "$SHELL_EFI" ]] || die "$SHELL_EFI not found"
    lower="$(echo "$SUFFIX" | tr '[:upper:]' '[:lower:]')"
    mkdir -p "$work/os/EFI/BOOT" "$work/os/EFI/fedora"
    cp "$SHELL_EFI" "$work/os/EFI/BOOT/BOOT$SUFFIX.EFI"
    cp "$SHELL_EFI" "$work/os/EFI/fedora/shim$lower.efi"
    cp "$SHELL_EFI" "$work/os/EFI/fedora/grub$lower.efi"
    args+=(-drive "if=none,id=os,format=raw,file=fat:rw:$work/os"
           -device virtio-blk-pci,drive=os,bootindex=1)
fi

case "$DISPLAY_MODE" in
    serial) args+=(-nographic) ;;
    *)
        if [[ "$ARCH" == aarch64 ]]; then
            args+=(-device ramfb -device qemu-xhci -device usb-kbd -serial null)
        else
            args+=(-vga std -serial null)
        fi
        if [[ "$DISPLAY_MODE" == none ]]; then
            args+=(-display none)
        else
            args+=(-display gtk)
        fi ;;
esac
if [[ -n "$MONITOR" ]]; then
    rm -f "$MONITOR"
    args+=(-monitor "unix:$MONITOR,server,nowait")
elif [[ "$DISPLAY_MODE" != serial ]]; then
    args+=(-monitor none)
fi

exec "qemu-system-$ARCH" "${args[@]}"
