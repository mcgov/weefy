//! The memory map, and the console's modes.

use crate::tui::{self, BrowseState, Doc, Item, Outcome, Screen, st};
use crate::util;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use uefi::boot::{self, MemoryType, SearchType};
use uefi::mem::memory_map::{MemoryMap, MemoryMapMut};
use uefi::proto::console::gop::GraphicsOutput;
use uefi::Identify;

/// Name, what happens to it when the OS takes over, and what it is.
const TYPES: [(&str, &str, &str); 16] = [
    ("Reserved", "never usable", "Memory the firmware says nobody may use: devices, firmware regions, holes."),
    ("Loader code", "the OS decides", "The code of UEFI applications, including OS loaders. This program is in a range of this type."),
    ("Loader data", "the OS decides", "Data of UEFI applications and their default allocations. A loader hands the kernel, initrd and command line to the OS in memory of this type."),
    ("Boot svc code", "free after ExitBootServices", "The code of the firmware's boot service drivers: disk, USB, video, file system. Dead once the OS takes over."),
    ("Boot svc data", "free after ExitBootServices", "Data of the firmware's boot service drivers, and the pool that AllocatePool() draws on by default."),
    ("Runtime code", "kept and mapped by the OS", "The code of the runtime services. The OS maps it and calls into it for as long as it runs."),
    ("Runtime data", "kept and mapped by the OS", "Data of the runtime services, the system table and the configuration tables' own storage."),
    ("Conventional", "free", "Free memory. Everything the OS starts out with comes from here."),
    ("Unusable", "never usable", "Memory in which errors were detected."),
    ("ACPI reclaim", "free once ACPI is read", "The ACPI tables. The OS may reuse it after it has parsed them."),
    ("ACPI NVS", "kept, never touched", "Memory the firmware's ACPI code uses across sleep states. The OS must save and restore it but not use it."),
    ("MMIO", "mapped by the OS if runtime", "Device registers that the runtime services need mapped, such as the flash that holds variables."),
    ("MMIO port", "mapped by the OS if runtime", "I/O port space mapped into memory (Itanium)."),
    ("PAL code", "kept", "Processor firmware code (Itanium)."),
    ("Persistent", "usable, survives power-off", "Non-volatile memory (NVDIMM) usable like RAM."),
    ("Unaccepted", "must be accepted first", "Memory a confidential VM (TDX, SEV-SNP) has not yet accepted from its host."),
];

fn type_info(ty: MemoryType) -> (&'static str, &'static str, &'static str) {
    TYPES.get(ty.0 as usize).copied().unwrap_or(("OEM / OS type", "defined by its owner", "A type outside the specification's list: defined by the firmware vendor (0x70000000 up) or by an OS loader (0x80000000 up)."))
}

const ATTRS: [(u64, &str); 16] = [
    (0x1, "UC: can be mapped uncached"),
    (0x2, "WC: can be mapped write-combining"),
    (0x4, "WT: can be mapped write-through"),
    (0x8, "WB: can be mapped write-back (normal RAM caching)"),
    (0x10, "UCE: uncached, exported"),
    (0x1000, "WP: can be write-protected"),
    (0x2000, "RP: can be read-protected"),
    (0x4000, "XP: can be made non-executable"),
    (0x8000, "NV: non-volatile"),
    (0x10000, "more reliable than other memory"),
    (0x20000, "RO: can be made read-only"),
    (0x40000, "SP: special purpose, not for general use"),
    (0x80000, "protected by CPU cryptography"),
    (0x100000, "hot-pluggable"),
    (0x8000_0000_0000_0000, "RUNTIME: the OS must map this for the runtime services"),
    (0x4000_0000_0000_0000, "ISA-specific attributes are valid"),
];

pub fn run(scr: &mut Screen) {
    let Ok(mut map) = boot::memory_map(MemoryType::LOADER_DATA) else {
        return;
    };
    map.sort();
    let entries: Vec<_> = map.entries().copied().collect();
    let key = map.key();
    let desc_size = map.meta().desc_size;
    drop(map);

    let mut items = alloc::vec![Item::new("Summary"), Item::header("Start         Size      Type")];
    for d in &entries {
        let (name, _, _) = type_info(d.ty);
        let style = match d.ty {
            MemoryType::CONVENTIONAL => st::GOOD,
            MemoryType::RUNTIME_SERVICES_CODE | MemoryType::RUNTIME_SERVICES_DATA => st::HEAD,
            MemoryType::BOOT_SERVICES_CODE | MemoryType::BOOT_SERVICES_DATA => st::DIM,
            _ => st::NORMAL,
        };
        items.push(Item::new(format!("{:012x}  {:>8}  {name}", d.phys_start, util::size(d.page_count * 4096).replace(" bytes", " B"))).styled(style));
    }
    let mut state = BrowseState::default();
    loop {
        let out = tui::browse(scr, &mut state, "Memory map", "Memory map", &items, &[], &mut |row, width| {
            let mut doc = Doc::new(width);
            if row == 0 {
                doc.p("Every range of physical memory the firmware knows about, as GetMemoryMap() returned it when this screen opened. An OS builds its view of memory from exactly this list.");
                doc.blank();
                doc.kv("Entries", format!("{}", entries.len()));
                doc.kv("Entry size", format!("{desc_size} bytes"));
                doc.kv("Map key", format!("{key:?}").replace("MemoryMapKey", "").replace(&['(', ')'][..], ""));
                doc.dim("The key names this version of the map. ExitBootServices() succeeds only with the key of the current map; any allocation changes it.");
                doc.h("By type");
                let mut totals = [0u64; 17];
                for d in &entries {
                    totals[(d.ty.0 as usize).min(16)] += d.page_count * 4096;
                }
                let ram: u64 = totals.iter().enumerate().filter(|(i, _)| !matches!(i, 0 | 11 | 12 | 16)).map(|(_, t)| *t).sum();
                let bar_w = width.saturating_sub(28).clamp(4, 30);
                for (i, &t) in totals.iter().enumerate() {
                    if t == 0 {
                        continue;
                    }
                    let name = if i < 16 { TYPES[i].0 } else { "OEM / OS type" };
                    let filled = if ram == 0 { 0 } else { ((t as u128 * bar_w as u128).div_ceil(ram as u128) as usize).min(bar_w) };
                    let counts = !matches!(i, 0 | 11 | 12 | 16);
                    let mut bar = String::new();
                    for b in 0..bar_w {
                        bar.push(if counts && b < filled { tui::BLOCK } else { tui::SHADE });
                    }
                    doc.line(
                        tui::Line::of(st::KEY, format!("{name:<14}"))
                            .push(st::NORMAL, format!("{:>9} ", util::size(t).replace(" bytes", " B")))
                            .push(if i == 7 { st::GOOD } else { st::DIM }, bar),
                    );
                }
                doc.blank();
                doc.kv("RAM in the map", util::size(ram));
                doc.kv("Free now", util::size(totals[7]));
                doc.kv("Free for the OS", format!("{}  (conventional + loader + boot services)", util::size(totals[7] + totals[1] + totals[2] + totals[3] + totals[4])));
                doc.kv("Kept by firmware", format!("{}  (runtime + ACPI NVS)", util::size(totals[5] + totals[6] + totals[10])));
                return doc;
            }
            let d = &entries[row - 2];
            let (name, fate, about) = type_info(d.ty);
            doc.p(about);
            doc.blank();
            doc.kv("Type", format!("{name}  ({})", d.ty.0));
            doc.kv("For the OS", fate);
            doc.kv("Start", format!("{:#x}", d.phys_start));
            doc.kv("End", format!("{:#x}", d.phys_start + d.page_count * 4096 - 1));
            doc.kv("Pages", format!("{} of 4 KiB", d.page_count));
            doc.kv("Size", util::size(d.page_count * 4096));
            doc.kv("Attributes", format!("{:#x}", d.att.bits()));
            for (bit, text) in ATTRS {
                if d.att.bits() & bit != 0 {
                    doc.bullet(text);
                }
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

/// The console: text modes to switch between, and what the display is.
pub fn run_console(scr: &mut Screen) {
    let mut state = BrowseState::default();
    loop {
        let (modes, current) = tui::text_modes();
        let mut items = alloc::vec![Item::header("Text modes (Enter to switch)")];
        for m in &modes {
            let mark = if m.index == current { "  (current)" } else { "" };
            let item = Item::new(format!("{} x {}{mark}", m.cols, m.rows));
            items.push(if m.index == current { item.styled(st::GOOD) } else { item });
        }
        items.push(Item::header("Devices"));
        items.push(Item::new("Graphics output"));
        let gop_row = items.len() - 1;
        let out = tui::browse(scr, &mut state, "Console", "Console", &items, &[], &mut |row, width| {
            let mut doc = Doc::new(width);
            if row == gop_row {
                doc.p("The Graphics Output protocol (GOP) is the firmware's framebuffer. The text you are reading is drawn into it by the firmware's graphics console driver, one character cell at a time. An OS loader passes the framebuffer's address and layout to the kernel.");
                let handles = boot::locate_handle_buffer(SearchType::ByProtocol(&GraphicsOutput::GUID)).map(|h| h.to_vec()).unwrap_or_default();
                if handles.is_empty() {
                    doc.blank();
                    doc.styled(st::HEAD, "No handle carries GOP: this console is a serial port, or the firmware has no display driver.");
                }
                for (n, h) in handles.iter().enumerate() {
                    let Some(mut gop) = util::open::<GraphicsOutput>(*h) else {
                        continue;
                    };
                    doc.h(&format!("Display {}", n + 1));
                    if let Some(dp) = util::handle_dp(*h) {
                        doc.kv("Device path", util::dp_bytes_text(&dp));
                    }
                    let info = gop.current_mode_info();
                    let (w, hgt) = info.resolution();
                    doc.kv("Resolution", format!("{w} x {hgt}"));
                    doc.kv("Pixel format", format!("{:?}", info.pixel_format()));
                    doc.kv("Stride", format!("{} pixels per line", info.stride()));
                    if info.pixel_format() != uefi::proto::console::gop::PixelFormat::BltOnly {
                        let mut fb = gop.frame_buffer();
                        doc.kv("Framebuffer", format!("{:#x}, {}", fb.as_mut_ptr() as usize, util::size(fb.size() as u64)));
                    }
                    let list: Vec<String> = gop.modes().map(|m| {
                        let (w, h) = m.info().resolution();
                        format!("{w}x{h}")
                    }).collect();
                    doc.kv("Modes", list.join(", "));
                }
                return doc;
            }
            let Some(m) = modes.get(row.wrapping_sub(1)) else {
                return doc;
            };
            doc.p("A text mode of the Simple Text Output protocol: a grid of character cells. Mode 0 is always 80 x 25. On a graphics console the others depend on the resolution and the font; when the console is several devices at once (a display and a serial port), only the modes they all share are offered.");
            doc.blank();
            doc.kv("Mode number", format!("{}", m.index));
            doc.kv("Columns", format!("{}", m.cols));
            doc.kv("Rows", format!("{}", m.rows));
            doc.kv("In use", if m.index == current { "yes" } else { "no: press Enter to switch to it" });
            if m.cols < 80 || m.rows < 24 {
                doc.styled(st::HEAD, "Smaller than this program's screens are laid out for.");
            }
            doc
        });
        match out {
            Outcome::Back => return,
            Outcome::Open(row) => {
                if let Some(m) = modes.get(row.wrapping_sub(1)) {
                    if m.index != current {
                        scr.set_mode(m.index);
                    }
                } else {
                    state.zoom = true;
                }
            }
            Outcome::Char(_) => {}
        }
    }
}
