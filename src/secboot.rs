//! The Secure Boot lab: where the machine stands, how this program is
//! signed, and, with the key files `make-image.sh --secure-boot` puts beside
//! the program, enrolling those keys and watching the firmware refuse an
//! image.
//!
//! This is the one screen that writes to the firmware's settings, and only
//! when asked: the key variables db, KEK and PK.

use crate::bootopt;
use crate::tui::{self, BrowseState, Doc, Item, Key, Outcome, Screen, st};
use crate::util;
use crate::vars;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use uefi::boot::{self, LoadImageSource};
use uefi::proto::BootPolicy;
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::runtime::{self, ResetType, VariableAttributes, VariableVendor};
use uefi::{Guid, Status, guid};

/// The load options this program gives a copy of itself that it starts, so
/// that the copy knows it is one.
pub const STAGE2_OPTIONS: &str = "weefy-stage2";

const CERT_X509: Guid = guid!("a5c059a1-94e4-4aa7-87b5-ab155c2bf072");
const CERT_SHA256: Guid = guid!("c1c41626-504c-4092-aca9-41f936934328");

// ------------------------------------------------------------------ SHA-256

fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01,
        0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
        0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
        0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08,
        0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

// ------------------------------------------------------------------- images

/// What Secure Boot looks at in a PE image.
struct Image {
    /// The Authenticode hash: SHA-256 of the file without its checksum, its
    /// pointer to the signatures, and the signatures.
    hash: [u8; 32],
    /// The first signature (a PKCS#7 SignedData), if the image has one.
    signature: Option<Vec<u8>>,
}

fn inspect(d: &[u8]) -> Option<Image> {
    if d.get(..2)? != b"MZ" {
        return None;
    }
    let pe = util::rd32(d, 0x3c)? as usize;
    if d.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let sections = util::rd16(d, pe + 6)? as usize;
    let opt = pe + 24;
    let table = opt + util::rd16(d, pe + 20)? as usize;
    // PE32+ only: every 64-bit UEFI image.
    if util::rd16(d, opt)? != 0x20b || util::rd32(d, opt + 108)? < 5 {
        return None;
    }
    let headers = util::rd32(d, opt + 60)? as usize;
    let checksum = opt + 64;
    let dir = opt + 112 + 4 * 8;
    let cert_at = util::rd32(d, dir)? as usize;
    let cert_size = if cert_at != 0 { util::rd32(d, dir + 4)? as usize } else { 0 };

    let mut hashed = Vec::with_capacity(d.len());
    hashed.extend_from_slice(d.get(..checksum)?);
    hashed.extend_from_slice(d.get(checksum + 4..dir)?);
    hashed.extend_from_slice(d.get(dir + 8..headers)?);
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for i in 0..sections {
        let size = util::rd32(d, table + 40 * i + 16)? as usize;
        let at = util::rd32(d, table + 40 * i + 20)? as usize;
        if size != 0 {
            ranges.push((at, size));
        }
    }
    ranges.sort();
    let mut done = headers;
    for (at, size) in ranges {
        hashed.extend_from_slice(d.get(at..at.checked_add(size)?)?);
        done += size;
    }
    let end = d.len().checked_sub(cert_size)?;
    if end > done {
        hashed.extend_from_slice(d.get(done..end)?);
    }

    let signature = if cert_at != 0 {
        let len = util::rd32(d, cert_at)? as usize;
        Some(d.get(cert_at + 8..cert_at.checked_add(len)?)?.to_vec())
    } else {
        None
    };
    Some(Image {
        hash: sha256(&hashed),
        signature,
    })
}

// ---------------------------------------------------------------- variables

fn key_var(name: &str) -> Option<Vec<u8>> {
    let vendor = match name {
        "PK" | "KEK" => VariableVendor::GLOBAL_VARIABLE,
        _ => VariableVendor::IMAGE_SECURITY_DATABASE,
    };
    runtime::get_variable_boxed(&util::cstring16(name), &vendor)
        .ok()
        .map(|(data, _)| data.into_vec())
}

fn flag(name: &str) -> Option<bool> {
    bootopt::global_var(name).and_then(|(d, _)| d.first().copied()).map(|b| b == 1)
}

/// The entries of the signature lists in a key variable: (type, value).
fn entries(data: &[u8]) -> Vec<(Guid, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 28 <= data.len() {
        let ty = Guid::from_bytes(data[at..at + 16].try_into().unwrap());
        let list = util::rd32(data, at + 16).unwrap_or(0) as usize;
        let head = util::rd32(data, at + 20).unwrap_or(0) as usize;
        let each = util::rd32(data, at + 24).unwrap_or(0) as usize;
        if list < 28 + head || at + list > data.len() || each <= 16 {
            break;
        }
        for sig in data[at + 28 + head..at + list].chunks_exact(each) {
            out.push((ty, &sig[16..]));
        }
        at += list;
    }
    out
}

struct Mode {
    /// The firmware has Secure Boot at all.
    present: bool,
    /// No platform key: the key variables are open.
    setup: bool,
    /// Signatures are being checked on this boot.
    enforcing: bool,
}

fn mode() -> Mode {
    let secure = flag("SecureBoot");
    Mode {
        present: secure.is_some(),
        setup: flag("SetupMode").unwrap_or(false),
        enforcing: secure.unwrap_or(false),
    }
}

/// The new contents inside a signed update (a *.auth file): what follows
/// the timestamp and the signature.
fn auth_payload(auth: &[u8]) -> Option<&[u8]> {
    let len = util::rd32(auth, 16)? as usize;
    auth.get(16usize.checked_add(len)?..)
}

fn key_file(name: &str) -> Option<Vec<u8>> {
    util::read_own_file(&format!("keys\\{name}.auth"))
}

/// The platform key is the one in keys\PK.auth.
fn pk_is_ours() -> bool {
    match (key_var("PK"), key_file("PK")) {
        (Some(var), Some(file)) => auth_payload(&file).is_some_and(|p| !p.is_empty() && p == &var[..]),
        _ => false,
    }
}

/// What the firmware should say to an image, worked out the way the
/// specification describes, with `db` as the allow list.
fn predict(image: &Image, enforcing: bool, db: &[u8]) -> (bool, String) {
    if !enforcing {
        return (true, String::from("Secure Boot isn't enforcing, so LoadImage() checks nothing."));
    }
    let dbx = key_var("dbx").unwrap_or_default();
    if entries(&dbx).iter().any(|(ty, v)| *ty == CERT_SHA256 && *v == &image.hash[..]) {
        return (false, String::from("its hash is in dbx, the revocation list, which overrides everything else."));
    }
    if entries(db).iter().any(|(ty, v)| *ty == CERT_SHA256 && *v == &image.hash[..]) {
        return (true, String::from("its hash is listed in db: allowed as this exact file, no signature needed."));
    }
    match &image.signature {
        None => (false, String::from("it has no signature and its hash isn't in db.")),
        Some(sig) if !contains(sig, &image.hash) => {
            (false, String::from("it has a signature, but for a different hash: the file was changed after it was signed."))
        }
        Some(sig) => {
            if entries(db).iter().any(|(ty, v)| *ty == CERT_X509 && contains(sig, v)) {
                (true, String::from("its signature is over this hash, and the signer's certificate is in db."))
            } else {
                (false, String::from("it is signed, but not by a certificate in db."))
            }
        }
    }
}

// --------------------------------------------------------------------- rows

const LAB: [(&str, &str, &str); 3] = [
    (
        "signed.efi",
        "Load a signed image",
        "A copy of this program, signed with the lab's db key: the same bytes the firmware started.",
    ),
    (
        "unsigned.efi",
        "Load an unsigned image",
        "The same program as the compiler produced it, with no signature.",
    ),
    (
        "tampered.efi",
        "Load a tampered image",
        "The signed copy with one bit changed afterwards (in the old MS-DOS message at the start of the file, where it does no harm). The signature is still attached and still genuine; it just no longer describes this file.",
    ),
];

#[derive(Clone, Copy)]
enum Row {
    Mode,
    Var(&'static str),
    Own,
    Keys,
    Enroll,
    Try(usize),
    RemovePk,
    Restart,
}

fn mode_page(doc: &mut Doc) {
    let m = mode();
    if !m.present {
        doc.styled(st::HEAD, "This firmware has no Secure Boot.");
        doc.blank();
        doc.p("The SecureBoot variable doesn't exist, which is how a firmware built without the feature shows itself. LoadImage() checks no signatures here, and there are no key variables to enrol anything into.");
        doc.blank();
        doc.p("The rest of this screen still shows how this program is signed. To see enforcement, run the lab under QEMU: scripts/run-qemu.sh --secure-boot");
        return;
    }
    if m.setup {
        doc.styled(st::HEAD, "Setup Mode: no platform key.");
        doc.blank();
        doc.p("Nobody owns this machine's Secure Boot yet. Until a Platform Key (PK) is enrolled, the key variables accept writes from any program that runs before the OS, such as this one, and nothing is enforced.");
        doc.blank();
        doc.p("This is the state of a new board, a new virtual machine, or a PC after \"clear Secure Boot keys\" in its firmware setup. It is the state the lab below needs.");
    } else if m.enforcing {
        doc.styled(st::GOOD, "User Mode, enforcing.");
        doc.blank();
        doc.p("A Platform Key is enrolled and Secure Boot is on: LoadImage() refuses any image that db doesn't vouch for, and the key variables only change through updates signed by the key one level up.");
        doc.blank();
        doc.p("That this screen is showing means the firmware accepted this program.");
    } else {
        doc.styled(st::HEAD, "User Mode, not enforcing.");
        doc.blank();
        doc.p("A Platform Key is enrolled, so the key variables are protected, but checking is switched off (a setting in the firmware's setup screens). Images load unchecked.");
    }
    if !m.setup && key_var("PK").is_none() {
        doc.blank();
        doc.styled(st::HEAD, "PK is empty all the same: this firmware works out its mode when it starts, and will be in Setup Mode after a restart.");
    }
    doc.blank();
    doc.h("The variables that say so");
    let show = |doc: &mut Doc, name: &str, what: &str| {
        let value = match flag(name) {
            Some(true) => "1     ",
            Some(false) => "0     ",
            None => "absent",
        };
        doc.kv(name, format!("{value}  (1 means: {what})"));
    };
    show(doc, "SetupMode", "there is no PK");
    show(doc, "SecureBoot", "signatures are enforced");
    show(doc, "AuditMode", "failures are recorded, not enforced");
    show(doc, "DeployedMode", "locked in user mode");
    show(doc, "VendorKeys", "the keys are untouched since the factory");
    doc.dim("All five are read-only: the firmware computes them from the keys.");
    doc.blank();
    doc.h("The chain");
    doc.bullet("PK, one certificate: its owner may change KEK, and PK itself.");
    doc.bullet("KEK: its owners may change db and dbx.");
    doc.bullet("db: what may run. dbx: what may not, whatever db says.");
    if pk_is_ours() {
        doc.blank();
        doc.styled(st::GOOD, "The platform key is the lab's own: this machine's Secure Boot is yours.");
    }
}

fn var_page(doc: &mut Doc, name: &str) {
    doc.p(match name {
        "PK" => "The Platform Key: one certificate. Whoever holds its private key owns the machine's Secure Boot: they can replace KEK, and replace or remove PK. Enrolling it is what ends Setup Mode.",
        "KEK" => "The Key Exchange Keys: certificates whose owners may update db and dbx. On a PC this is usually the manufacturer and Microsoft, which is how revocations arrive through OS updates.",
        "db" => "The allow list. An image loads if it is signed by a certificate here (or by one that a certificate here issued), or if its own hash is listed.",
        _ => "The revocation list: certificates and hashes of images that are refused even though db would allow them. This is how a loader with a known hole is retired.",
    });
    doc.blank();
    match key_var(name) {
        Some(data) => {
            doc.kv("Size", format!("{} bytes, {} entries", data.len(), entries(&data).len()));
            vars::signature_lists(doc, &data);
        }
        None => doc.dim("The variable doesn't exist: empty."),
    }
    doc.blank();
    doc.dim("The Variables screen shows the same variable with its attributes and raw bytes.");
}

fn image_facts(doc: &mut Doc, data: &[u8], m: &Mode) -> Option<bool> {
    let Some(image) = inspect(data) else {
        doc.styled(st::BAD, "Not a 64-bit PE image.");
        return None;
    };
    doc.kv("File size", util::size(data.len() as u64));
    doc.kv("Authenticode hash", "SHA-256, computed here:");
    doc.raw(st::BRIGHT, format!("  {}", hex(&image.hash)));
    match &image.signature {
        None => doc.kv_styled("Signature", st::HEAD, "none"),
        Some(sig) => {
            doc.kv("Signature", format!("{} bytes of PKCS#7 at the end of the file", sig.len()));
            let names = vars::common_names(sig);
            if !names.is_empty() {
                doc.kv("Signed by", names.join("; "));
            }
            if contains(sig, &image.hash) {
                doc.kv_styled("Hash it signs", st::GOOD, "the one above: the file is as it was signed");
            } else {
                doc.kv_styled("Hash it signs", st::BAD, "a different one: the file changed after signing");
            }
        }
    }
    let db = key_var("db").unwrap_or_default();
    let (allow, why) = predict(&image, m.enforcing, &db);
    doc.blank();
    if allow {
        doc.styled(st::GOOD, &format!("The firmware should load it: {why}"));
    } else {
        doc.styled(st::BAD, &format!("The firmware should refuse it: {why}"));
    }
    Some(allow)
}

fn own_page(doc: &mut Doc) {
    doc.p("The file this program was started from, read back from the disk. A signature isn't loaded into memory with the image, so the file is the only place to see it.");
    doc.blank();
    let Some((_, dir)) = util::own_location() else {
        doc.dim("This program wasn't loaded from a file system, so there is no file to read.");
        return;
    };
    let name = util::open::<LoadedImage>(boot::image_handle())
        .and_then(|i| util::dp_file_name(i.file_path()?.as_bytes()))
        .unwrap_or_default();
    doc.kv("File", name.clone());
    let leaf = name.strip_prefix(&dir).unwrap_or(&name).trim_start_matches('\\');
    match util::read_own_file(leaf) {
        Some(data) => {
            image_facts(doc, &data, &mode());
        }
        None => doc.dim("The file could not be read."),
    }
    doc.blank();
    doc.h("How signing an image works");
    doc.bullet("Hash the file, leaving out the three fields signing itself changes: the checksum, the pointer to the signatures, and the signatures.");
    doc.bullet("Wrap that hash in a small structure that says \"a PE image with this SHA-256\".");
    doc.bullet("Sign the structure with the private key (RSA here), and package signature and certificate as PKCS#7.");
    doc.bullet("Append the package to the file and point the header's certificate table at it.");
    doc.dim("tools/sbtool.py does these steps in about forty lines of Python.");
}

fn keys_page(doc: &mut Doc) {
    doc.p("Signed updates for the key variables, made on another computer by scripts/make-image.sh --secure-boot and put beside this program, in the keys directory. The private keys stayed on that computer.");
    doc.blank();
    let mut any = false;
    for (name, what) in [
        ("db", "puts the lab's signing certificate in db; signed by the lab's KEK"),
        ("KEK", "puts the lab's KEK certificate in KEK; signed by the lab's PK"),
        ("PK", "makes the lab's PK the platform key; signed by itself"),
        ("noPK", "empties PK, back to Setup Mode; signed by the lab's PK"),
    ] {
        let Some(auth) = key_file(name) else {
            continue;
        };
        any = true;
        doc.h(&format!("keys\\{name}.auth"));
        doc.dim(what);
        if let (Some(y), Some(payload)) = (util::rd16(&auth, 0), auth_payload(&auth)) {
            doc.kv("Timestamp", format!("{y:04}-{:02}-{:02} {:02}:{:02}:{:02}", auth[2], auth[3], auth[4], auth[5], auth[6]));
            let sig = &auth[16..auth.len() - payload.len()];
            doc.kv("Signature", format!("{} bytes, by {}", sig.len(), vars::common_names(sig).first().cloned().unwrap_or_default()));
            if payload.is_empty() {
                doc.kv("New contents", "nothing: this update deletes the variable");
            } else {
                let names: Vec<String> = entries(payload).iter().flat_map(|(_, v)| vars::common_names(v)).collect();
                doc.kv("New contents", format!("{} bytes: certificate {}", payload.len(), names.join("; ")));
            }
        }
    }
    if !any {
        doc.styled(st::HEAD, "No key files here.");
        doc.blank();
        doc.p("This copy of weefy was built without the lab. On the computer you build on:");
        doc.raw(st::BRIGHT, "  scripts/make-image.sh --secure-boot");
        doc.p("makes the keys, signs weefy with them and puts the signed updates and three test images on the disk.");
        return;
    }
    doc.blank();
    doc.h("What is in an update");
    doc.p("A timestamp, a PKCS#7 signature, then the new contents. The signature covers the variable's name, GUID and attributes, the timestamp and the contents, so an update can't be replayed onto another variable, and an older one can't be replayed over a newer.");
    doc.p("That is how an OS changes these variables: it cannot sign, but it can deliver an update someone with the key signed elsewhere.");
}

fn enroll_page(doc: &mut Doc) {
    let m = mode();
    doc.p("Writes the lab's three keys into the firmware with SetVariable(), in the order that keeps each step allowed:");
    doc.bullet("db first: the certificate this program is signed with.");
    doc.bullet("KEK next: the key that may change db later.");
    doc.bullet("PK last. The moment it is in, Setup Mode ends: the variables lock and, on most firmware, enforcement starts at once.");
    doc.blank();
    if !m.present {
        doc.styled(st::HEAD, "Not possible here: this firmware has no Secure Boot.");
    } else if !m.setup {
        doc.styled(st::HEAD, "Not possible now: the machine has a platform key.");
        doc.p("Only an update signed by that key's owner can change the keys. To take ownership, clear the Secure Boot keys in the firmware's setup screens (which puts it in Setup Mode), then come back.");
        doc.dim("On a PC with an OS installed, the practical way to add your own key is different: shim's Machine Owner Key list (mokutil --import), which needs no change to db.");
    } else if key_file("PK").is_none() {
        doc.styled(st::HEAD, "No key files beside this program. See \"The key files\".");
    } else {
        doc.styled(st::GOOD, "Ready: the machine is in Setup Mode and the key files are here.");
        doc.p("Enter asks once more, then writes.");
        doc.blank();
        doc.dim("These are non-volatile variables: on a machine that keeps its settings, they stay until removed. db and KEK are replaced, not added to. \"Remove the platform key\" below undoes the last step.");
    }
}

fn try_page(doc: &mut Doc, index: usize) {
    let (file, _, about) = LAB[index];
    doc.p(about);
    doc.blank();
    doc.kv("File", format!("lab\\{file}"));
    match util::read_own_file(&format!("lab\\{file}")) {
        Some(data) => {
            image_facts(doc, &data, &mode());
            doc.blank();
            doc.p("Enter calls LoadImage() on it and shows what the firmware really says.");
        }
        None => {
            doc.dim("Not on this disk. scripts/make-image.sh --secure-boot puts it there.");
        }
    }
}

fn remove_page(doc: &mut Doc) {
    doc.p("Writes an empty PK, using an update the lab's PK signed in advance (keys\\noPK.auth). With no platform key the machine is back in Setup Mode and enforcement stops. db and KEK keep the lab's certificates; without a PK they have no effect.");
    doc.blank();
    if pk_is_ours() {
        doc.styled(st::GOOD, "Possible: the platform key is the lab's.");
    } else if key_var("PK").is_some() {
        doc.styled(st::HEAD, "Not possible: the platform key isn't the lab's, so the lab has no update the firmware would accept.");
    } else {
        doc.dim("There is no platform key to remove.");
    }
}

// ------------------------------------------------------------------ actions

fn notice(scr: &mut Screen, title: &str, lines: &[(tui::Style, String)]) -> Key {
    tui::message(
        scr,
        title,
        &|w| {
            let mut d = Doc::new(w);
            for (style, text) in lines {
                if text.is_empty() {
                    d.blank();
                } else {
                    d.styled(*style, text);
                }
            }
            d
        },
        &[("Any key", "Continue")],
    )
}

fn set_failure(status: Status) -> String {
    match status {
        Status::SECURITY_VIOLATION => String::from("SECURITY_VIOLATION: the firmware rejected the update's signature or timestamp"),
        Status::WRITE_PROTECTED => String::from("WRITE_PROTECTED: the firmware has locked this variable"),
        Status::INVALID_PARAMETER => String::from("INVALID_PARAMETER: the firmware didn't accept the update's format"),
        Status::OUT_OF_RESOURCES => String::from("OUT_OF_RESOURCES: the variable store is full"),
        Status::UNSUPPORTED | Status::NOT_FOUND => String::from("this firmware has no such variable"),
        s => format!("{s:?}"),
    }
}

fn write_auth(name: &str, auth: &[u8]) -> uefi::Result {
    let vendor = match name {
        "PK" | "KEK" => VariableVendor::GLOBAL_VARIABLE,
        _ => VariableVendor::IMAGE_SECURITY_DATABASE,
    };
    runtime::set_variable(
        &util::cstring16(name),
        &vendor,
        VariableAttributes::NON_VOLATILE
            | VariableAttributes::BOOTSERVICE_ACCESS
            | VariableAttributes::RUNTIME_ACCESS
            | VariableAttributes::TIME_BASED_AUTHENTICATED_WRITE_ACCESS,
        auth,
    )
}

fn enroll(scr: &mut Screen) {
    let m = mode();
    let files: Vec<Option<Vec<u8>>> = ["db", "KEK", "PK"].iter().map(|n| key_file(n)).collect();
    if !m.present || !m.setup || files.iter().any(|f| f.is_none()) {
        notice(scr, "Can't enrol", &[(st::NORMAL, String::from("The page on the right says why: the machine must be in Setup Mode and db.auth, KEK.auth and PK.auth must be in the keys directory."))]);
        return;
    }
    // Would this program still be let in afterwards?
    let new_db = files[0].as_deref().and_then(auth_payload).unwrap_or_default();
    let own_ok = util::open::<LoadedImage>(boot::image_handle())
        .and_then(|i| util::dp_file_name(i.file_path()?.as_bytes()))
        .and_then(|name| {
            let (_, dir) = util::own_location()?;
            let leaf = String::from(name.strip_prefix(&dir).unwrap_or(&name).trim_start_matches('\\'));
            util::read_own_file(&leaf)
        })
        .and_then(|data| inspect(&data))
        .is_some_and(|image| predict(&image, true, new_db).0);
    let key = tui::message(
        scr,
        "Enrol the lab's keys?",
        &|w| {
            let mut d = Doc::new(w);
            d.p("This writes db, KEK and PK. From then on this firmware only starts images signed with the lab's db key, until the platform key is removed.");
            d.blank();
            if own_ok {
                d.styled(st::GOOD, "This program is signed with that key, so it will still start, and can undo this.");
            } else {
                d.styled(st::BAD, "This program is NOT signed with that key: after a restart the firmware will refuse to start it, and the keys will have to be cleared in the firmware's own setup screens.");
            }
            d.p("Any other OS or loader on this machine will be refused while these keys are in place.");
            d
        },
        &[("Enter", "Write the keys"), ("Esc", "Cancel")],
    );
    if key != Key::Enter {
        return;
    }
    let mut lines: Vec<(tui::Style, String)> = Vec::new();
    let mut failed = false;
    for (name, auth) in ["db", "KEK", "PK"].iter().zip(&files) {
        let auth = auth.as_deref().unwrap_or_default();
        if key_var(name).is_some_and(|v| auth_payload(auth) == Some(&v[..])) {
            lines.push((st::NORMAL, format!("{name}: already holds the lab's certificate")));
            continue;
        }
        match write_auth(name, auth) {
            Ok(()) => lines.push((st::GOOD, format!("{name}: SetVariable() succeeded"))),
            Err(e) => {
                lines.push((st::BAD, format!("{name}: {}", set_failure(e.status()))));
                lines.push((st::NORMAL, String::from("Stopped there, so that no key is in place without the ones before it.")));
                failed = true;
                break;
            }
        }
    }
    if !failed {
        let after = mode();
        lines.push((st::NORMAL, String::new()));
        lines.push((st::NORMAL, format!("SetupMode is now {}, SecureBoot is {}.", after.setup as u8, after.enforcing as u8)));
        lines.push((
            st::NORMAL,
            String::from(if after.enforcing {
                "Enforcement is on already. Try the three test images."
            } else {
                "This firmware starts enforcing at the next restart. Restart from this screen, then try the three test images."
            }),
        ));
    }
    notice(scr, "Enrolling", &lines);
}

fn remove_pk(scr: &mut Screen) {
    let Some(auth) = key_file("noPK").filter(|_| pk_is_ours()) else {
        notice(scr, "Can't remove it", &[(st::NORMAL, String::from("The platform key isn't the lab's, or keys\\noPK.auth is missing."))]);
        return;
    };
    let key = tui::message(
        scr,
        "Remove the platform key?",
        &|w| {
            let mut d = Doc::new(w);
            d.p("The machine goes back to Setup Mode: nothing is enforced and the key variables are open again.");
            d
        },
        &[("Enter", "Remove it"), ("Esc", "Cancel")],
    );
    if key != Key::Enter {
        return;
    }
    let line = match write_auth("PK", &auth) {
        Ok(()) => {
            let after = mode();
            if after.setup {
                (st::GOOD, format!("PK is empty. SetupMode is now 1, SecureBoot is {}.", after.enforcing as u8))
            } else {
                (st::HEAD, String::from("PK is empty, but SetupMode still reads 0: this firmware works out its mode when it starts. Restart, and it will be in Setup Mode."))
            }
        }
        Err(e) => (st::BAD, set_failure(e.status())),
    };
    notice(scr, "Removing the platform key", &[line]);
}

/// Starts an image this program loaded as "stage two": a copy of itself that
/// the firmware has just verified.
fn start_stage2(scr: &mut Screen, image: uefi::Handle) {
    let options: &'static [u16] = Vec::leak(STAGE2_OPTIONS.encode_utf16().chain(core::iter::once(0)).collect());
    if let Some(mut loaded) = util::open::<LoadedImage>(image) {
        // SAFETY: the buffer is leaked, so it outlives the image.
        unsafe { loaded.set_load_options(options.as_ptr().cast(), (options.len() * 2) as u32) };
    }
    scr.release();
    let result = boot::start_image(image);
    let _ = boot::set_watchdog_timer(0, 0x10000, None);
    tui::drain_keys();
    scr.reset();
    let status = match result {
        Ok(()) => Status::SUCCESS,
        Err(e) => e.status(),
    };
    notice(
        scr,
        "Back in stage one",
        &[(st::NORMAL, format!("Stage two exited with {}, and StartImage() returned here.", bootopt::status_text(status)))],
    );
}

fn try_load(scr: &mut Screen, index: usize) {
    let (file, _, _) = LAB[index];
    let Some(data) = util::read_own_file(&format!("lab\\{file}")) else {
        notice(scr, "Not there", &[(st::NORMAL, format!("lab\\{file} isn't on this disk."))]);
        return;
    };
    let m = mode();
    let expected = inspect(&data).map(|image| predict(&image, m.enforcing, &key_var("db").unwrap_or_default()).0);
    let Some((device, dir)) = util::own_location() else {
        return;
    };
    let Some(device_path) = util::handle_dp(device) else {
        return;
    };
    // The way a boot manager loads: by device path, so that the firmware
    // reads the file itself and knows where it came from.
    let mut path = util::dp_body(&device_path).to_vec();
    path.extend_from_slice(&util::dp_file_node(&format!("{dir}\\lab\\{file}")));
    path.extend_from_slice(&util::DP_END);
    let Ok(dp) = <&DevicePath>::try_from(&path[..]) else {
        return;
    };
    let result = boot::load_image(
        boot::image_handle(),
        LoadImageSource::FromDevicePath {
            device_path: dp,
            boot_policy: BootPolicy::BootSelection,
        },
    );
    let agrees = |allowed: bool| match expected {
        Some(e) if e == allowed => (st::NORMAL, String::from("That is what the rules predicted.")),
        Some(_) => (st::HEAD, String::from("That is not what this program predicted: this firmware decides differently.")),
        None => (st::NORMAL, String::new()),
    };
    match result {
        Ok(image) => {
            let key = tui::message(
                scr,
                "LoadImage() succeeded",
                &|w| {
                    let mut d = Doc::new(w);
                    d.styled(st::GOOD, &format!("The firmware loaded lab\\{file}."));
                    if m.enforcing {
                        d.p("Secure Boot is enforcing, so it verified the image against db first.");
                    } else {
                        d.p("Secure Boot isn't enforcing, so nothing was checked: any image loads.");
                    }
                    let (style, text) = agrees(true);
                    d.styled(style, &text);
                    d.blank();
                    d.p("The image is in memory with a handle of its own, not yet started. Enter starts it as stage two: a second weefy, running inside this one's StartImage() call. Leaving it comes back here.");
                    d
                },
                &[("Enter", "Start stage two"), ("Esc", "Unload it")],
            );
            if key == Key::Enter {
                start_stage2(scr, image);
            } else {
                let _ = boot::unload_image(image);
            }
        }
        Err(e) => {
            let status = e.status();
            let refused = matches!(status, Status::SECURITY_VIOLATION | Status::ACCESS_DENIED);
            let mut lines = Vec::new();
            if refused {
                lines.push((st::BAD, format!("The firmware refused lab\\{file}: {status:?}.")));
                lines.push((st::NORMAL, String::from("Secure Boot found nothing in db that vouches for the image. It was never given control.")));
                lines.push(agrees(false));
                lines.push((st::NORMAL, String::new()));
                lines.push((st::DIM, String::from("ACCESS_DENIED: not loaded at all. SECURITY_VIOLATION: loaded but not startable, left for the platform to decide. Which one a firmware returns is its policy.")));
            } else {
                lines.push((st::BAD, format!("LoadImage() failed with {status:?}, which is not a Secure Boot refusal.")));
            }
            notice(scr, "LoadImage() refused", &lines);
        }
    }
}

pub fn run(scr: &mut Screen) {
    let mut state = BrowseState::default();
    loop {
        let m = mode();
        let mut rows: Vec<(Option<Row>, String, tui::Style)> = Vec::new();
        let on_off = if !m.present {
            "not in this firmware"
        } else if m.setup {
            "Setup Mode"
        } else if m.enforcing {
            "enforcing"
        } else {
            "keys enrolled, off"
        };
        rows.push((None, "This machine".into(), st::NORMAL));
        rows.push((Some(Row::Mode), format!("State: {on_off}"), if m.enforcing { st::GOOD } else { st::NORMAL }));
        for name in ["PK", "KEK", "db", "dbx"] {
            let count = key_var(name).map(|d| entries(&d).len()).unwrap_or(0);
            let noun = if count == 1 { "entry" } else { "entries" };
            rows.push((Some(Row::Var(name)), format!("{name:<4} {count} {noun}"), st::NORMAL));
        }
        rows.push((None, "This program".into(), st::NORMAL));
        rows.push((Some(Row::Own), "Its signature".into(), st::NORMAL));
        rows.push((None, "The lab".into(), st::NORMAL));
        rows.push((Some(Row::Keys), "The key files".into(), st::NORMAL));
        rows.push((Some(Row::Enroll), "Enrol the keys".into(), st::NORMAL));
        for (i, (_, label, _)) in LAB.iter().enumerate() {
            rows.push((Some(Row::Try(i)), (*label).into(), st::NORMAL));
        }
        rows.push((Some(Row::RemovePk), "Remove the platform key".into(), st::NORMAL));
        rows.push((Some(Row::Restart), "Restart".into(), st::NORMAL));
        let items: Vec<Item> = rows
            .iter()
            .map(|(row, label, style)| match row {
                None => Item::header(label.clone()),
                Some(_) => Item::new(label.clone()).styled(*style),
            })
            .collect();

        let out = tui::browse(scr, &mut state, "Secure Boot lab", "Secure Boot", &items, &[], &mut |row, width| {
            let mut doc = Doc::new(width);
            match rows[row].0 {
                Some(Row::Mode) => mode_page(&mut doc),
                Some(Row::Var(name)) => var_page(&mut doc, name),
                Some(Row::Own) => own_page(&mut doc),
                Some(Row::Keys) => keys_page(&mut doc),
                Some(Row::Enroll) => enroll_page(&mut doc),
                Some(Row::Try(i)) => try_page(&mut doc, i),
                Some(Row::RemovePk) => remove_page(&mut doc),
                Some(Row::Restart) => {
                    doc.p("Resets the machine with ResetSystem(). Firmware that only starts enforcing at boot needs this after the keys are enrolled.");
                    doc.blank();
                    doc.p("If the keys were kept, the firmware will check this program's signature before starting it, and this screen will say \"enforcing\".");
                    doc.dim("A machine whose firmware doesn't keep its settings (the Orange Pi's UEFI card) forgets the keys instead.");
                }
                None => {}
            }
            doc
        });
        match out {
            Outcome::Back => return,
            Outcome::Open(row) => match rows[row].0 {
                Some(Row::Enroll) => enroll(scr),
                Some(Row::Try(i)) => try_load(scr, i),
                Some(Row::RemovePk) => remove_pk(scr),
                Some(Row::Restart) => {
                    let key = tui::message(
                        scr,
                        "Restart?",
                        &|w| {
                            let mut d = Doc::new(w);
                            d.p("The machine resets now.");
                            d
                        },
                        &[("Enter", "Restart"), ("Esc", "Cancel")],
                    );
                    if key == Key::Enter {
                        scr.release();
                        runtime::reset(ResetType::COLD, Status::SUCCESS, None);
                    }
                }
                _ => state.zoom = true,
            },
            Outcome::Char(_) => {}
        }
    }
}
