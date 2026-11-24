//! The system table and its configuration tables, decoded.

use crate::guids;
use crate::tui::{self, BrowseState, Doc, Item, Outcome, Screen, st};
use crate::util;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use uefi::table::cfg::ConfigTableEntry;
use uefi::{Guid, boot, guid, system, table};
use uefi_raw::table::Header;

const FDT_GUID: Guid = guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0");
pub const RT_PROPERTIES_GUID: Guid = guid!("eb66918a-7eef-402a-842e-931d21c38ae9");
const CONFORMANCE_GUID: Guid = guid!("36122546-f7e7-4c8f-bd9b-eb8525b50c0b");

/// Bytes at a physical address. Firmware memory is identity-mapped and
/// readable while boot services are active.
///
/// # Safety
/// `addr..addr+len` must be memory the firmware has mapped.
pub unsafe fn peek(addr: usize, len: usize) -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(addr as *const u8, len) }
}

fn ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
        .collect::<String>()
        .trim_end()
        .into()
}

/// A table header's fields, and whether its CRC32 checks out.
pub fn header(doc: &mut Doc, h: &Header, what: &str) {
    let sig = h.signature.to_le_bytes();
    doc.kv("Signature", format!("\"{}\"  ({:#018x})", ascii(&sig), h.signature));
    doc.kv("Revision", format!("{}  ({what} this table conforms to)", h.revision));
    doc.kv("Size", format!("{} bytes", h.size));
    // The CRC covers the whole table with the CRC field itself as zero.
    let size = h.size as usize;
    let verdict = if (24..=4096).contains(&size) {
        // SAFETY: the header says the table is this long.
        let mut copy = unsafe { peek(h as *const Header as usize, size) }.to_vec();
        copy[16..20].fill(0);
        match boot::calculate_crc32(&copy) {
            Ok(crc) if crc == h.crc => (st::GOOD, format!("{:#010x}  matches CalculateCrc32() of the table", h.crc)),
            Ok(crc) => (st::BAD, format!("{:#010x}  but the table's CRC32 is {crc:#010x}", h.crc)),
            Err(_) => (st::NORMAL, format!("{:#010x}", h.crc)),
        }
    } else {
        (st::NORMAL, format!("{:#010x}", h.crc))
    };
    doc.kv_styled("CRC32", verdict.0, &verdict.1);
}

pub fn run(scr: &mut Screen) {
    let tables: Vec<(Guid, *const c_void)> =
        system::with_config_table(|t| t.iter().map(|e: &ConfigTableEntry| (e.guid, e.address)).collect());
    let mut items = alloc::vec![
        Item::header("EFI_SYSTEM_TABLE"),
        Item::new("Header"),
        Item::new("Firmware"),
        Item::new("Console"),
        Item::new("Service tables"),
        Item::header("Configuration tables"),
    ];
    for (g, _) in &tables {
        items.push(Item::new(guids::short(g)));
    }
    let mut state = BrowseState::default();
    loop {
        let out = tui::browse(scr, &mut state, "System table", "System table", &items, &[], &mut |i, width| {
            let mut doc = Doc::new(width);
            // SAFETY: the system table pointer is valid while boot services run.
            let st_raw = unsafe { table::system_table_raw().unwrap().as_ref() };
            match i {
                1 => {
                    doc.p("Every UEFI table begins with this header. The system table is the root: the firmware passes its address to each image it starts.");
                    doc.blank();
                    doc.kv("Address", format!("{:#x}", st_raw as *const _ as usize));
                    header(&mut doc, &st_raw.header, "the UEFI specification version");
                }
                2 => {
                    doc.p("Who made this firmware. The vendor string and revision are the firmware maker's own; the UEFI version is the specification it implements.");
                    doc.blank();
                    doc.kv("Vendor", String::from(system::firmware_vendor()));
                    doc.kv("Revision", format!("{:#010x}", system::firmware_revision()));
                    doc.kv("UEFI version", format!("{}", system::uefi_revision()));
                }
                3 => {
                    doc.p("The three console devices. Each is a handle and a protocol pointer: Simple Text Input for the keyboard, Simple Text Output for the screen and for errors. They are usually virtual devices that fan out to every real console: a display, a serial port, a USB keyboard.");
                    doc.blank();
                    doc.kv("ConIn handle", format!("{:#x}", st_raw.stdin_handle as usize));
                    doc.kv("ConIn", format!("{:#x}", st_raw.stdin as usize));
                    doc.kv("ConOut handle", format!("{:#x}", st_raw.stdout_handle as usize));
                    doc.kv("ConOut", format!("{:#x}", st_raw.stdout as usize));
                    doc.kv("StdErr handle", format!("{:#x}", st_raw.stderr_handle as usize));
                    doc.kv("StdErr", format!("{:#x}", st_raw.stderr as usize));
                    doc.blank();
                    doc.dim("The Console screen lists the text modes ConOut offers.");
                }
                4 => {
                    doc.p("Pointers to the two tables of functions. Each has the same kind of header as the system table.");
                    doc.h("Boot services");
                    doc.kv("Address", format!("{:#x}", st_raw.boot_services as usize));
                    // SAFETY: valid while boot services run.
                    header(&mut doc, unsafe { &(*st_raw.boot_services).header }, "the UEFI version");
                    doc.h("Runtime services");
                    doc.kv("Address", format!("{:#x}", st_raw.runtime_services as usize));
                    // SAFETY: valid for the life of the system.
                    header(&mut doc, unsafe { &(*st_raw.runtime_services).header }, "the UEFI version");
                    doc.blank();
                    doc.dim("The Boot services and Runtime services screens list the functions in each.");
                }
                _ => {
                    let (g, addr) = tables[i - 6];
                    config_table(&mut doc, &g, addr as usize);
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

fn config_table(doc: &mut Doc, g: &Guid, addr: usize) {
    match guids::explain(g) {
        Some(text) => doc.p(text),
        None if guids::lookup(g).is_some() => doc.dim("A table this program has no explanation for."),
        None => doc.dim("A GUID that edk2's public packages don't declare: specific to this firmware's vendor."),
    }
    doc.blank();
    if let Some((name, pkg)) = guids::lookup(g) {
        doc.kv("Name", format!("g{name}Guid ({pkg})"));
    }
    doc.kv("GUID", format!("{g}"));
    doc.kv("Address", format!("{addr:#x}"));
    if addr == 0 {
        return;
    }
    // SAFETY (all arms): a configuration table pointer is to firmware memory
    // laid out as its GUID says; each decoder checks signatures and lengths.
    unsafe {
        if *g == ConfigTableEntry::ACPI2_GUID || *g == ConfigTableEntry::ACPI_GUID {
            acpi(doc, addr);
        } else if *g == ConfigTableEntry::SMBIOS3_GUID {
            let e = peek(addr, 24);
            if &e[..5] == b"_SM3_" {
                doc.kv("SMBIOS version", format!("{}.{}.{}", e[7], e[8], e[9]));
                let max = util::rd32(e, 12).unwrap_or(0) as usize;
                let at = util::rd64(e, 16).unwrap_or(0) as usize;
                doc.kv("Structures at", format!("{at:#x}, at most {max} bytes"));
                smbios(doc, at, max);
            }
        } else if *g == ConfigTableEntry::SMBIOS_GUID {
            let e = peek(addr, 31);
            if &e[..4] == b"_SM_" {
                doc.kv("SMBIOS version", format!("{}.{}", e[6], e[7]));
                let len = util::rd16(e, 0x16).unwrap_or(0) as usize;
                let at = util::rd32(e, 0x18).unwrap_or(0) as usize;
                doc.kv("Structures at", format!("{at:#x}, {len} bytes"));
                smbios(doc, at, len);
            }
        } else if *g == FDT_GUID {
            fdt(doc, addr);
        } else if *g == RT_PROPERTIES_GUID {
            let t = peek(addr, 8);
            let mask = util::rd32(t, 4).unwrap_or(0);
            doc.h("Runtime services that work after ExitBootServices()");
            for (bit, name) in RT_SERVICES.iter().enumerate() {
                let on = mask & (1 << bit) != 0;
                doc.kv_styled(name, if on { st::GOOD } else { st::BAD }, if on { "supported" } else { "not supported" });
            }
        } else if *g == ConfigTableEntry::MEMORY_ATTRIBUTES_GUID {
            let t = peek(addr, 16);
            doc.kv("Version", format!("{}", util::rd32(t, 0).unwrap_or(0)));
            doc.kv("Entries", format!("{}", util::rd32(t, 4).unwrap_or(0)));
            doc.kv("Entry size", format!("{} bytes", util::rd32(t, 8).unwrap_or(0)));
        } else if *g == ConfigTableEntry::ESRT_GUID {
            let t = peek(addr, 16);
            let count = (util::rd32(t, 0).unwrap_or(0) as usize).min(32);
            doc.kv("Components", format!("{count}"));
            for n in 0..count {
                let e = peek(addr + 16 + n * 40, 40);
                let class = Guid::from_bytes(e[..16].try_into().unwrap());
                let kind = match util::rd32(e, 16) {
                    Some(1) => "system firmware",
                    Some(2) => "device firmware",
                    Some(3) => "UEFI driver",
                    _ => "unknown type",
                };
                doc.h(&format!("{class}"));
                doc.kv("Type", kind);
                doc.kv("Version", format!("{:#x}", util::rd32(e, 20).unwrap_or(0)));
                doc.kv("Lowest allowed", format!("{:#x}", util::rd32(e, 24).unwrap_or(0)));
            }
            doc.blank();
            doc.dim("The Capsules screen shows each component in full, beside what the Firmware Management Protocol says about it.");
        } else if *g == crate::capsule::LAB_CAPSULE {
            crate::capsule::landed_page(doc);
        } else if *g == CONFORMANCE_GUID {
            let t = peek(addr, 4);
            let count = (util::rd16(t, 2).unwrap_or(0) as usize).min(16);
            doc.h("Profiles");
            for n in 0..count {
                let p = Guid::from_bytes(peek(addr + 4 + n * 16, 16).try_into().unwrap());
                let name = guids::explain(&p).unwrap_or("A profile this program does not know.");
                doc.h(&guids::short(&p));
                doc.p(name);
                doc.kv("GUID", format!("{p}"));
            }
        }
        doc.h("First bytes at that address");
        doc.hexdump(peek(addr, 64), 64);
    }
}

pub const RT_SERVICES: [&str; 14] = [
    "GetTime",
    "SetTime",
    "GetWakeupTime",
    "SetWakeupTime",
    "GetVariable",
    "GetNextVariableName",
    "SetVariable",
    "SetVirtualAddressMap",
    "ConvertPointer",
    "GetNextHighMonotonicCount",
    "ResetSystem",
    "UpdateCapsule",
    "QueryCapsuleCapabilities",
    "QueryVariableInfo",
];

/// The mask from the RT properties table: which runtime services work after
/// ExitBootServices(). None if the firmware publishes no such table (then
/// all of them are supposed to).
pub fn rt_properties() -> Option<u32> {
    let addr = system::with_config_table(|t| t.iter().find(|e| e.guid == RT_PROPERTIES_GUID).map(|e| e.address as usize))?;
    // SAFETY: the table is 8 bytes: version, length, mask.
    util::rd32(unsafe { peek(addr, 8) }, 4)
}

fn acpi_about(sig: &str) -> &'static str {
    match sig {
        "FACP" => "fixed description: power management registers, and where the DSDT is",
        "DSDT" => "the main block of AML code describing devices",
        "SSDT" => "more AML code, added to the DSDT",
        "APIC" => "interrupt controllers and CPU cores (MADT)",
        "MCFG" => "where PCI Express configuration space is mapped",
        "GTDT" => "Arm generic timers",
        "SPCR" => "which serial port is the console",
        "DBG2" => "debug ports",
        "HPET" => "high precision event timer",
        "BGRT" => "the boot logo, so the OS can keep it on screen",
        "PPTT" => "CPU topology and caches",
        "IORT" => "Arm interrupt and IOMMU routing",
        "TPM2" => "the TPM 2.0 device",
        "FPDT" => "firmware boot performance timings",
        "WSMT" => "Windows SMM security mitigations",
        "SRAT" => "NUMA: which memory belongs to which node",
        "SLIT" => "NUMA distances",
        "DMAR" => "Intel IOMMU (VT-d)",
        "IVRS" => "AMD IOMMU",
        "WAET" => "Windows hints for virtual machines",
        "PCCT" => "platform communication channels",
        "BERT" | "HEST" | "ERST" | "EINJ" => "hardware error reporting (APEI)",
        "FACS" => "firmware control structure: the waking vector",
        "ECDT" => "the embedded controller",
        "MSDM" => "a Windows licence key",
        "LPIT" => "low power idle states",
        "WDAT" | "WDRT" => "watchdog timer",
        "SDEI" => "Arm software delegated exceptions",
        "RHCT" => "RISC-V hart capabilities",
        _ => "",
    }
}

/// # Safety
/// `addr` must be the ACPI root pointer from the configuration table.
unsafe fn acpi(doc: &mut Doc, addr: usize) {
    unsafe {
        let rsdp = peek(addr, 36);
        if &rsdp[..8] != b"RSD PTR " {
            doc.styled(st::BAD, "No \"RSD PTR \" signature here.");
            return;
        }
        doc.h("Root System Description Pointer");
        doc.kv("OEM ID", ascii(&rsdp[9..15]));
        doc.kv("Revision", format!("{} ({})", rsdp[15], if rsdp[15] >= 2 { "ACPI 2.0 or later" } else { "ACPI 1.0" }));
        let (root, width, name) = if rsdp[15] >= 2 && util::rd64(rsdp, 24).unwrap_or(0) != 0 {
            (util::rd64(rsdp, 24).unwrap() as usize, 8, "XSDT")
        } else {
            (util::rd32(rsdp, 16).unwrap_or(0) as usize, 4, "RSDT")
        };
        doc.kv(name, format!("{root:#x}"));
        if root == 0 {
            return;
        }
        let head = peek(root, 36);
        let len = util::rd32(head, 4).unwrap_or(0) as usize;
        if &head[..4] != name.as_bytes() || !(36..=36 + 8 * 256).contains(&len) {
            return;
        }
        let body = peek(root, len);
        doc.h(&format!("Tables the {name} lists"));
        let mut dsdt = 0usize;
        for chunk in body[36..].chunks_exact(width) {
            let at = if width == 8 {
                u64::from_le_bytes(chunk.try_into().unwrap()) as usize
            } else {
                u32::from_le_bytes(chunk.try_into().unwrap()) as usize
            };
            if at == 0 {
                continue;
            }
            let t = peek(at, 36);
            let sig = ascii(&t[..4]);
            if sig == "FACP" {
                let tlen = util::rd32(t, 4).unwrap_or(0) as usize;
                let fadt = peek(at, tlen.clamp(36, 276));
                dsdt = util::rd64(fadt, 140).filter(|&x| x != 0).map(|x| x as usize).or(util::rd32(fadt, 40).map(|x| x as usize)).unwrap_or(0);
            }
            acpi_line(doc, &sig, t, at);
        }
        if dsdt != 0 {
            let t = peek(dsdt, 36);
            if &t[..4] == b"DSDT" {
                acpi_line(doc, "DSDT", t, dsdt);
            }
        }
    }
}

fn acpi_line(doc: &mut Doc, sig: &str, head: &[u8], at: usize) {
    doc.line(
        tui::Line::of(st::KEY, format!("{sig:<5}"))
            .push(st::NORMAL, format!("{:<9}", ascii(&head[16..24])))
            .push(st::DIM, format!("{at:#x}, {} bytes", util::rd32(head, 4).unwrap_or(0))),
    );
    let about = acpi_about(sig);
    if !about.is_empty() {
        doc.dim(&format!("     {about}"));
    }
}

/// # Safety
/// `at..at+max` must be the SMBIOS structure table.
unsafe fn smbios(doc: &mut Doc, at: usize, max: usize) {
    if at == 0 || max < 4 {
        return;
    }
    let data = unsafe { peek(at, max.min(64 * 1024)) };
    let mut pos = 0;
    let mut counts = [0usize; 128];
    let mut shown = Vec::new();
    while pos + 4 <= data.len() {
        let ty = data[pos];
        let len = data[pos + 1] as usize;
        if len < 4 || pos + len > data.len() {
            break;
        }
        let formatted = &data[pos..pos + len];
        // The strings follow the formatted part, each NUL-terminated, the
        // set ended by an empty one.
        let mut strings: Vec<&[u8]> = Vec::new();
        let mut s = pos + len;
        loop {
            let end = match data[s.min(data.len())..].iter().position(|&b| b == 0) {
                Some(e) => s + e,
                None => data.len(),
            };
            if end == s {
                break;
            }
            strings.push(&data[s..end]);
            s = end + 1;
            if s >= data.len() {
                break;
            }
        }
        let next = if strings.is_empty() { s + 2 } else { s + 1 };
        let string = |index_at: usize| -> String {
            formatted
                .get(index_at)
                .and_then(|&n| strings.get((n as usize).wrapping_sub(1)))
                .map(|b| ascii(b))
                .unwrap_or_default()
        };
        if (ty as usize) < counts.len() {
            counts[ty as usize] += 1;
        }
        match ty {
            0 => {
                shown.push(("Firmware vendor", string(4)));
                shown.push(("Firmware version", string(5)));
                shown.push(("Firmware date", string(8)));
            }
            1 => {
                shown.push(("Manufacturer", string(4)));
                shown.push(("Product", string(5)));
                shown.push(("Version", string(6)));
            }
            2 if counts[2] == 1 => {
                shown.push(("Board maker", string(4)));
                shown.push(("Board", string(5)));
            }
            4 if counts[4] == 1 => shown.push(("Processor", string(0x10))),
            _ => {}
        }
        if ty == 127 {
            break;
        }
        pos = next;
    }
    doc.h("From the SMBIOS structures");
    for (k, v) in shown {
        if !v.is_empty() {
            doc.kv(k, v);
        }
    }
    doc.kv("Processors", format!("{} (type 4 structures)", counts[4]));
    doc.kv("Memory slots", format!("{} (type 17 structures)", counts[17]));
    doc.kv("Structures", format!("{}", counts.iter().sum::<usize>()));
}

/// # Safety
/// `addr` must be the device tree blob from the configuration table.
unsafe fn fdt(doc: &mut Doc, addr: usize) {
    let be32 = |b: &[u8], at: usize| b.get(at..at + 4).map(|x| u32::from_be_bytes(x.try_into().unwrap()));
    let head = unsafe { peek(addr, 40) };
    if be32(head, 0) != Some(0xd00d_feed) {
        doc.styled(st::BAD, "No device tree magic (d00dfeed) here.");
        return;
    }
    let total = be32(head, 4).unwrap_or(0) as usize;
    doc.kv("Size", util::size(total as u64));
    doc.kv("Version", format!("{}", be32(head, 20).unwrap_or(0)));
    if !(40..=4 << 20).contains(&total) {
        return;
    }
    let blob = unsafe { peek(addr, total) };
    let structs = be32(head, 8).unwrap_or(0) as usize;
    let strings = be32(head, 12).unwrap_or(0) as usize;
    let name_at = |off: usize| -> &[u8] {
        let s = blob.get(strings + off..).unwrap_or(&[]);
        &s[..s.iter().position(|&b| b == 0).unwrap_or(0)]
    };
    // Walk the structure block for the root node's own properties and count
    // the nodes.
    let (mut pos, mut depth, mut nodes) = (structs, 0usize, 0usize);
    doc.h("The root node");
    while let Some(token) = be32(blob, pos) {
        pos += 4;
        match token {
            1 => {
                let n = blob[pos.min(blob.len())..].iter().position(|&b| b == 0).unwrap_or(0);
                pos = (pos + n + 4) & !3;
                depth += 1;
                nodes += 1;
            }
            2 => depth = depth.saturating_sub(1),
            3 => {
                let len = be32(blob, pos).unwrap_or(0) as usize;
                let name = name_at(be32(blob, pos + 4).unwrap_or(0) as usize);
                let value = blob.get(pos + 8..pos + 8 + len).unwrap_or(&[]);
                if depth == 1 && (name == b"model" || name == b"compatible" || name == b"serial-number") {
                    let text: Vec<String> = value.split(|&b| b == 0).filter(|s| !s.is_empty()).map(ascii).collect();
                    if name != b"serial-number" {
                        doc.kv(&ascii(name), text.join(", "));
                    }
                }
                pos = (pos + 8 + len + 3) & !3;
            }
            4 => {}
            _ => break,
        }
    }
    doc.kv("Nodes", format!("{nodes}"));
}
