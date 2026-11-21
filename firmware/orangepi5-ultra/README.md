# UEFI firmware for the Orange Pi 5 Ultra

Copied from the `hvhyp` project's working UEFI boot of this board. None of it
was built here. `scripts/make-image.sh --board orangepi5-ultra` puts it on an
SD card image with weefy.

| File | SD card sector | What it is |
|---|---|---|
| `idbloader.img` | 64 | Rockchip's first-stage loader (memory training and SPL), from Armbian's `linux-u-boot-orangepi5-ultra-vendor` package, version 26.8.3. |
| `uefi.itb` | 16384 | A FIT image holding edk2-rk3588 (the UEFI firmware), Arm Trusted Firmware (BL31), OP-TEE and the board's device tree. |
| `orangepi-5-max_UEFI_Release_v1.0-ex.img` | not on the card | The release image `uefi.itb` was repacked from; kept so that it can be repacked again. |

The ESP with weefy starts at sector 32768 (16 MiB).

## Where `uefi.itb` comes from

`hvhyp/scripts/boot/build-pi-uefi.sh` made it from the community edk2-rk3588
build for the Orange Pi 5 Max:

    https://github.com/amazingfate/edk2-rk35xx/releases/download/v1.0-ex/orangepi-5-max_UEFI_Release_v1.0-ex.img

The Ultra shares the Max's board base (PMIC, regulators, RAM); they differ in
HDMI wiring and two GPIOs. The script unpacks the release image's FIT and
repacks the same images at the same load addresses, with the UEFI firmware
volume marked as the "U-Boot" image, because the board's own loader, in its
SPI flash, runs first, reads a FIT from sector 16384 of the card and starts
the image of that name.

The UEFI variable store in the FIT is empty, as on a freshly written card.
The firmware starts from its default settings on every boot, and changes made
in its setup menu, or to variables, don't persist.

## Checksums (SHA-256)

    9de5b8cc05ece69522f53c661fd8c936404ad3c0a4eb8fec40c8006a5f7a1316  idbloader.img
    d5bee83eb40ec5a2afd30db28023a564c7b09b3273610eaee6a984122ce1855b  uefi.itb
    fcd8574ebec41731ae2b9ec692f8ab46e083aedc7cd359a4ca1b959e02e6fc9a  orangepi-5-max_UEFI_Release_v1.0-ex.img

The last is the checksum `build-pi-uefi.sh` pins for the download.
