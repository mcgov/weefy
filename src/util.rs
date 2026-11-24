//! Small helpers shared by the screens.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, ScopedProtocol};
use uefi::proto::ProtocolPointer;
use uefi::proto::device_path::DevicePath;
use uefi::proto::device_path::text::{AllowShortcuts, DisplayOnly};
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::{CString16, Handle};

/// Opens a protocol on a handle without taking it from anyone: the way an
/// application looks at a protocol that drivers may be using.
pub fn open<P: ProtocolPointer + ?Sized>(handle: Handle) -> Option<ScopedProtocol<P>> {
    // SAFETY: GetProtocol doesn't stop the firmware removing the interface,
    // but nothing runs between this call and the use of the result except
    // this single-threaded program, which never disconnects drivers.
    unsafe {
        boot::open_protocol::<P>(
            OpenProtocolParams {
                handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
        .ok()
    }
}

/// UCS-2 code units up to a NUL, as a String.
pub fn ucs2_to_string(units: impl Iterator<Item = u16>) -> String {
    char::decode_utf16(units.take_while(|&u| u != 0))
        .map(|r| r.unwrap_or('?'))
        .collect()
}

/// A NUL-terminated UCS-2 string at the start of `bytes`: the text, and how
/// many bytes it takes with its terminator.
pub fn ucs2_bytes(bytes: &[u8]) -> (String, usize) {
    let units = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]));
    let n = units.clone().take_while(|&u| u != 0).count();
    (ucs2_to_string(units), ((n + 1) * 2).min(bytes.len()))
}

pub fn cstring16(s: &str) -> CString16 {
    CString16::try_from(s).unwrap_or_default()
}

pub fn rd16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

pub fn rd32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

pub fn rd64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// A size in bytes, in the largest unit that keeps it readable.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KiB", "MiB", "GiB", "TiB"];
    let mut unit = 0;
    let mut whole = bytes;
    let mut rem = 0;
    while whole >= 1024 && unit < UNITS.len() - 1 {
        rem = whole % 1024;
        whole /= 1024;
        unit += 1;
    }
    if unit == 0 {
        format!("{whole} bytes")
    } else if whole >= 100 || rem * 10 / 1024 == 0 {
        format!("{whole} {}", UNITS[unit])
    } else {
        format!("{whole}.{} {}", rem * 10 / 1024, UNITS[unit])
    }
}

/// A device path as text, by the firmware's own converter if it has one.
pub fn dp_text(dp: &DevicePath) -> String {
    if let Ok(s) = dp.to_string16(DisplayOnly(false), AllowShortcuts(false)) {
        return String::from(&*s);
    }
    // No Device Path To Text protocol: name each node by its numbers.
    let mut out = String::new();
    for node in dp.node_iter() {
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&format!(
            "Path({},{},{} bytes)",
            node.device_type().0,
            node.sub_type().0,
            node.length()
        ));
    }
    out
}

/// The nodes of a device path held as bytes: (type, subtype, the whole node).
/// Stops at the end node or at anything malformed.
pub fn dp_nodes(bytes: &[u8]) -> Vec<(u8, u8, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 4 <= bytes.len() {
        let len = u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        if len < 4 || at + len > bytes.len() {
            break;
        }
        let (ty, sub) = (bytes[at], bytes[at + 1]);
        if ty == 0x7f {
            break;
        }
        out.push((ty, sub, &bytes[at..at + len]));
        at += len;
    }
    out
}

pub const DP_END: [u8; 4] = [0x7f, 0xff, 0x04, 0x00];

/// The bytes of a device path up to, not including, its end node.
pub fn dp_body(bytes: &[u8]) -> &[u8] {
    let len: usize = dp_nodes(bytes).iter().map(|n| n.2.len()).sum();
    &bytes[..len]
}

/// A device path held as bytes, as text.
pub fn dp_bytes_text(bytes: &[u8]) -> String {
    // Only the first instance, ended properly: what the converter is given
    // must be well formed whatever the variable held.
    let mut clean = dp_body(bytes).to_vec();
    if clean.is_empty() {
        return String::from("(empty)");
    }
    clean.extend_from_slice(&DP_END);
    match <&DevicePath>::try_from(&clean[..]) {
        Ok(dp) => dp_text(dp),
        Err(_) => String::from("(malformed device path)"),
    }
}

/// A file path node for `path` (backslash separated).
pub fn dp_file_node(path: &str) -> Vec<u8> {
    let units: Vec<u16> = path.encode_utf16().chain(core::iter::once(0)).collect();
    let len = 4 + units.len() * 2;
    let mut out = alloc::vec![0x04, 0x04, len as u8, (len >> 8) as u8];
    for u in units {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

/// The file name in a device path's file path nodes, if it has any.
pub fn dp_file_name(bytes: &[u8]) -> Option<String> {
    let mut name = String::new();
    for (ty, sub, node) in dp_nodes(bytes) {
        if ty == 4 && sub == 4 {
            let (part, _) = ucs2_bytes(&node[4..]);
            if !name.is_empty() && !name.ends_with('\\') && !part.starts_with('\\') {
                name.push('\\');
            }
            name.push_str(&part);
        }
    }
    (!name.is_empty()).then_some(name)
}

/// The device path installed on a handle, as bytes.
pub fn handle_dp(handle: Handle) -> Option<Vec<u8>> {
    open::<DevicePath>(handle).map(|dp| dp.as_bytes().to_vec())
}

/// The device this program was loaded from and the directory it is in.
pub fn own_location() -> Option<(Handle, String)> {
    let image = open::<LoadedImage>(boot::image_handle())?;
    let own = dp_file_name(image.file_path()?.as_bytes())?;
    let dir = own.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
    Some((image.device()?, String::from(dir)))
}

/// A whole file from this program's own directory (`name` may have
/// subdirectories, separated by backslashes).
pub fn read_own_file(name: &str) -> Option<Vec<u8>> {
    let (device, dir) = own_location()?;
    // Nothing this program reads from beside itself is larger.
    let (data, size) = read_file(device, &format!("{dir}\\{name}"), 16 << 20)?;
    (data.len() as u64 == size).then_some(data)
}

/// The first `max` bytes of a file on a device's file system (`path` from
/// its root), and the file's full size.
pub fn read_file(device: Handle, path: &str, max: usize) -> Option<(Vec<u8>, u64)> {
    let mut fs = open::<SimpleFileSystem>(device)?;
    let mut root = fs.open_volume().ok()?;
    let mut file = root.open(&cstring16(path), FileMode::Read, FileAttribute::empty()).ok()?.into_regular_file()?;
    let size = file.get_boxed_info::<FileInfo>().ok()?.file_size();
    let want = size.min(max as u64) as usize;
    let mut buf = alloc::vec![0u8; want];
    let mut got = 0;
    while got < want {
        match file.read(&mut buf[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    buf.truncate(got);
    Some((buf, size))
}
