# weefy

An educational UEFI application. 
Enter the hidden world before ExitBootServices().
Boot it to see what the firmware offers your OS.

> **Note:** weefy is for educational purposes only. Do not use it on machines
> that contain important data or anything else that you care about.

![The intro's logo screen in QEMU: "Weefy teaches UEFI, with Dr. Weefy Fulgens", a pixel-art red panda in a lab coat waving from behind a lab bench](docs/intro.png)

It is a text UI in the firmware's own console (a display or a serial port):

- **Take the tour**: thirteen short lessons, each ending with facts read from
  this machine.
- **System table**: the header, the console, and every configuration table,
  with ACPI, SMBIOS, device tree and others decoded.
- **Boot services / Runtime services**: every function in the two tables, what
  it does, the address this firmware put there and which loaded image that
  address is inside.
- **Handles / Protocols**: the handle database from both directions, with
  about 950 GUIDs named and the common protocols explained.
- **Variables**: every variable, read-only, with the standard ones decoded
  (boot options, BootOrder, Secure Boot keys, console paths).
- **Memory map**: every range and a summary by type.
- **Console**: the text modes (switchable) and the framebuffer.
- **Secure Boot lab**: the keys the firmware holds, how weefy itself is
  signed, and what `LoadImage()` says to a signed, an unsigned and a tampered
  image. On a machine in Setup Mode it enrols keys you made, which turns
  Secure Boot on under your own key (see "Secure Boot, end to end").
- **Capsules and firmware updates**: what the firmware says about capsules
  (`QueryCapsuleCapabilities()`, `OsIndicationsSupported`), the updatable
  components in the ESRT, each Firmware Management Protocol image with its
  attributes (signed updates required or not), and the capsule result
  variables. Capsule files in a `capsules` directory beside weefy or in
  `\EFI\UpdateCapsule` are taken apart header by header and compared with
  this machine, never submitted. A test capsule with a made-up GUID can be
  sent with `UpdateCapsule()` and looked for in the configuration table
  after the restart.
- **Boot**: the firmware's boot options and loaders found on disk, each with
  its stored device path and what it resolves to; start one, or hand control
  back to the firmware, restart or power off.

Keys: arrows (or `j`/`k`) move, Enter opens, Tab switches pane, `/` finds,
`n` finds the next, `r` refreshes, Esc (or `q`) goes back, and `b` on the main
menu boots the OS.

## Status

Tested:
- QEMU under edk2, on aarch64 and x86_64
    - graphics console and on a serial-only console
    - the Secure Boot lab end to end, on both, with Secure Boot firmware:
      enrolling the keys, the three load tests, stage two, restarting into
      enforcement, removing the platform key and enrolling again

    - the Capsules screen on both: the empty states, the sample capsule
      files, and the firmware refusing the test capsule (QEMU's edk2 keeps
      no capsule across a reset and has no ESRT)

    - the intro on both: the cartoon on a display with and without an OS
      to find, `[skip dialogue]` kept across a restart, and the text
      version on a serial-only console (aarch64)

Untested:
- The Capsules screen on firmware that does capsule updates: the ESRT and
  Firmware Management Protocol pages, the result variables, and the test
  capsule surviving a reset
- Booting on the home lab as of 11/21/2026. 
- USB ISO image
- Thinkpad x86 boot
- Orange PI 5 ultra boot
- PXE raw image and hook up the netboot tool from hypehv (private repo, sorry)
- The Secure Boot lab on real firmware (only QEMU's edk2 so far)

## Build and try it

```
rustup target add aarch64-unknown-uefi x86_64-unknown-uefi
scripts/run-qemu.sh --fake-os                  # aarch64, in a window
scripts/run-qemu.sh --arch x86_64 --fake-os
scripts/run-qemu.sh --display serial           # console on this terminal (Ctrl-A X quits)
```

`--fake-os` adds a second disk whose loaders are the UEFI Shell, so there is
something to boot; type `exit` in the shell to come back. `--help` lists the
rest.

### Orange Pi 5 Ultra

```
scripts/make-image.sh --board orangepi5-ultra                    # build the card image
scripts/make-image.sh --board orangepi5-ultra --write /dev/sdX   # and write it
```

The card carries the UEFI firmware (edk2-rk3588) as well as weefy; see
`firmware/orangepi5-ultra/README.md` for what the pieces are and where they
came from. Nothing is written to the board's SPI flash or eMMC. The console
is HDMI with a USB keyboard, and the debug UART at 1500000 8N1.

With this card the firmware starts from default settings on every boot, so
weefy finds the OS by looking through the disks for loaders under `\EFI`
rather than through saved boot options.

## Any other UEFI machine

Either boot it from a USB stick:

```
scripts/make-image.sh --arch x86_64 --write /dev/sdX
```

or install it beside the OS on the existing EFI System Partition and put it
first in the boot order:

```
cargo build --release --target x86_64-unknown-uefi
sudo mkdir -p /boot/efi/EFI/weefy
sudo cp target/x86_64-unknown-uefi/release/weefy.efi /boot/efi/EFI/weefy/
sudo efibootmgr --create --disk /dev/nvme0n1 --part 1 --label weefy --loader '\EFI\weefy\weefy.efi'
```

Built this way weefy is not signed. With Secure Boot on, the firmware refuses
to load it: turn Secure Boot off, or sign it with a key the firmware trusts
(next section).

## Secure Boot, end to end

The whole process with keys of your own, on a machine whose firmware has
Secure Boot and is in Setup Mode (no platform key: a new board or virtual
machine, or a PC after "clear Secure Boot keys" in its setup screens).

1. **Make keys, sign, prepare the disk.** One flag does all three:

   ```
   scripts/make-image.sh --secure-boot [--board orangepi5-ultra] [--write /dev/sdX]
   ```

   The first time, this makes three RSA key pairs with self-signed
   certificates in `keys/`: PK (platform key), KEK and db. It signs weefy
   with the db key and puts on the disk, beside weefy:

   - `keys/db.auth`, `KEK.auth`, `PK.auth`: signed updates that enrol the
     three certificates (db signed by KEK, KEK by PK, PK by itself), and
     `noPK.auth`, which removes the platform key again
   - `lab/signed.efi`, `unsigned.efi`, `tampered.efi`: copies of weefy to
     try loading

   `keys/*.key` are the private keys. They never go on the disk; keep them
   if you want to sign for this machine again.

2. **Boot the disk** and open **Secure Boot lab**. It shows Setup Mode, the
   empty PK, KEK, db and dbx, weefy's own signature, and what is inside each
   key file.

3. **Enrol the keys.** weefy writes db, KEK, then PK with `SetVariable()`.
   Enrolling PK ends Setup Mode and, on edk2, enforcement starts at once.

4. **Try the three images.** The signed one loads; the unsigned and the
   tampered one get `ACCESS_DENIED`. The signed one can be started as "stage
   two": a second weefy, which the firmware verified, running inside the
   first. Esc on its main menu returns.

5. **Restart.** The firmware now checks weefy's signature before starting
   it, and the lab says "enforcing".

6. **Remove the platform key** to go back to Setup Mode.

To try it without hardware:

```
scripts/run-qemu.sh --secure-boot                 # aarch64
scripts/run-qemu.sh --secure-boot --arch x86_64
```

`--keep-vars` starts the next run with the keys the last one enrolled.

`tools/sbtool.py` does the signing. The Authenticode hash, the PKCS#7
signature, the signature lists and the signed updates are each written out in
it; python3-cryptography is used only for RSA and for making certificates.
`sbtool.py info --in file.efi` shows an image's hash and signature.

Things to know:

- While the lab's keys are enrolled, the firmware refuses everything not
  signed with the lab's db key, an installed OS included. Remove the platform
  key (or clear the keys in firmware setup) to undo that.
- Enrolling replaces what is in db and KEK. weefy only offers it in Setup
  Mode; don't use it on a machine whose existing db you want to keep.
- On a PC that already has keys, weefy can't add yours to db: that needs an
  update signed by a KEK owner. Put the machine in Setup Mode from its
  firmware setup first, or use the firmware's own "enrol from file" screen
  with `keys/db.cer`.
- The Orange Pi's UEFI card keeps no settings, so keys enrolled there last
  until the power goes. Whether that firmware build has Secure Boot at all
  is the first thing the lab reports.

## Configuration

An optional `weefy.conf` in the same directory as the program:

```
# Boot the OS by itself after this many seconds unless a key is pressed.
# Without it, weefy waits.
timeout=10
# Which OS: the first whose title or path contains this text (or Boot0003).
# Without it, the first usable entry of BootOrder, then the first loader found.
default=fedora
# The intro: off (never), always, or auto (until [skip dialogue] is chosen).
intro=off
```

`make-image.sh` and `run-qemu.sh` write it with `--conf-timeout` and
`--conf-default`; `--conf-intro off` is the one to pass while developing.

### The intro

It is drawn at 320 by 200 in 16 colours and scaled by a whole number onto
the firmware's framebuffer with the Graphics Output Protocol's `Blt()`.
There are no image files: `src/art.rs` draws Weefy and his lab from
rectangles, ellipses and lines each frame, and `src/pixel.rs` has the canvas,
a 5 by 7 font and the code that sends only the changed part of the picture
to the display. A console with no display (a serial port) gets the same
lines as text.

`[skip dialogue]` stores one byte in the variable `SkipIntro` under weefy's
own GUID; choosing it again deletes the variable. Firmware that forgets its
variables at power-off needs `intro=off` in `weefy.conf` instead.

## What it changes

weefy reads. The exceptions: the Secure Boot lab writes PK, KEK and db when
you tell it to enrol or remove keys, after asking; it disarms the firmware's five-minute boot
watchdog while you read and arms it again before starting a loader; it sets
the volatile `BootCurrent` variable when it starts a boot option; and
"Restart into firmware setup" sets bit 0 of `OsIndications`, after asking.
`[skip dialogue]` writes weefy's own `SkipIntro` variable.

## Layout

- `src/main.rs`: entry point, main menu, `weefy.conf`, the countdown
- `src/tui.rs`: the screen buffer, keys, and the list-and-detail browser
  every screen is made of
- `src/learn.rs`: the tour
- `src/intro.rs`, `art.rs`, `pixel.rs`: the intro, its pictures, and the
  canvas and font they are drawn with
- `src/systab.rs`, `services.rs`, `handles.rs`, `vars.rs`, `memmap.rs`: the
  inspection screens
- `src/secboot.rs`: the Secure Boot lab
- `src/capsule.rs`: capsules and firmware updates
- `src/bootopt.rs`, `bootview.rs`: finding what can be booted, booting it
- `src/guids.rs`: explanations; `src/guid_names.rs`: GUID names, generated by
  `tools/gen-guids.py` from the edk2 package declarations in `tools/dec`
- `tools/sbtool.py`: makes keys, signs images, prepares the key updates
- `tools/mkcapsule.py`: makes the sample capsule files both scripts put on
  the disk
- `scripts/`: `run-qemu.sh`, `make-image.sh`
- `keys/`: your Secure Boot keys, once made (kept out of git)
- `firmware/orangepi5-ultra/`: the board's loader and UEFI firmware
