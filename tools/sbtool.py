#!/usr/bin/env python3
"""Secure Boot by hand: makes keys, signs UEFI images and prepares the signed
variable updates that enrol the keys.

It does what sbsign, cert-to-efi-sig-list and sign-efi-sig-list do, in one
file with every structure written out, so that the process can be read. The
only library used is `cryptography`, for RSA and for making certificates.

  sbtool.py keys --dir keys                 make PK, KEK and db, and *.auth
  sbtool.py sign --dir keys --in a.efi --out a-signed.efi
  sbtool.py info --in a.efi                 hash and signature of an image
  sbtool.py lab --dir keys --in weefy.efi --out lab-files
                                            everything weefy's Secure Boot lab
                                            needs on the disk
"""

import argparse
import datetime
import hashlib
import struct
import sys
import uuid
from pathlib import Path

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding, rsa
from cryptography.x509.oid import NameOID

# ---------------------------------------------------------------- DER

def tlv(tag, content):
    """One DER element: tag, length, content."""
    n = len(content)
    if n < 0x80:
        head = bytes([n])
    else:
        size = (n.bit_length() + 7) // 8
        head = bytes([0x80 | size]) + n.to_bytes(size, "big")
    return bytes([tag]) + head + content


def seq(*parts):
    return tlv(0x30, b"".join(parts))


def set_of(*parts):
    # DER wants the members of a SET OF in order of their encodings.
    return tlv(0x31, b"".join(sorted(parts)))


def oid(dotted):
    nums = [int(x) for x in dotted.split(".")]
    out = bytes([nums[0] * 40 + nums[1]])
    for n in nums[2:]:
        chunk = [n & 0x7F]
        while n > 0x7F:
            n >>= 7
            chunk.append(0x80 | (n & 0x7F))
        out += bytes(reversed(chunk))
    return tlv(0x06, out)


def integer(n):
    return tlv(0x02, n.to_bytes(n.bit_length() // 8 + 1, "big"))


NULL = b"\x05\x00"
OID_SIGNED_DATA = oid("1.2.840.113549.1.7.2")
OID_DATA = oid("1.2.840.113549.1.7.1")
OID_SHA256 = oid("2.16.840.1.101.3.4.2.1")
OID_RSA = oid("1.2.840.113549.1.1.1")
OID_CONTENT_TYPE = oid("1.2.840.113549.1.9.3")
OID_MESSAGE_DIGEST = oid("1.2.840.113549.1.9.4")
# Microsoft's Authenticode: "this signature is over a description of a
# file", and "the file is a PE image".
OID_SPC_INDIRECT_DATA = oid("1.3.6.1.4.1.311.2.1.4")
OID_SPC_PE_IMAGE_DATA = oid("1.3.6.1.4.1.311.2.1.15")
SHA256_ALG = seq(OID_SHA256, NULL)


def rsa_sign(key, data):
    return key.sign(data, padding.PKCS1v15(), hashes.SHA256())


def signed_data(cert, key, content_type, content, signed_bytes, attributes):
    """A PKCS#7 SignedData with one signer and its certificate.

    Without attributes the RSA signature is over `signed_bytes` directly
    (the content is elsewhere: a detached signature). With attributes it is
    over the attributes, one of which is the hash of the content.
    """
    if attributes:
        # Signed as a SET, stored with the context tag [0].
        signature = rsa_sign(key, set_of(*attributes))
        attrs = tlv(0xA0, b"".join(sorted(attributes)))
    else:
        signature = rsa_sign(key, signed_bytes)
        attrs = b""
    signer = seq(
        integer(1),
        seq(cert.issuer.public_bytes(), integer(cert.serial_number)),
        SHA256_ALG,
        attrs,
        seq(OID_RSA, NULL),
        tlv(0x04, signature),
    )
    inner = content_type + (tlv(0xA0, content) if content else b"")
    return seq(
        integer(1),
        set_of(SHA256_ALG),
        seq(inner),
        tlv(0xA0, cert.public_bytes(serialization.Encoding.DER)),
        set_of(signer),
    )

# ---------------------------------------------------------------- PE images

class Pe:
    """The few fields of a PE32+ image that signing needs."""

    def __init__(self, data):
        self.data = bytearray(data)
        d = self.data
        if d[:2] != b"MZ":
            raise SystemExit("not a PE image: no MZ header")
        pe = struct.unpack_from("<I", d, 0x3C)[0]
        if d[pe:pe + 4] != b"PE\0\0":
            raise SystemExit("not a PE image: no PE signature")
        self.sections = struct.unpack_from("<H", d, pe + 6)[0]
        opt_size = struct.unpack_from("<H", d, pe + 20)[0]
        opt = pe + 24
        if struct.unpack_from("<H", d, opt)[0] != 0x20B:
            raise SystemExit("only PE32+ (64-bit) images are supported")
        self.size_of_headers = struct.unpack_from("<I", d, opt + 60)[0]
        self.checksum_at = opt + 64
        if struct.unpack_from("<I", d, opt + 108)[0] < 5:
            raise SystemExit("the image has no slot for a certificate table")
        # Data directory 4: where the signatures are. Unlike the others its
        # "address" is an offset in the file: signatures are not loaded.
        self.certdir_at = opt + 112 + 4 * 8
        self.section_table = opt + opt_size

    def certdir(self):
        return struct.unpack_from("<II", self.data, self.certdir_at)

    def authenticode_hash(self):
        """SHA-256 of the image the way Authenticode defines it: everything
        except the three things signing changes (the checksum, the pointer
        to the signatures, and the signatures)."""
        d = self.data
        cert_off, cert_size = self.certdir()
        h = hashlib.sha256()
        h.update(d[:self.checksum_at])
        h.update(d[self.checksum_at + 4:self.certdir_at])
        h.update(d[self.certdir_at + 8:self.size_of_headers])
        ranges = []
        for i in range(self.sections):
            size, ptr = struct.unpack_from("<II", d, self.section_table + 40 * i + 16)
            if size:
                ranges.append((ptr, size))
        hashed = self.size_of_headers
        for ptr, size in sorted(ranges):
            h.update(d[ptr:ptr + size])
            hashed += size
        # Anything after the last section, up to the signatures.
        end = len(d) - (cert_size if cert_off else 0)
        if end > hashed:
            h.update(d[hashed:end])
        return h.digest()

    def set_checksum(self):
        d = self.data
        struct.pack_into("<I", d, self.checksum_at, 0)
        total = 0
        for (word,) in struct.iter_unpack("<H", bytes(d) + b"\0" * (len(d) % 2)):
            total += word
            total = (total & 0xFFFF) + (total >> 16)
        struct.pack_into("<I", d, self.checksum_at, (total & 0xFFFF) + len(d))


def sign_image(data, cert, key):
    pe = Pe(data)
    if pe.certdir() != (0, 0):
        raise SystemExit("the image is already signed")
    # Signatures start on an 8-byte boundary; the padding becomes part of
    # the image and of its hash.
    pe.data += b"\0" * (-len(pe.data) % 8)
    digest = pe.authenticode_hash()

    # What is signed is not the file but this description of it:
    #   SpcIndirectDataContent { "a PE image", { SHA-256, the hash } }
    obsolete = "<<<Obsolete>>>".encode("utf-16-be")
    pe_image_data = seq(tlv(0x03, b"\0"), tlv(0xA0, tlv(0xA2, tlv(0x80, obsolete))))
    indirect_body = seq(OID_SPC_PE_IMAGE_DATA, pe_image_data) + seq(SHA256_ALG, tlv(0x04, digest))
    attributes = [
        seq(OID_CONTENT_TYPE, set_of(OID_SPC_INDIRECT_DATA)),
        seq(OID_MESSAGE_DIGEST, set_of(tlv(0x04, hashlib.sha256(indirect_body).digest()))),
    ]
    sd = signed_data(cert, key, OID_SPC_INDIRECT_DATA, seq(indirect_body), None, attributes)
    pkcs7 = seq(OID_SIGNED_DATA, tlv(0xA0, sd))

    # WIN_CERTIFICATE: length, revision 2.0, type 2 (PKCS#7 SignedData).
    entry = struct.pack("<IHH", 8 + len(pkcs7), 0x0200, 0x0002) + pkcs7
    entry += b"\0" * (-len(entry) % 8)
    struct.pack_into("<II", pe.data, pe.certdir_at, len(pe.data), len(entry))
    pe.data += entry
    pe.set_checksum()
    return bytes(pe.data), digest

# ---------------------------------------------------------------- variables

GLOBAL = uuid.UUID("8be4df61-93ca-11d2-aa0d-00e098032b8c")
IMAGE_SECURITY_DATABASE = uuid.UUID("d719b2cb-3d3a-4596-a3bc-dad00e67656f")
CERT_X509 = uuid.UUID("a5c059a1-94e4-4aa7-87b5-ab155c2bf072")
CERT_TYPE_PKCS7 = uuid.UUID("4aafd29d-68df-49ee-8aa9-347d375665a7")
# NON_VOLATILE | BOOTSERVICE_ACCESS | RUNTIME_ACCESS |
# TIME_BASED_AUTHENTICATED_WRITE_ACCESS
ATTRIBUTES = 0x27


def signature_list(cert, owner):
    """An EFI_SIGNATURE_LIST holding one X.509 certificate."""
    der = cert.public_bytes(serialization.Encoding.DER)
    entry = owner.bytes_le + der
    return CERT_X509.bytes_le + struct.pack("<III", 28 + len(entry), 0, len(entry)) + entry


def auth_update(name, vendor, payload, when, cert, key):
    """What SetVariable() takes for an authenticated variable: a timestamp,
    a signature, then the new contents. The signature covers the variable's
    name, GUID and attributes too, so it can't be replayed onto another."""
    efi_time = struct.pack("<HBBBBBBIhBB", when.year, when.month, when.day,
                           when.hour, when.minute, when.second, 0, 0, 0, 0, 0)
    signed = name.encode("utf-16-le") + vendor.bytes_le + struct.pack("<I", ATTRIBUTES) + efi_time + payload
    # A bare SignedData, without the ContentInfo wrapper: the form the
    # specification asks for and every firmware accepts.
    sd = signed_data(cert, key, OID_DATA, None, signed, None)
    # WIN_CERTIFICATE_UEFI_GUID: length, revision 2.0, type 0x0EF1, GUID.
    wincert = struct.pack("<IHH", 24 + len(sd), 0x0200, 0x0EF1) + CERT_TYPE_PKCS7.bytes_le + sd
    return efi_time + wincert + payload

# ---------------------------------------------------------------- commands

def make_cert(common_name):
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, common_name)])
    now = datetime.datetime.now(datetime.timezone.utc)
    cert = (
        x509.CertificateBuilder()
        .subject_name(name)
        .issuer_name(name)
        .public_key(key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - datetime.timedelta(days=1))
        .not_valid_after(now + datetime.timedelta(days=3650))
        .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
        .sign(key, hashes.SHA256())
    )
    return key, cert


def load_pair(directory, name):
    d = Path(directory)
    try:
        key = serialization.load_pem_private_key((d / f"{name}.key").read_bytes(), None)
        cert = x509.load_pem_x509_certificate((d / f"{name}.crt").read_bytes())
    except FileNotFoundError as e:
        raise SystemExit(f"{e.filename} not found: run `sbtool.py keys --dir {directory}` first")
    return key, cert


def cmd_keys(args):
    d = Path(args.dir)
    if (d / "PK.key").exists() and not args.force:
        raise SystemExit(f"{d} already has keys; pass --force to replace them")
    make_keys(d, args.name)


def make_keys(d, common_name):
    d.mkdir(parents=True, exist_ok=True)
    owner = uuid.uuid4()
    (d / "owner-guid.txt").write_text(f"{owner}\n")
    pairs = {}
    for name in ("PK", "KEK", "db"):
        key, cert = make_cert(f"{common_name} {name}")
        pairs[name] = (key, cert)
        keyfile = d / f"{name}.key"
        keyfile.touch(mode=0o600)
        keyfile.write_bytes(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                              serialization.NoEncryption()))
        (d / f"{name}.crt").write_bytes(cert.public_bytes(serialization.Encoding.PEM))
        (d / f"{name}.cer").write_bytes(cert.public_bytes(serialization.Encoding.DER))
        (d / f"{name}.esl").write_bytes(signature_list(cert, owner))
        print(f"  {name}: {name}.key (private), {name}.crt, {name}.cer, {name}.esl")

    # Each update is signed by the key one level up; PK signs itself.
    now = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0)
    esl = {n: signature_list(c, owner) for n, (_, c) in pairs.items()}
    pk_key, pk_cert = pairs["PK"]
    kek_key, kek_cert = pairs["KEK"]
    files = {
        "db.auth": auth_update("db", IMAGE_SECURITY_DATABASE, esl["db"], now, kek_cert, kek_key),
        "KEK.auth": auth_update("KEK", GLOBAL, esl["KEK"], now, pk_cert, pk_key),
        "PK.auth": auth_update("PK", GLOBAL, esl["PK"], now, pk_cert, pk_key),
        # An empty PK, signed by the PK: deletes it, back to Setup Mode. It
        # must be newer than the PK it replaces.
        "noPK.auth": auth_update("PK", GLOBAL, b"", now + datetime.timedelta(seconds=1), pk_cert, pk_key),
    }
    for name, content in files.items():
        (d / name).write_bytes(content)
    print("  signed updates: db.auth (by KEK), KEK.auth (by PK), PK.auth (by itself), noPK.auth (by PK)")


def cmd_sign(args):
    key, cert = load_pair(args.dir, args.key)
    signed, digest = sign_image(Path(args.input).read_bytes(), cert, key)
    Path(args.output).write_bytes(signed)
    print(f"  {args.output}: signed by \"{cert.subject.rfc4514_string()}\"")
    print(f"  Authenticode SHA-256 {digest.hex()}")


def cmd_info(args):
    pe = Pe(Path(args.input).read_bytes())
    off, size = pe.certdir()
    print(f"Authenticode SHA-256  {pe.authenticode_hash().hex()}")
    if not off:
        print("Signature             none")
        return
    length, revision, kind = struct.unpack_from("<IHH", pe.data, off)
    blob = bytes(pe.data[off + 8:off + length])
    print(f"Signature             {length} bytes at offset {off:#x} (revision {revision:#x}, type {kind})")
    print(f"Signs this hash       {'yes' if pe.authenticode_hash() in blob else 'NO: the image changed after signing'}")


def cmd_lab(args):
    keys, out = Path(args.dir), Path(args.output)
    if not (keys / "PK.key").exists():
        print(f"making keys in {keys}")
        make_keys(keys, args.name)
    else:
        print(f"using the keys in {keys}")
    key, cert = load_pair(keys, "db")
    unsigned = Path(args.input).read_bytes()
    signed, digest = sign_image(unsigned, cert, key)
    # One bit of the MS-DOS stub's message: covered by the hash, used by
    # nothing.
    if struct.unpack_from("<I", signed, 0x3C)[0] < 0x60:
        raise SystemExit("the image has no MS-DOS stub to tamper with")
    tampered = bytearray(signed)
    tampered[0x50] ^= 0x01
    (out / "keys").mkdir(parents=True, exist_ok=True)
    (out / "lab").mkdir(parents=True, exist_ok=True)
    (out / "weefy-signed.efi").write_bytes(signed)
    (out / "lab" / "signed.efi").write_bytes(signed)
    (out / "lab" / "unsigned.efi").write_bytes(unsigned)
    (out / "lab" / "tampered.efi").write_bytes(tampered)
    for name in ("db", "KEK", "PK", "noPK"):
        (out / "keys" / f"{name}.auth").write_bytes((keys / f"{name}.auth").read_bytes())
    print(f"  signed by \"{cert.subject.rfc4514_string()}\", Authenticode SHA-256 {digest.hex()}")
    print(f"  {out}: weefy-signed.efi, keys/{{db,KEK,PK,noPK}}.auth, lab/{{signed,unsigned,tampered}}.efi")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="command", required=True)
    k = sub.add_parser("keys", help="make PK, KEK and db keys, certificates and signed updates")
    k.add_argument("--dir", default="keys", help="where they go (default: %(default)s)")
    k.add_argument("--name", default="weefy lab", help="start of the certificates' names (default: %(default)s)")
    k.add_argument("--force", action="store_true", help="replace existing keys")
    k.set_defaults(func=cmd_keys)
    s = sub.add_parser("sign", help="sign a UEFI image")
    s.add_argument("--dir", default="keys", help="where the keys are (default: %(default)s)")
    s.add_argument("--key", default="db", help="which key signs (default: %(default)s)")
    s.add_argument("--in", dest="input", required=True, help="the image")
    s.add_argument("--out", dest="output", required=True, help="the signed image")
    s.set_defaults(func=cmd_sign)
    i = sub.add_parser("info", help="show an image's hash and signature")
    i.add_argument("--in", dest="input", required=True, help="the image")
    i.set_defaults(func=cmd_info)
    lab = sub.add_parser("lab", help="sign weefy and lay out the files for its Secure Boot lab")
    lab.add_argument("--dir", default="keys", help="where the keys are, or will be made (default: %(default)s)")
    lab.add_argument("--name", default="weefy lab", help="start of new certificates' names (default: %(default)s)")
    lab.add_argument("--in", dest="input", required=True, help="the unsigned weefy.efi")
    lab.add_argument("--out", dest="output", required=True, help="directory for the files")
    lab.set_defaults(func=cmd_lab)
    args = ap.parse_args()
    args.func(args)


if __name__ == "__main__":
    sys.exit(main())
