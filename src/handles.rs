//! The handle database: every handle with its protocols, and every protocol
//! with its handles.

use crate::bootopt;
use crate::guids;
use crate::tui::{self, BrowseState, Doc, Item, Outcome, Screen, st};
use crate::util;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use uefi::boot::{self, SearchType};
use uefi::proto::console::gop::GraphicsOutput;
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::{Guid, Handle, Identify, guid};

const DRIVER_BINDING: Guid = guid!("18a031ab-b443-4d1a-a5c0-0c09261e9f71");
const FV2: Guid = guid!("220e73b6-6bdb-4413-8405-b974b108619a");

/// A loaded image: where it is in memory and what it is called.
pub struct Image {
    pub base: usize,
    pub size: usize,
    pub name: String,
}

/// The name a linker left in a PE/COFF image: the file name in its debug
/// directory's CodeView record. edk2 builds keep it, which is the only place
/// a driver inside the firmware image carries its own name.
fn pe_debug_name(base: usize, size: usize) -> Option<String> {
    if base == 0 || size < 0x200 {
        return None;
    }
    // SAFETY: Loaded Image says the image occupies these bytes.
    let img = unsafe { core::slice::from_raw_parts(base as *const u8, size) };
    if img.get(..2)? != b"MZ" {
        return None;
    }
    let pe = util::rd32(img, 0x3c)? as usize;
    if img.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let opt = pe + 24;
    // The data directories follow the optional header's fixed part, whose
    // size depends on whether the image is 32-bit or 64-bit.
    let dirs = match util::rd16(img, opt)? {
        0x20b => opt + 112,
        0x10b => opt + 96,
        _ => return None,
    };
    if util::rd32(img, dirs - 4)? < 7 {
        return None;
    }
    let debug_rva = util::rd32(img, dirs + 6 * 8)? as usize;
    let debug_size = util::rd32(img, dirs + 6 * 8 + 4)? as usize;
    for entry in (debug_rva..debug_rva.checked_add(debug_size)?).step_by(28) {
        // Type 2 is CodeView.
        if util::rd32(img, entry + 12)? != 2 {
            continue;
        }
        let at = util::rd32(img, entry + 20)? as usize;
        let len = util::rd32(img, entry + 16)? as usize;
        let rec = img.get(at..at.checked_add(len)?)?;
        let path = match rec.get(..4)? {
            b"RSDS" => rec.get(24..)?,
            b"NB10" => rec.get(16..)?,
            b"MTOC" => rec.get(20..)?,
            _ => continue,
        };
        let path = &path[..path.iter().position(|&b| b == 0).unwrap_or(path.len())];
        let path: String = path.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '?' }).collect();
        let file = path.rsplit(['/', '\\']).next()?;
        let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
        if !stem.is_empty() {
            return Some(String::from(stem));
        }
    }
    None
}

fn image_name(image: &LoadedImage) -> String {
    let (base, size) = image.info();
    let from_path = image.file_path().map(|path| {
        let bytes = path.as_bytes();
        (util::dp_file_name(bytes), util::dp_bytes_text(bytes))
    });
    match from_path {
        // A file on a disk: its path says what it is.
        Some((Some(file), _)) => file,
        // A file in the firmware image is named only by a GUID; the name
        // its linker recorded is more use.
        Some((None, text)) => pe_debug_name(base as usize, size as usize).unwrap_or(text),
        None => pe_debug_name(base as usize, size as usize).unwrap_or_else(|| String::from("(no file path)")),
    }
}

/// Every image in memory, for telling which one an address belongs to.
pub fn images() -> Vec<Image> {
    let Ok(hs) = boot::locate_handle_buffer(SearchType::ByProtocol(&LoadedImage::GUID)) else {
        return Vec::new();
    };
    hs.iter()
        .filter_map(|&h| {
            let image = util::open::<LoadedImage>(h)?;
            let (base, size) = image.info();
            Some(Image {
                base: base as usize,
                size: size as usize,
                name: image_name(&image),
            })
        })
        .collect()
}

fn protocols(handle: Handle) -> Vec<Guid> {
    boot::protocols_per_handle(handle)
        .map(|p| p.iter().map(|g| **g).collect())
        .unwrap_or_default()
}

/// One line that says what a handle is.
fn summary(handle: Handle, protos: &[Guid]) -> String {
    let has = |g: &Guid| protos.contains(g);
    if has(&LoadedImage::GUID) {
        if let Some(image) = util::open::<LoadedImage>(handle) {
            let name = image_name(&image);
            let short = name.rsplit('\\').next().unwrap_or(&name);
            return if handle == boot::image_handle() {
                format!("Image: {short} (this program)")
            } else {
                format!("Image: {short}")
            };
        }
    }
    if has(&SimpleFileSystem::GUID) {
        return match bootopt::volume_label(handle) {
            Some(label) => format!("File system: {label}"),
            None => String::from("File system"),
        };
    }
    if has(&BlockIO::GUID) {
        if let Some(bio) = util::open::<BlockIO>(handle) {
            let m = bio.media();
            let size = (m.last_block() + 1) * m.block_size() as u64;
            let kind = if m.is_logical_partition() { "Partition" } else { "Disk" };
            return format!("{kind}, {}", util::size(size));
        }
    }
    if has(&GraphicsOutput::GUID) {
        return String::from("Graphics output");
    }
    if has(&DRIVER_BINDING) {
        return String::from("Driver");
    }
    if has(&FV2) {
        return String::from("Firmware volume");
    }
    if has(&DevicePath::GUID) {
        if let Some(dp) = util::handle_dp(handle) {
            let text = util::dp_bytes_text(&dp);
            // The last node says the most about the device.
            let last = text.rsplit('/').next().unwrap_or(&text);
            return String::from(last);
        }
    }
    match protos.first() {
        Some(g) => guids::short(g),
        None => String::from("(no protocols)"),
    }
}

struct Entry {
    handle: Handle,
    protos: Vec<Guid>,
    summary: String,
}

fn collect() -> Vec<Entry> {
    let Ok(all) = boot::locate_handle_buffer(SearchType::AllHandles) else {
        return Vec::new();
    };
    all.iter()
        .map(|&handle| {
            let protos = protocols(handle);
            let summary = summary(handle, &protos);
            Entry {
                handle,
                protos,
                summary,
            }
        })
        .collect()
}

/// Facts read through a protocol's own interface, for the ones this program
/// knows how to open.
fn decode(doc: &mut Doc, handle: Handle, g: &Guid) {
    if *g == LoadedImage::GUID {
        if let Some(image) = util::open::<LoadedImage>(handle) {
            let (base, size) = image.info();
            doc.kv("  Image base", format!("{:#x}", base as usize));
            doc.kv("  Image size", util::size(size));
            if let Some(path) = image.file_path() {
                doc.kv("  File path", util::dp_text(path));
            }
            if let Some(dev) = image.device().and_then(util::handle_dp) {
                doc.kv("  Loaded from", util::dp_bytes_text(&dev));
            }
            let kind = match image.code_type() {
                boot::MemoryType::LOADER_CODE => "application (loader code)",
                boot::MemoryType::BOOT_SERVICES_CODE => "boot service driver",
                boot::MemoryType::RUNTIME_SERVICES_CODE => "runtime driver: stays for the OS",
                _ => "other",
            };
            doc.kv("  Kind", kind);
            if let Some(opts) = image.load_options_as_bytes() {
                doc.kv("  Load options", format!("{} bytes", opts.len()));
            }
        }
    } else if *g == BlockIO::GUID {
        if let Some(bio) = util::open::<BlockIO>(handle) {
            let m = bio.media();
            let blocks = m.last_block() + 1;
            doc.kv("  Block size", format!("{} bytes", m.block_size()));
            doc.kv("  Blocks", format!("{blocks}"));
            doc.kv("  Capacity", util::size(blocks * m.block_size() as u64));
            let mut flags = Vec::new();
            flags.push(if m.is_logical_partition() { "a partition" } else { "a whole device" });
            if m.is_removable_media() {
                flags.push("removable");
            }
            if !m.is_media_present() {
                flags.push("no media");
            }
            if m.is_read_only() {
                flags.push("read-only");
            }
            doc.kv("  Media", flags.join(", "));
        }
    } else if *g == SimpleFileSystem::GUID {
        if let Some(label) = bootopt::volume_label(handle) {
            doc.kv("  Volume label", label);
        }
    } else if *g == GraphicsOutput::GUID {
        if let Some(gop) = util::open::<GraphicsOutput>(handle) {
            let info = gop.current_mode_info();
            let (w, h) = info.resolution();
            doc.kv("  Current mode", format!("{w} x {h}, {:?}", info.pixel_format()));
            doc.kv("  Modes", format!("{}", gop.modes().count()));
        }
    }
}

fn handle_page(doc: &mut Doc, index: usize, e: &Entry) {
    doc.kv("Handle", format!("#{index}, at {:#x}", e.handle.as_ptr() as usize));
    if let Some(dp) = util::handle_dp(e.handle) {
        doc.kv("Device path", util::dp_bytes_text(&dp));
    }
    doc.kv("Protocols", format!("{}", e.protos.len()));
    for g in &e.protos {
        doc.blank();
        doc.line(tui::Line::of(st::HEAD, guids::short(g)));
        doc.dim(&format!("{g}"));
        decode(doc, e.handle, g);
        match guids::explain(g) {
            Some(text) => doc.p(text),
            None if guids::lookup(g).is_none() => doc.dim("Not declared in edk2's public packages: private to this firmware."),
            None => {}
        }
    }
}

pub fn run_handles(scr: &mut Screen) {
    let entries = collect();
    let items: Vec<Item> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let item = Item::new(format!("{i:>3} {}", e.summary));
            if e.handle == boot::image_handle() { item.styled(st::GOOD) } else { item }
        })
        .collect();
    let mut state = BrowseState::default();
    loop {
        let out = tui::browse(scr, &mut state, "Handles", "Handles", &items, &[], &mut |i, width| {
            let mut doc = Doc::new(width);
            handle_page(&mut doc, i, &entries[i]);
            doc
        });
        match out {
            Outcome::Back => return,
            Outcome::Open(_) => state.zoom = true,
            Outcome::Char(_) => {}
        }
    }
}

pub fn run_protocols(scr: &mut Screen) {
    let entries = collect();
    // Every distinct protocol, with the handles that carry it.
    let mut protos: Vec<(Guid, Vec<usize>)> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        for g in &e.protos {
            match protos.iter_mut().find(|p| p.0 == *g) {
                Some(p) => p.1.push(i),
                None => protos.push((*g, alloc::vec![i])),
            }
        }
    }
    // Named ones first, by name; then the anonymous GUIDs.
    protos.sort_by_cached_key(|p| (guids::lookup(&p.0).is_none(), guids::short(&p.0).to_lowercase()));
    let items: Vec<Item> = protos
        .iter()
        .map(|(g, hs)| {
            let item = Item::new(format!("{:>3} {}", hs.len(), guids::short(g)));
            if guids::lookup(g).is_none() { item.styled(st::DIM) } else { item }
        })
        .collect();
    let mut state = BrowseState::default();
    loop {
        let out = tui::browse(scr, &mut state, "Protocols", "Count, protocol", &items, &[], &mut |i, width| {
            let mut doc = Doc::new(width);
            let (g, hs) = &protos[i];
            match guids::explain(g) {
                Some(text) => doc.p(text),
                None if guids::lookup(g).is_none() => doc.dim("A GUID that edk2's public packages don't declare: a protocol private to this firmware."),
                None => doc.dim("A protocol this program has no explanation for. Its name says what it is for."),
            }
            doc.blank();
            if let Some((name, pkg)) = guids::lookup(g) {
                doc.kv("Name", format!("g{name}Guid"));
                doc.kv("Declared in", format!("edk2 {pkg}"));
            }
            doc.kv("GUID", format!("{g}"));
            doc.h(&format!("On {} handle{}", hs.len(), if hs.len() == 1 { "" } else { "s" }));
            for &h in hs.iter().take(200) {
                doc.line(tui::Line::of(st::KEY, format!("{h:>4} ")).push(st::NORMAL, entries[h].summary.clone()));
            }
            doc
        });
        match out {
            Outcome::Back => return,
            Outcome::Open(_) => state.zoom = true,
            Outcome::Char(_) => {}
        }
    }
}
