#!/usr/bin/env bash
# Builds weefy and packs it into bootable images:
#   - an EFI System Partition image (FAT32) with weefy as the removable-media
#     default loader, \EFI\BOOT\BOOT<arch>.EFI
#   - a whole-disk image with that ESP as its one partition: for a USB stick,
#     or, with --board orangepi5-ultra, an SD card that also carries the
#     board's loader and the edk2-rk3588 UEFI firmware from
#     firmware/orangepi5-ultra
# and optionally writes the disk image to a card or stick (--write).
# With --secure-boot, weefy is signed with keys made here and the image also
# carries what its Secure Boot lab needs: signed updates that enrol those
# keys, and signed, unsigned and tampered test images.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
ARCH=aarch64
BOARD=none
OUT="$REPO/target/image"
ESP_MB=64
CONF_TIMEOUT=""
CONF_DEFAULT=""
WRITE=""
SECURE_BOOT=0
KEYS="$REPO/keys"
FORCE=0
ASSUME_YES=0
FW="$REPO/firmware/orangepi5-ultra"
# Where the Orange Pi's own first-stage loader (in its SPI flash) expects
# things on the card, in 512-byte sectors.
IDB_SECTOR=64
FIT_SECTOR=16384
ESP_SECTOR=32768

usage() {
    cat <<EOF
Usage: $0 [options]
  --arch <aarch64|x86_64>   CPU to build for                  (default: $ARCH)
  --board <none|orangepi5-ultra>
                            none: for a machine that already has UEFI
                            firmware.
                            orangepi5-ultra: a whole SD card image with the
                            UEFI firmware too (implies --arch aarch64)
                                                              (default: $BOARD)
  --firmware <dir>          the board's firmware files        (default: $FW)
  --esp-mb <n>              size of the ESP in MiB            (default: $ESP_MB)
  --conf-timeout <secs>     put weefy.conf on the ESP: boot the OS by itself
                            after this many seconds           (default: wait for a key)
  --conf-default <text>     put weefy.conf on the ESP: the OS to boot is the
                            one whose title or path contains this text
                                                              (default: first in BootOrder)
  --secure-boot             sign weefy and add the Secure Boot lab's files:
                            the key updates and three test images
  --keys <dir>              the Secure Boot keys; made on first use. The
                            *.key files are private: keep them (default: $KEYS)
  --out <dir>               where the images go               (default: $OUT)
  --write <device>          write the image to this whole disk (an SD card or
                            USB stick, e.g. /dev/sdX). Everything on it is
                            lost. Asks you to retype the device first.
  --force                   allow --write to a device that doesn't look removable
  --yes                     don't ask before --write
  -h, --help                this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch) ARCH="$2"; shift 2 ;;
        --board) BOARD="$2"; shift 2 ;;
        --firmware) FW="$2"; shift 2 ;;
        --esp-mb) ESP_MB="$2"; shift 2 ;;
        --conf-timeout) CONF_TIMEOUT="$2"; shift 2 ;;
        --conf-default) CONF_DEFAULT="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --secure-boot) SECURE_BOOT=1; shift ;;
        --keys) KEYS="$2"; shift 2 ;;
        --write) WRITE="$2"; shift 2 ;;
        --force) FORCE=1; shift ;;
        --yes) ASSUME_YES=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage; exit 1 ;;
    esac
done

die() { echo "error: $*" >&2; exit 1; }
step() { echo; echo "=== $* ==="; }

[[ "$BOARD" =~ ^(none|orangepi5-ultra)$ ]] || die "--board must be none or orangepi5-ultra"
[[ "$BOARD" == orangepi5-ultra ]] && ARCH=aarch64
case "$ARCH" in
    aarch64) SUFFIX=AA64 ;;
    x86_64) SUFFIX=X64 ;;
    *) die "--arch must be aarch64 or x86_64" ;;
esac
[[ "$ESP_MB" =~ ^[0-9]+$ ]] && (( ESP_MB >= 34 )) || die "--esp-mb must be at least 34 (FAT32's minimum)"
for t in cargo mkfs.vfat mcopy mmd sfdisk; do
    command -v "$t" >/dev/null || die "$t not found"
done

# Check the target before building anything, so a wrong --write fails fast.
if [[ -n "$WRITE" ]]; then
    [[ -b "$WRITE" ]] || die "--write $WRITE is not a block device"
    read -r type rm tran < <(lsblk -dno TYPE,RM,TRAN "$WRITE")
    [[ "$type" == disk ]] || die "--write $WRITE is a $type, not a whole disk (pass /dev/sdX, not /dev/sdX1)"
    if lsblk -nro MOUNTPOINT "$WRITE" | grep -q .; then
        die "$WRITE (or one of its partitions) is mounted; unmount it first"
    fi
    if [[ "$rm" != 1 && "$tran" != usb && "$(basename "$WRITE")" != mmcblk* && "$FORCE" != 1 ]]; then
        die "$WRITE doesn't look removable (RM=$rm TRAN=${tran:-none}); pass --force if you're sure"
    fi
fi

step "build weefy for $ARCH"
(cd "$REPO" && cargo build --release --target "$ARCH-unknown-uefi")
EFI="$REPO/target/$ARCH-unknown-uefi/release/weefy.efi"
mkdir -p "$OUT"

if [[ "$SECURE_BOOT" == 1 ]]; then
    step "sign weefy and prepare the Secure Boot lab"
    python3 -c 'import cryptography' 2>/dev/null || die "the python3 'cryptography' package is needed for --secure-boot"
    LAB="$OUT/lab-$ARCH"
    rm -rf "$LAB"
    python3 "$REPO/tools/sbtool.py" lab --dir "$KEYS" --in "$EFI" --out "$LAB"
    EFI="$LAB/weefy-signed.efi"
fi

step "EFI System Partition"
ESP="$OUT/weefy-esp-$ARCH.img"
rm -f "$ESP"
truncate -s $(( ESP_MB << 20 )) "$ESP"
mkfs.vfat -F 32 -n WEEFY "$ESP" >/dev/null
mmd -i "$ESP" ::/EFI ::/EFI/BOOT
mcopy -i "$ESP" "$EFI" "::/EFI/BOOT/BOOT$SUFFIX.EFI"
if [[ "$SECURE_BOOT" == 1 ]]; then
    mcopy -s -i "$ESP" "$LAB/keys" "$LAB/lab" ::/EFI/BOOT/
fi
if [[ -n "$CONF_TIMEOUT$CONF_DEFAULT" ]]; then
    conf="$(mktemp)"
    {
        [[ -z "$CONF_TIMEOUT" ]] || echo "timeout=$CONF_TIMEOUT"
        [[ -z "$CONF_DEFAULT" ]] || echo "default=$CONF_DEFAULT"
    } > "$conf"
    mcopy -i "$ESP" "$conf" ::/EFI/BOOT/weefy.conf
    rm -f "$conf"
fi
echo ">> $ESP ($ESP_MB MiB FAT32):"
mdir -/ -b -i "$ESP" ::/ | sed 's/^/     /'

# A whole-disk image: a GPT with the ESP as its one partition. On the Orange
# Pi the board's loaders go in the gap before it, where the partition table
# (which ends at sector 33) doesn't reach.
if [[ "$BOARD" == orangepi5-ultra ]]; then
    step "SD card image for the Orange Pi 5 Ultra"
    idb="$FW/idbloader.img"; itb="$FW/uefi.itb"
    for f in "$idb" "$itb"; do
        [[ -f "$f" ]] || die "$f not found (see firmware/orangepi5-ultra/README.md)"
    done
    # Sanity-check the blobs before they go anywhere near a card.
    [[ "$(head -c 4 "$idb")" == RKNS ]] || die "$idb lacks the Rockchip RKNS header"
    [[ "$(od -An -tx1 -N4 "$itb" | tr -d ' ')" == d00dfeed ]] || die "$itb is not a FIT image"
    (( IDB_SECTOR + ( $(stat -c%s "$idb") + 511 ) / 512 <= FIT_SECTOR )) || die "idbloader.img would overlap the firmware"
    (( FIT_SECTOR + ( $(stat -c%s "$itb") + 511 ) / 512 <= ESP_SECTOR )) || die "uefi.itb would overlap the ESP"
    IMAGE="$OUT/weefy-orangepi5-ultra.img"
    esp_start=$ESP_SECTOR
else
    step "disk image for a USB stick"
    IMAGE="$OUT/weefy-disk-$ARCH.img"
    esp_start=2048
fi
rm -f "$IMAGE"
esp_sectors=$(( $(stat -c%s "$ESP") / 512 ))
# Room after the ESP for the backup partition table.
truncate -s $(( (esp_start + esp_sectors + 2048) * 512 )) "$IMAGE"
printf 'label: gpt\nstart=%d, size=%d, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name="ESP"\n' \
    "$esp_start" "$esp_sectors" | sfdisk --quiet "$IMAGE"
if [[ "$BOARD" == orangepi5-ultra ]]; then
    dd if="$idb" of="$IMAGE" seek="$IDB_SECTOR" bs=512 conv=notrunc status=none
    dd if="$itb" of="$IMAGE" seek="$FIT_SECTOR" bs=512 conv=notrunc status=none
    echo ">> loader @$IDB_SECTOR, UEFI firmware @$FIT_SECTOR, ESP @$esp_start"
fi
dd if="$ESP" of="$IMAGE" seek="$esp_start" bs=512 conv=notrunc status=none
echo ">> $IMAGE ($(( $(stat -c%s "$IMAGE") >> 20 )) MiB)"

if [[ -z "$WRITE" ]]; then
    echo
    echo "To put it on an SD card or USB stick, add: --write /dev/sdX"
    exit 0
fi

step "write $WRITE"
lsblk -o NAME,SIZE,TYPE,RM,TRAN,MODEL,MOUNTPOINT "$WRITE"
(( $(lsblk -bdno SIZE "$WRITE") >= $(stat -c%s "$IMAGE") )) || die "$WRITE is smaller than the image"
if [[ "$ASSUME_YES" != 1 ]]; then
    echo ">> everything on $WRITE will be lost. Retype the device to confirm:"
    read -r answer || die "no confirmation; not writing"
    [[ "$answer" == "$WRITE" ]] || die "confirmation didn't match; not writing"
fi
sudo dd if="$IMAGE" of="$WRITE" bs=4M conv=fsync status=progress
sync
echo ">> wrote $IMAGE to $WRITE"
