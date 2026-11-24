#!/usr/bin/env python3
"""Makes sample firmware management capsules for weefy's Capsules screen to
take apart, with every structure written out so that the format can be read.

The "firmware" inside is a few kilobytes of filler for a component that no
machine has, so the files are safe to leave on a disk: a firmware that was
handed one would find nothing to apply it to.

  mkcapsule.py --out capsules           sample-unsigned.cap
  mkcapsule.py --out capsules --sign    and sample-signed.cap, signed with a
                                        key made for the occasion (needs the
                                        `cryptography` package, like sbtool.py)
"""

import argparse
import struct
import sys
import uuid
from pathlib import Path

FMP_CAPSULE = uuid.UUID("6dcbd5ed-e82d-4c44-bda1-7194199ad92a")
CERT_TYPE_PKCS7 = uuid.UUID("4aafd29d-68df-49ee-8aa9-347d375665a7")
# The component the samples claim to update: a GUID made up for them.
SAMPLE_COMPONENT = "0f1e2a6c-93d7-4b58-8a41-c2d9e07b35f4"

PERSIST_ACROSS_RESET = 0x00010000
INITIATE_RESET = 0x00040000


def payload_header(version, lowest):
    """edk2's FMP_PAYLOAD_HEADER: 'MSS1', its own size, the firmware's
    version and the lowest version that may replace it later."""
    return struct.pack("<4sIII", b"MSS1", 16, version, lowest)


def authentication(body, count, cert, key):
    """EFI_FIRMWARE_IMAGE_AUTHENTICATION: a counter, then a detached PKCS#7
    signature in a WIN_CERTIFICATE_UEFI_GUID. What is signed is the image
    followed by the counter."""
    import sbtool

    counter = struct.pack("<Q", count)
    sd = sbtool.signed_data(cert, key, sbtool.OID_DATA, None, body + counter, None)
    wincert = struct.pack("<IHH", 24 + len(sd), 0x0200, 0x0EF1) + CERT_TYPE_PKCS7.bytes_le + sd
    return counter + wincert


def capsule(component, version, lowest, firmware, signer=None):
    body = payload_header(version, lowest) + firmware
    image = (authentication(body, 1, *signer) if signer else b"") + body

    # EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER, version 3.
    image_header = struct.pack(
        "<I16sB3sIIQQ",
        3,                      # Version
        component.bytes_le,     # UpdateImageTypeId
        1,                      # UpdateImageIndex
        b"\0\0\0",              # reserved
        len(image),             # UpdateImageSize
        0,                      # UpdateVendorCodeSize
        0,                      # UpdateHardwareInstance: any
        1 if signer else 0,     # ImageCapsuleSupport: bit 0, authentication
    )
    # EFI_FIRMWARE_MANAGEMENT_CAPSULE_HEADER: no drivers, one payload, and
    # the payload's offset from the start of this header.
    fmp_header = struct.pack("<IHHQ", 1, 0, 1, 16)
    rest = fmp_header + image_header + image

    # EFI_CAPSULE_HEADER.
    header = struct.pack("<16sIII", FMP_CAPSULE.bytes_le, 28, PERSIST_ACROSS_RESET | INITIATE_RESET, 28 + len(rest))
    return header + rest


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--out", default="capsules", help="directory for the files (default: %(default)s)")
    p.add_argument("--sign", action="store_true", help="also make a signed sample (needs `cryptography`)")
    p.add_argument("--component", default=SAMPLE_COMPONENT,
                   help="the image type GUID the samples are for (default: %(default)s)")
    p.add_argument("--fw-version", type=lambda s: int(s, 0), default=0x00010002,
                   help="the firmware version inside (default: 0x00010002)")
    p.add_argument("--lowest", type=lambda s: int(s, 0), default=0x00010000,
                   help="the lowest supported version inside (default: 0x00010000)")
    p.add_argument("--name", default="weefy sample firmware vendor",
                   help="the signing certificate's name (default: %(default)s)")
    args = p.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    component = uuid.UUID(args.component)
    firmware = (b"weefy sample firmware: filler, not code. " * 100)[:4096]

    made = ["sample-unsigned.cap"]
    (out / made[0]).write_bytes(capsule(component, args.fw_version, args.lowest, firmware))
    if args.sign:
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        import sbtool

        key, cert = sbtool.make_cert(args.name)
        (out / "sample-signed.cap").write_bytes(capsule(component, args.fw_version, args.lowest, firmware, (cert, key)))
        made.append("sample-signed.cap")
    print(f"  {out}: {', '.join(made)}")


if __name__ == "__main__":
    main()
