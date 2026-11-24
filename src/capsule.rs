//! Capsules and firmware updates: what this firmware says it can update and
//! how (the capsule services, the ESRT, the Firmware Management Protocol and
//! the result variables), a reader for capsule files found on the disk, and
//! one harmless capsule to send and follow across a reset.
//!
//! Everything here only reads, except "Send a test capsule", which asks
//! first. No capsule file is ever submitted.

use crate::bootopt;
use crate::guids;
use crate::systab;
use crate::tui::{self, BrowseState, Doc, Item, Key, Outcome, Screen, st};
use crate::util;
use crate::vars;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use uefi::boot::{self, AllocateType, MemoryType, SearchType};
use uefi::proto::media::file::{File, FileAttribute, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::proto::unsafe_protocol;
use uefi::runtime::{self, CapsuleBlockDescriptor, CapsuleFlags, CapsuleHeader, CapsuleInfo, ResetType, VariableVendor};
use uefi::table::cfg::ConfigTableEntry;
use uefi::{Guid, Handle, Identify, Status, guid, system};
use uefi_raw::protocol::firmware_management::FirmwareManagementProtocol;

const FMP_CAPSULE: Guid = guid!("6dcbd5ed-e82d-4c44-bda1-7194199ad92a");
const UX_CAPSULE: Guid = guid!("3b8c8162-188c-46a4-aec9-be43f1d65697");
const JSON_CAPSULE: Guid = guid!("67d6f4cd-d6b8-4573-bf4a-de5e252d61ae");
const CAPSULE_ARCH: Guid = guid!("5053697e-2cbc-4819-90d9-0580deee5754");
const CAPSULE_VENDOR: Guid = guid!("711c703f-c285-4b10-a3b0-36ecbd3c8be2");
const PKCS7_CERT: Guid = guid!("4aafd29d-68df-49ee-8aa9-347d375665a7");
/// The namespace of the capsule result variables.
pub const CAPSULE_REPORT: Guid = guid!("39b68c46-f7fb-441b-b6ec-16b0f69821f3");
/// The GUID of the test capsule: one made up for this program, so that no
/// firmware has anything to do with the capsule but carry it.
pub const LAB_CAPSULE: Guid = guid!("6da511a3-64c4-443d-adc5-69780db26f9c");

/// sizeof(EFI_CAPSULE_HEADER).
const HEADER: usize = 28;
const PERSIST: u32 = 0x0001_0000;
const POPULATE: u32 = 0x0002_0000;
const INITIATE_RESET: u32 = 0x0004_0000;
/// Where an OS leaves capsule files for the firmware.
const ON_DISK_DIR: &str = "\\EFI\\UpdateCapsule";
/// The most of a capsule file this program reads.
const MAX_READ: usize = 64 << 20;

fn guid_at(b: &[u8], at: usize) -> Option<Guid> {
    Some(Guid::from_bytes(b.get(at..at + 16)?.try_into().ok()?))
}

fn config_table(guid: &Guid) -> Option<usize> {
    system::with_config_table(|t| t.iter().find(|e| e.guid == *guid).map(|e| e.address as usize)).filter(|&a| a != 0)
}

fn version(v: u32) -> String {
    format!("{v:#010x}  ({v})")
}

fn reset_name(t: ResetType) -> &'static str {
    match t {
        ResetType::COLD => "a cold reset",
        ResetType::WARM => "a warm reset (memory is kept)",
        ResetType::SHUTDOWN => "a shutdown",
        ResetType::PLATFORM_SPECIFIC => "a platform-specific reset",
        _ => "an unknown kind of reset",
    }
}

/// The status of the last update attempt, as the ESRT and the Firmware
/// Management Protocol both report it.
fn attempt_status(s: u32) -> String {
    let text = match s {
        0 => "success",
        1 => "unsuccessful",
        2 => "insufficient resources",
        3 => "incorrect version",
        4 => "invalid format",
        5 => "authentication error",
        6 => "power event: not on mains power",
        7 => "power event: battery too low",
        8 => "unsatisfied dependencies",
        0x1000..=0x3fff => "a code of the firmware vendor's own",
        _ => "an undefined code",
    };
    format!("{s:#x}  ({text})")
}

fn capsule_kind(g: &Guid) -> &'static str {
    if *g == FMP_CAPSULE {
        "A firmware management capsule: the standard container for firmware updates."
    } else if *g == UX_CAPSULE {
        "A Windows UX capsule: a picture for the firmware to show while it applies the updates sent with it."
    } else if *g == JSON_CAPSULE {
        "A JSON capsule: configuration data for the firmware, as JSON."
    } else if *g == LAB_CAPSULE {
        "This program's test capsule."
    } else {
        "A type this program doesn't know. The GUID is the firmware's cue for who should read the capsule; vendors define their own."
    }
}

fn flag_bullets(doc: &mut Doc, flags: u32) {
    if flags & PERSIST != 0 {
        doc.bullet("PERSIST_ACROSS_RESET: keep it in memory and process it after a reset");
    }
    if flags & POPULATE != 0 {
        doc.bullet("POPULATE_SYSTEM_TABLE: after the reset, publish it as a configuration table");
    }
    if flags & INITIATE_RESET != 0 {
        doc.bullet("INITIATE_RESET: the firmware does the reset itself");
    }
    if flags & 0xffff != 0 {
        doc.bullet(&format!("{:#06x}: bits whose meaning belongs to this capsule's GUID", flags & 0xffff));
    }
    if flags & (PERSIST | POPULATE | INITIATE_RESET) == 0 {
        doc.bullet("no PERSIST_ACROSS_RESET: to be processed at once, inside the UpdateCapsule() call");
    }
}

// --------------------------------------------------------------------- ESRT

struct EsrtEntry {
    class: Guid,
    kind: u32,
    version: u32,
    lowest: u32,
    flags: u32,
    last_version: u32,
    last_status: u32,
}

/// The entries of the EFI System Resource Table. None: the firmware
/// publishes no such table.
fn esrt() -> Option<Vec<EsrtEntry>> {
    let addr = config_table(&ConfigTableEntry::ESRT_GUID)?;
    // SAFETY: the table is a 16-byte header (two counts and a version) and
    // then that many 40-byte entries.
    let count = (util::rd32(unsafe { systab::peek(addr, 16) }, 0)? as usize).min(64);
    let mut out = Vec::new();
    for n in 0..count {
        // SAFETY: as above.
        let e = unsafe { systab::peek(addr + 16 + n * 40, 40) };
        let field = |at: usize| util::rd32(e, at).unwrap_or(0);
        out.push(EsrtEntry {
            class: guid_at(e, 0)?,
            kind: field(16),
            version: field(20),
            lowest: field(24),
            flags: field(28),
            last_version: field(32),
            last_status: field(36),
        });
    }
    Some(out)
}

fn esrt_kind(kind: u32) -> &'static str {
    match kind {
        1 => "system firmware",
        2 => "device firmware",
        3 => "UEFI driver",
        _ => "unknown",
    }
}

fn esrt_page(doc: &mut Doc, e: &EsrtEntry, fmps: &[Fmp]) {
    doc.p("One entry of the EFI System Resource Table: a piece of firmware on this machine that a capsule can update. An OS matches the GUID against the updates on offer for it: fwupd does on Linux, Windows Update on Windows.");
    doc.blank();
    doc.kv("FwClass", format!("{}", e.class));
    doc.kv("Type", esrt_kind(e.kind));
    doc.kv("Version", version(e.version));
    doc.kv("Lowest allowed", version(e.lowest));
    doc.dim("An update older than the lowest allowed version is refused: this is what stops a rollback to firmware with a known hole.");
    doc.kv("Capsule flags", format!("{:#010x}", e.flags));
    doc.dim("What an OS should put in the flags of a capsule for this component.");
    doc.h("The last attempt to update it");
    doc.kv("Version", version(e.last_version));
    doc.kv("Status", attempt_status(e.last_status));
    doc.h("Behind the entry");
    match fmps.iter().flat_map(|f| f.images.iter().map(move |im| (f, im))).find(|(_, im)| im.type_id == e.class) {
        Some((f, im)) => {
            doc.p(&format!(
                "Image {} of the Firmware Management Protocol on handle {:#x} has this GUID. A capsule for this component ends up in that driver's SetImage().",
                im.index, f.handle
            ));
            doc.kv_styled("Signed updates", if im.auth_required() { st::GOOD } else { st::HEAD }, if im.auth_required() { "required" } else { "not required" });
        }
        None => doc.p("No Firmware Management Protocol instance has this GUID at the moment. The firmware fills the table from its own records, and some keep the driver that does the update out of the way until a capsule arrives."),
    }
}

// ---------------------------------------------------------------------- FMP

#[repr(transparent)]
#[unsafe_protocol("86c77a67-0b97-4633-a187-49104d0685c7")]
struct FmpProtocol(FirmwareManagementProtocol);

const ATTR_AUTH: u64 = 1 << 2;
const ATTRIBUTES: [(&str, &str); 6] = [
    ("Updatable", "the image can be replaced"),
    ("Reset required", "a new image takes effect after a reset"),
    ("Auth. required", "a new image must be signed"),
    ("In use", "this is the image running now"),
    ("UEFI image", "the image is a UEFI executable"),
    ("Dependency", "the image depends on the versions of other components"),
];

struct FmpImage {
    index: u8,
    type_id: Guid,
    id: u64,
    id_name: String,
    version: u32,
    version_name: String,
    size: usize,
    supported: u64,
    setting: u64,
    compatibilities: u64,
    /// The fields later versions of the descriptor added.
    lowest: Option<u32>,
    last_version: Option<u32>,
    last_status: Option<u32>,
    hardware_instance: Option<u64>,
}

impl FmpImage {
    fn auth_required(&self) -> bool {
        self.supported & self.setting & ATTR_AUTH != 0
    }
}

/// One instance of the Firmware Management Protocol and what its
/// GetImageInfo() returned.
struct Fmp {
    handle: usize,
    descriptor_version: u32,
    package_version: u32,
    package_name: String,
    images: Vec<FmpImage>,
    error: Option<Status>,
}

fn fmp_handles() -> Vec<Handle> {
    boot::locate_handle_buffer(SearchType::ByProtocol(&FmpProtocol::GUID)).map(|h| h.to_vec()).unwrap_or_default()
}

/// A NUL-terminated UCS-2 string the firmware points to.
///
/// # Safety
/// `at` must be zero or the address of such a string.
unsafe fn ucs2_at(at: usize) -> String {
    if at == 0 {
        return String::new();
    }
    // The iterator is only advanced up to the terminator.
    util::ucs2_to_string((0..128).map(|i| unsafe { (at as *const u16).add(i).read_unaligned() }))
}

fn read_fmp(handle: Handle) -> Fmp {
    let mut out = Fmp {
        handle: handle.as_ptr() as usize,
        descriptor_version: 0,
        package_version: 0,
        package_name: String::new(),
        images: Vec::new(),
        error: None,
    };
    let Some(proto) = util::open::<FmpProtocol>(handle) else {
        out.error = Some(Status::UNSUPPORTED);
        return out;
    };
    let raw: *const FirmwareManagementProtocol = &proto.0;
    let (mut size, mut count, mut each) = (0usize, 0u8, 0usize);
    let mut name: *mut u16 = core::ptr::null_mut();
    // Asked first with no buffer, to learn the size.
    // SAFETY: every pointer is to a local of the type the protocol
    // specifies, and the buffer is as long as `size` says.
    let mut status = unsafe {
        ((*raw).get_image_info)(raw, &mut size, core::ptr::null_mut(), &mut out.descriptor_version, &mut count, &mut each, &mut out.package_version, &mut name)
    };
    // u64s, so that the descriptors are aligned as the firmware expects.
    let mut buf: Vec<u64> = Vec::new();
    if status == Status::BUFFER_TOO_SMALL && (1..=1 << 20).contains(&size) {
        buf = alloc::vec![0u64; size.div_ceil(8)];
        size = buf.len() * 8;
        name = core::ptr::null_mut();
        // SAFETY: as above.
        status = unsafe {
            ((*raw).get_image_info)(raw, &mut size, buf.as_mut_ptr().cast(), &mut out.descriptor_version, &mut count, &mut each, &mut out.package_version, &mut name)
        };
    }
    if status != Status::SUCCESS {
        out.error = Some(status);
        return out;
    }
    // The firmware allocated the name for the caller to free. It is left
    // alone: a few bytes, against firmware that hands out a string it
    // didn't allocate.
    // SAFETY: on success the firmware set it to NULL or to a string.
    out.package_name = unsafe { ucs2_at(name as usize) };
    // SAFETY: the buffer is `buf`, initialized, and u8 has no alignment.
    let bytes = unsafe { core::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len() * 8) };
    // The first version of the descriptor ends after Compatibilities.
    if each < 88 {
        return out;
    }
    for d in bytes.chunks_exact(each).take(count as usize) {
        let ptr = |at: usize| util::rd64(d, at).unwrap_or(0) as usize;
        let later = |at: usize, since: u32| (out.descriptor_version >= since && each >= at + 4).then(|| util::rd32(d, at).unwrap_or(0));
        out.images.push(FmpImage {
            index: d[0],
            type_id: guid_at(d, 4).unwrap_or_default(),
            id: util::rd64(d, 24).unwrap_or(0),
            // SAFETY: the descriptor's string pointers are NULL or strings.
            id_name: unsafe { ucs2_at(ptr(32)) },
            version: util::rd32(d, 40).unwrap_or(0),
            // SAFETY: as above.
            version_name: unsafe { ucs2_at(ptr(48)) },
            size: ptr(56),
            supported: util::rd64(d, 64).unwrap_or(0),
            setting: util::rd64(d, 72).unwrap_or(0),
            compatibilities: util::rd64(d, 80).unwrap_or(0),
            lowest: later(88, 2),
            last_version: later(92, 3),
            last_status: later(96, 3),
            hardware_instance: (out.descriptor_version >= 3 && each >= 112).then(|| util::rd64(d, 104).unwrap_or(0)),
        });
    }
    out
}

fn fmp_page(doc: &mut Doc, f: &Fmp, im: &FmpImage) {
    doc.p("One image a Firmware Management Protocol instance describes. The instance is the driver that knows how to read and write this piece of firmware; a firmware management capsule that names this image type is handed to its SetImage().");
    doc.blank();
    doc.kv("Handle", format!("{:#x}", f.handle));
    doc.kv("Descriptor", format!("version {}", f.descriptor_version));
    doc.kv("ImageIndex", format!("{}", im.index));
    doc.kv("ImageTypeId", format!("{}", im.type_id));
    doc.dim("The GUID a capsule payload must carry to reach this image. The same GUID is this component's FwClass in the ESRT.");
    doc.kv("ImageId", format!("{:#x}  {}", im.id, im.id_name));
    doc.kv("Version", format!("{}  {}", version(im.version), im.version_name));
    if let Some(v) = im.lowest {
        doc.kv("Lowest allowed", version(v));
    }
    doc.kv("Size", util::size(im.size as u64));
    if let Some(v) = im.hardware_instance {
        doc.kv("Hardware inst.", format!("{v:#x}"));
    }
    doc.h("Attributes");
    for (bit, (name, about)) in ATTRIBUTES.iter().enumerate() {
        let (supported, set) = (im.supported & (1 << bit) != 0, im.setting & (1 << bit) != 0);
        let value = match (supported, set) {
            (false, _) => format!("not supported  ({about})"),
            (true, true) => format!("yes: {about}"),
            (true, false) => format!("no  ({about})"),
        };
        doc.kv_styled(name, if supported && set { st::BRIGHT } else { st::DIM }, &value);
    }
    doc.blank();
    if im.auth_required() {
        doc.p("Updates must be signed. The firmware holds a public key for this component and checks the signature in the capsule against it; the private half stays with whoever makes the updates.");
    } else {
        doc.styled(st::HEAD, "Updates need not be signed.");
        doc.p("The driver will write any image of the right type that reaches it. What protects this firmware, if anything, is elsewhere: who may call UpdateCapsule(), and whether the flash is locked.");
    }
    if im.compatibilities != 0 {
        doc.kv("Compatibilities", format!("{:#x}", im.compatibilities));
    }
    if let (Some(v), Some(s)) = (im.last_version, im.last_status) {
        doc.h("The last attempt to update it");
        doc.kv("Version", version(v));
        doc.kv("Status", attempt_status(s));
    }
    doc.h("The package the image belongs to");
    match f.package_version {
        0xffff_ffff => doc.kv("Version", "0xffffffff  (package versions not supported)"),
        0xffff_fffe => doc.kv("Version", "0xfffffffe  (no number: see the name)"),
        v => doc.kv("Version", version(v)),
    }
    if !f.package_name.is_empty() {
        doc.kv("Name", f.package_name.clone());
    }
}

/// The component on this machine with a GUID, from the Firmware Management
/// Protocol if an instance has it, or else from the ESRT.
struct Component {
    version: u32,
    lowest: Option<u32>,
    auth_required: Option<bool>,
}

fn component(ty: &Guid, esrt: &Option<Vec<EsrtEntry>>, fmps: &[Fmp]) -> Option<Component> {
    if let Some(im) = fmps.iter().flat_map(|f| f.images.iter()).find(|im| im.type_id == *ty) {
        return Some(Component {
            version: im.version,
            lowest: im.lowest,
            auth_required: Some(im.auth_required()),
        });
    }
    let e = esrt.as_ref()?.iter().find(|e| e.class == *ty)?;
    Some(Component {
        version: e.version,
        lowest: Some(e.lowest),
        auth_required: None,
    })
}

// ------------------------------------------------------- the capsule services

fn os_indications_supported() -> u64 {
    bootopt::global_var("OsIndicationsSupported").and_then(|(d, _)| util::rd64(&d, 0)).unwrap_or(0)
}

fn yes_no(doc: &mut Doc, key: &str, yes: bool, about: &str) {
    doc.kv_styled(key, if yes { st::GOOD } else { st::NORMAL }, &format!("{}  {about}", if yes { "yes" } else { "no" }));
}

fn support_page(doc: &mut Doc, esrt: &Option<Vec<EsrtEntry>>, fmps: &[Fmp]) {
    doc.p("A capsule is a block of data with a GUID on the front, handed to the firmware by the OS. Most carry a firmware update. This is what this firmware says about receiving them.");
    doc.h("Delivery");
    let arch = boot::locate_handle_buffer(SearchType::ByProtocol(&CAPSULE_ARCH)).map(|h| h.len()).unwrap_or(0) > 0;
    yes_no(doc, "Capsule driver", arch, "(the Capsule Architectural Protocol: UpdateCapsule() has a driver behind it)");
    match systab::rt_properties() {
        Some(mask) => yes_no(doc, "After boot", mask & (1 << 11) != 0, "(UpdateCapsule() still works once the OS is running, by the RT properties table)"),
        None => doc.kv("After boot", "presumed: no RT properties table says otherwise"),
    }
    let os = os_indications_supported();
    yes_no(doc, "By a file", os & 4 != 0, &format!("(a capsule left in {ON_DISK_DIR} on the EFI System Partition: OsIndications bit 2)"));
    yes_no(doc, "FMP capsules", os & 8 != 0, "(firmware management capsules: OsIndications bit 3)");
    yes_no(doc, "Result reports", os & 16 != 0, "(a result variable for each capsule processed: OsIndications bit 4)");
    doc.dim("The QueryCapsuleCapabilities row asks about particular capsules.");
    doc.h("What can be updated");
    match esrt {
        Some(e) => doc.kv("ESRT", format!("{} components", e.len())),
        None => doc.kv("ESRT", "not published"),
    }
    doc.kv("FMP instances", format!("{}, describing {} images", fmps.len(), fmps.iter().map(|f| f.images.len()).sum::<usize>()));
    doc.h("Waiting");
    let staged = runtime::get_variable_boxed(&util::cstring16("CapsuleUpdateData"), &VariableVendor(CAPSULE_VENDOR)).ok();
    match staged.and_then(|(d, _)| util::rd64(&d, 0)) {
        Some(addr) => {
            doc.kv_styled("In memory", st::HEAD, &format!("a capsule is staged, its block list at {addr:#x}"));
            doc.dim("edk2 keeps that address in the variable CapsuleUpdateData, for the next boot's early code to find after a warm reset.");
        }
        None => doc.kv("In memory", "nothing staged that this program can see"),
    }
    doc.blank();
    doc.dim("How a firmware remembers a staged capsule is its own business; the variable looked for here is edk2's.");
}

/// QueryCapsuleCapabilities() for a capsule with this GUID and these flags.
fn query(g: Guid, flags: u32) -> uefi::Result<CapsuleInfo> {
    // A whole small capsule, in case the firmware looks past the header.
    // For a firmware management capsule the body is that format's header,
    // announcing no drivers and no payloads.
    let mut space = [0u64; 16];
    space[4] = 1 << 32;
    let header = CapsuleHeader {
        capsule_guid: g,
        header_size: HEADER as u32,
        flags: CapsuleFlags::from_bits_retain(flags),
        capsule_image_size: 128,
    };
    // SAFETY: `space` is 128 bytes, aligned for the header.
    let header = unsafe {
        space.as_mut_ptr().cast::<CapsuleHeader>().write(header);
        &*space.as_ptr().cast::<CapsuleHeader>()
    };
    runtime::query_capsule_capabilities(&[header])
}

fn query_page(doc: &mut Doc) {
    doc.p("QueryCapsuleCapabilities() is how an OS asks before it sends: would this capsule be accepted, how large may it be, and which kind of reset does it need? Asking changes nothing. These are this firmware's answers for small capsules of each kind.");
    let cases: [(&str, Guid, u32, &str); 5] = [
        ("Processed at once", LAB_CAPSULE, 0, "No flags, and a GUID no firmware knows: the firmware would have to act on the capsule inside the call, and has no idea how."),
        ("Kept across a reset", LAB_CAPSULE, PERSIST, "PERSIST_ACROSS_RESET, same unknown GUID."),
        ("Kept, then published", LAB_CAPSULE, PERSIST | POPULATE, "PERSIST_ACROSS_RESET and POPULATE_SYSTEM_TABLE: after the reset the firmware need only list the capsule in the configuration table. The test capsule on this screen is this kind."),
        ("Kept, firmware resets", LAB_CAPSULE, PERSIST | INITIATE_RESET, "PERSIST_ACROSS_RESET and INITIATE_RESET."),
        ("A firmware update", FMP_CAPSULE, PERSIST, "A firmware management capsule with PERSIST_ACROSS_RESET: what an OS sends to update firmware."),
    ];
    for (title, g, flags, about) in cases {
        doc.h(title);
        doc.dim(about);
        match query(g, flags) {
            Ok(info) => {
                doc.kv_styled("Answer", st::GOOD, "accepted");
                doc.kv("Largest", util::size(info.maximum_capsule_size));
                doc.kv("Then needs", reset_name(info.reset_type));
            }
            Err(e) => doc.kv_styled("Answer", st::HEAD, &format!("{:?}", e.status())),
        }
    }
    doc.blank();
    doc.dim("UNSUPPORTED means the firmware has no use for that kind of capsule. Many builds of edk2, QEMU's among them, are made without the code that keeps a capsule across a reset.");
}

// --------------------------------------------------------- result variables

fn result_names() -> Vec<String> {
    let mut names: Vec<String> = runtime::variable_keys()
        .filter_map(|k| k.ok())
        .filter(|k| k.vendor.0 == CAPSULE_REPORT)
        .map(|k| String::from(&*k.name))
        .filter(|n| n.len() == 11 && n.starts_with("Capsule") && n[7..].chars().all(|c| c.is_ascii_hexdigit()))
        .collect();
    names.sort();
    names
}

/// A capsule result variable, decoded.
pub fn result_var(doc: &mut Doc, name: &str, data: &[u8]) {
    if matches!(name, "CapsuleMax" | "CapsuleLast") {
        doc.kv("Names", util::ucs2_bytes(data).0);
        return;
    }
    let (Some(total), Some(g), Some(status)) = (util::rd32(data, 0), guid_at(data, 8), util::rd64(data, 40)) else {
        doc.dim("Shorter than a result header (48 bytes).");
        return;
    };
    doc.kv("Total size", format!("{total} bytes"));
    doc.kv("Capsule GUID", guids::full(&g));
    doc.dim(capsule_kind(&g));
    // EFI_TIME: year, month, day, hour, minute, second.
    let t = &data[24..40];
    doc.kv("Processed", format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", u16::from_le_bytes([t[0], t[1]]), t[2], t[3], t[4], t[5], t[6]));
    let status = Status(status as usize);
    doc.kv_styled("Status", if status.is_success() { st::GOOD } else { st::BAD }, &format!("{status:?}"));
    if g != FMP_CAPSULE || data.len() < 68 {
        return;
    }
    // What a firmware management capsule's result adds.
    doc.kv("Payload", format!("number {} in the capsule", data[50]));
    doc.kv("ImageIndex", format!("{}", data[51]));
    if let Some(ty) = guid_at(data, 52) {
        doc.kv("ImageTypeId", format!("{ty}"));
    }
    let (file, used) = util::ucs2_bytes(&data[68..]);
    if !file.is_empty() {
        doc.kv("From the file", file);
    }
    let (target, _) = util::ucs2_bytes(data.get(68 + used..).unwrap_or(&[]));
    if !target.is_empty() {
        doc.kv("Target", target);
    }
}

fn result_page(doc: &mut Doc, name: &str) {
    doc.p("The firmware's report on one capsule it processed. An OS reads these after the restart to learn whether its update went in.");
    doc.blank();
    doc.kv("Variable", name);
    match runtime::get_variable_boxed(&util::cstring16(name), &VariableVendor(CAPSULE_REPORT)) {
        Ok((data, _)) => result_var(doc, name, &data),
        Err(e) => doc.kv_styled("GetVariable()", st::BAD, &format!("{:?}", e.status())),
    }
}

// ------------------------------------------------------------ capsule files

struct CapFile {
    path: String,
    size: u64,
}

/// The files in the two places this program looks for capsules: a
/// `capsules` directory beside itself, and where an OS leaves them.
fn cap_files() -> Vec<CapFile> {
    let mut out = Vec::new();
    let Some((device, dir)) = util::own_location() else {
        return out;
    };
    let Some(mut fs) = util::open::<SimpleFileSystem>(device) else {
        return out;
    };
    let Ok(mut root) = fs.open_volume() else {
        return out;
    };
    for folder in [format!("{dir}\\capsules"), String::from(ON_DISK_DIR)] {
        let Some(mut d) = root.open(&util::cstring16(&folder), FileMode::Read, FileAttribute::empty()).ok().and_then(|f| f.into_directory()) else {
            continue;
        };
        let _ = d.reset_entry_readout();
        while let Ok(Some(info)) = d.read_entry_boxed() {
            if !info.is_directory() && out.len() < 32 {
                out.push(CapFile {
                    path: format!("{folder}\\{}", info.file_name()),
                    size: info.file_size(),
                });
            }
        }
    }
    out
}

fn file_page(doc: &mut Doc, f: &CapFile, esrt: &Option<Vec<EsrtEntry>>, fmps: &[Fmp]) {
    doc.kv("File", f.path.clone());
    doc.kv("Size", util::size(f.size));
    if f.path.starts_with(ON_DISK_DIR) {
        doc.styled(st::HEAD, "This file is where an OS leaves a capsule for the firmware: with OsIndications bit 2 set, the firmware processes it on the next boot and deletes it.");
    }
    doc.dim("Read and described here. Nothing is sent to the firmware.");
    let read = util::own_location().and_then(|(device, _)| util::read_file(device, &f.path, MAX_READ));
    let Some((data, _)) = read else {
        doc.styled(st::BAD, "The file could not be read.");
        return;
    };
    if (data.len() as u64) < f.size {
        doc.dim(&format!("Only the first {} were read.", util::size(data.len() as u64)));
    }
    describe(doc, &data, f.size, esrt, fmps);
}

fn describe(doc: &mut Doc, d: &[u8], file_size: u64, esrt: &Option<Vec<EsrtEntry>>, fmps: &[Fmp]) {
    doc.h("EFI_CAPSULE_HEADER");
    let (Some(g), Some(header_size), Some(flags), Some(image_size)) = (guid_at(d, 0), util::rd32(d, 16), util::rd32(d, 20), util::rd32(d, 24)) else {
        doc.styled(st::BAD, "Too short to hold a capsule header (28 bytes).");
        return;
    };
    doc.kv("CapsuleGuid", guids::full(&g));
    doc.dim(capsule_kind(&g));
    doc.kv("HeaderSize", format!("{header_size} bytes"));
    doc.kv("Flags", format!("{flags:#010x}"));
    flag_bullets(doc, flags);
    if image_size as u64 == file_size {
        doc.kv_styled("ImageSize", st::GOOD, &format!("{image_size} bytes: the whole file"));
    } else {
        doc.kv_styled("ImageSize", st::HEAD, &format!("{image_size} bytes, but the file has {file_size}"));
    }
    let (start, end) = (header_size as usize, (image_size as usize).min(d.len()));
    if start < HEADER || start > end {
        doc.styled(st::BAD, "The sizes don't describe a capsule: this is probably some other kind of file.");
        return;
    }
    let body = &d[start..end];
    if g == FMP_CAPSULE {
        fmp_body(doc, body, esrt, fmps);
    } else if g == UX_CAPSULE {
        doc.h("The display payload");
        if let (Some(mode), Some(x), Some(y)) = (util::rd32(body, 4), util::rd32(body, 8), util::rd32(body, 12)) {
            doc.kv("Version", format!("{}", body[0]));
            doc.kv("Image type", if body[2] == 0 { "0: a bitmap (BMP)" } else { "unknown" });
            doc.kv("Video mode", format!("{mode}"));
            doc.kv("Position", format!("{x}, {y}"));
            doc.kv("Picture", util::size(body.len() as u64 - 16));
        }
    } else if g == JSON_CAPSULE {
        doc.h("EFI_JSON_CAPSULE_HEADER");
        if let (Some(v), Some(id), Some(len)) = (util::rd32(body, 0), util::rd32(body, 4), util::rd32(body, 8)) {
            doc.kv("Version", format!("{v}"));
            doc.kv("CapsuleId", format!("{id:#x}"));
            doc.kv("Payload", util::size(len as u64));
            let text = &body[12..body.len().min(12 + (len as usize).min(400))];
            doc.p(&String::from_utf8_lossy(text));
        }
    } else {
        doc.h("Body");
        doc.kv("Size", util::size(body.len() as u64));
        doc.hexdump(body, 64);
    }
}

fn fmp_body(doc: &mut Doc, b: &[u8], esrt: &Option<Vec<EsrtEntry>>, fmps: &[Fmp]) {
    doc.h("EFI_FIRMWARE_MANAGEMENT_CAPSULE_HEADER");
    let (Some(v), Some(drivers), Some(payloads)) = (util::rd32(b, 0), util::rd16(b, 4), util::rd16(b, 6)) else {
        doc.styled(st::BAD, "The file ends before this header does.");
        return;
    };
    let (drivers, payloads) = (drivers as usize, payloads as usize);
    doc.kv("Version", format!("{v}"));
    doc.kv("Drivers", format!("{drivers}"));
    doc.kv("Payloads", format!("{payloads}"));
    let offsets: Option<Vec<usize>> = (0..(drivers + payloads).min(64)).map(|i| util::rd64(b, 8 + i * 8).map(|o| o as usize)).collect();
    let Some(offsets) = offsets else {
        doc.styled(st::BAD, "The file ends inside the list of offsets.");
        return;
    };
    doc.dim("Then one offset for each, counted from the start of this header.");
    for (i, &at) in offsets.iter().enumerate() {
        // An item runs to the next one's offset, or to the end.
        let end = offsets.get(i + 1).copied().unwrap_or(b.len()).min(b.len());
        let Some(item) = b.get(at..end.max(at)) else {
            doc.styled(st::BAD, &format!("Item {} is at offset {at:#x}, which is outside the file.", i + 1));
            continue;
        };
        if i < drivers {
            doc.h(&format!("Driver {}", i + 1));
            doc.p("A UEFI driver the firmware loads before it applies the payloads: usually the one that installs the Firmware Management Protocol for the device being updated.");
            doc.kv("Offset", format!("{at:#x}"));
            doc.kv("Size", util::size(item.len() as u64));
            doc.kv("Format", if item.starts_with(b"MZ") { "a PE image" } else { "not a PE image" });
        } else {
            payload(doc, i - drivers + 1, at, item, esrt, fmps);
        }
    }
}

fn payload(doc: &mut Doc, number: usize, at: usize, p: &[u8], esrt: &Option<Vec<EsrtEntry>>, fmps: &[Fmp]) {
    doc.h(&format!("Payload {number}: EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER"));
    let (Some(v), Some(ty), Some(&index), Some(image_size), Some(vendor_size)) = (util::rd32(p, 0), guid_at(p, 4), p.get(20), util::rd32(p, 24), util::rd32(p, 28)) else {
        doc.styled(st::BAD, "The file ends before this header does.");
        return;
    };
    // Each version of the header added a field to the end.
    let header_len = match v {
        1 => 32,
        2 => 40,
        _ => 48,
    };
    doc.kv("Offset", format!("{at:#x}"));
    doc.kv("Version", format!("{v}"));
    doc.kv("ImageTypeId", format!("{ty}"));
    let here = component(&ty, esrt, fmps);
    match &here {
        Some(c) => doc.kv_styled("On this machine", st::GOOD, &format!("a component has this GUID, at version {:#x}", c.version)),
        None => doc.kv_styled("On this machine", st::HEAD, "no component has this GUID: this firmware would have nowhere to apply the payload"),
    }
    doc.kv("ImageIndex", format!("{index}"));
    doc.kv("Image", util::size(image_size as u64));
    doc.kv("Vendor code", util::size(vendor_size as u64));
    if v >= 2 {
        match util::rd64(p, 32) {
            Some(0) => doc.kv("Hardware inst.", "0: any instance of the device"),
            Some(h) => doc.kv("Hardware inst.", format!("{h:#x}")),
            None => {}
        }
    }
    if v >= 3 {
        let support = util::rd64(p, 40).unwrap_or(0);
        doc.kv("Capsule support", format!("{support:#x}"));
        if support & 1 != 0 {
            doc.bullet("the image carries a signature");
        }
        if support & 2 != 0 {
            doc.bullet("the image carries dependencies on other components' versions");
        }
    }
    let Some(image) = p.get(header_len..header_len + image_size as usize) else {
        doc.styled(st::HEAD, "The image runs past the end of what was read.");
        return;
    };

    // EFI_FIRMWARE_IMAGE_AUTHENTICATION: a counter, then a signature in a
    // WIN_CERTIFICATE_UEFI_GUID (length, revision 2.0, type 0x0EF1, GUID).
    let signed = util::rd16(image, 12) == Some(0x0200) && util::rd16(image, 14) == Some(0x0ef1) && guid_at(image, 16) == Some(PKCS7_CERT);
    let mut rest = image;
    if signed {
        let len = util::rd32(image, 8).unwrap_or(0) as usize;
        doc.h("EFI_FIRMWARE_IMAGE_AUTHENTICATION");
        doc.p("The image is signed. The firmware checks the signature against a public key built into it, before anything is written.");
        doc.kv("MonotonicCount", format!("{:#x}", util::rd64(image, 0).unwrap_or(0)));
        doc.dim("Signed together with the image, so that one signature can't be moved to another update.");
        doc.kv("Signature", format!("PKCS#7, {} bytes", len.saturating_sub(24)));
        let names = image.get(32..8 + len).map(vars::common_names).unwrap_or_default();
        if !names.is_empty() {
            doc.kv("Names in it", names.join(", "));
        }
        rest = image.get(8 + len..).unwrap_or(&[]);
    } else {
        doc.h("No signature");
        doc.styled(st::HEAD, "The image doesn't begin with an authentication header.");
        match here.as_ref().and_then(|c| c.auth_required) {
            Some(true) => doc.styled(st::BAD, "The component on this machine requires signed updates: it would refuse this one."),
            Some(false) => doc.p("The component on this machine doesn't require signed updates."),
            None => doc.p("A component whose attributes say \"authentication required\" refuses an image like this."),
        }
    }

    // edk2 puts a small header of its own in front of the firmware.
    if rest.starts_with(b"MSS1") && rest.len() >= 16 {
        let header = util::rd32(rest, 4).unwrap_or(16) as usize;
        let (new, lowest) = (util::rd32(rest, 8).unwrap_or(0), util::rd32(rest, 12).unwrap_or(0));
        doc.h("FMP_PAYLOAD_HEADER");
        doc.dim("edk2's own, not the specification's: how its update driver learns the version before it writes anything.");
        doc.kv("Version", version(new));
        doc.kv("Lowest allowed", version(lowest));
        if let Some(c) = &here {
            let (style, verdict) = if c.lowest.is_some_and(|l| new < l) {
                (st::BAD, "older than the lowest version this machine allows: refused as a rollback")
            } else if new > c.version {
                (st::GOOD, "newer than what is installed")
            } else if new == c.version {
                (st::NORMAL, "the version already installed")
            } else {
                (st::NORMAL, "older than what is installed, but not below the lowest allowed")
            };
            doc.kv_styled("Against this", style, verdict);
        }
        rest = rest.get(header..).unwrap_or(&[]);
    }
    doc.h("The firmware itself");
    doc.kv("Size", util::size(rest.len() as u64));
    doc.hexdump(rest, 32);
}

// ----------------------------------------------------------------- the lab

/// The test capsules the firmware has put in the configuration table: where
/// each is, its flags and its text.
fn landed() -> Vec<(usize, u32, String)> {
    let mut out = Vec::new();
    let Some(addr) = config_table(&LAB_CAPSULE) else {
        return out;
    };
    // EFI_CAPSULE_TABLE: a count, then (aligned) that many pointers.
    // SAFETY: the firmware published this table under the capsule's GUID.
    let count = (util::rd32(unsafe { systab::peek(addr, 8) }, 0).unwrap_or(0) as usize).min(4);
    for n in 0..count {
        // SAFETY: as above.
        let at = util::rd64(unsafe { systab::peek(addr + 8 + n * 8, 8) }, 0).unwrap_or(0) as usize;
        if at == 0 {
            continue;
        }
        // SAFETY: each pointer is to a capsule, which begins with a header.
        let head = unsafe { systab::peek(at, HEADER) };
        if guid_at(head, 0) != Some(LAB_CAPSULE) {
            continue;
        }
        let start = (util::rd32(head, 16).unwrap_or(0) as usize).clamp(HEADER, 4096);
        let end = (util::rd32(head, 24).unwrap_or(0) as usize).clamp(start, 4096);
        // SAFETY: the header says the capsule is this long.
        let text = unsafe { systab::peek(at + start, end - start) };
        out.push((at, util::rd32(head, 20).unwrap_or(0), String::from_utf8_lossy(text).into_owned()));
    }
    out
}

/// Whether the test capsule arrived, for this screen and for the system
/// table's.
pub fn landed_page(doc: &mut Doc) {
    let found = landed();
    if found.is_empty() {
        doc.p("No configuration table carries the test capsule's GUID.");
        doc.blank();
        doc.p("That is what to expect before a test capsule has been sent, after a restart that didn't keep memory (a cold reset or a power cycle), and on firmware that doesn't carry capsules across a reset.");
        doc.blank();
        doc.kv("Looked for", format!("{LAB_CAPSULE}"));
        return;
    }
    doc.styled(st::GOOD, "The test capsule is in the configuration table.");
    doc.blank();
    doc.p("It was handed to UpdateCapsule() before the reset. The firmware found it in memory on the way back up, copied it somewhere safe and listed it under its own GUID, as POPULATE_SYSTEM_TABLE asks. A firmware update travels the same way; the difference is that the firmware knows what to do with one.");
    for (at, flags, text) in found {
        doc.h("A capsule");
        doc.kv("Now at", format!("{at:#x}"));
        doc.kv("Flags", format!("{flags:#010x}"));
        doc.kv("It says", text);
    }
}

fn send_page(doc: &mut Doc) {
    doc.p("Builds a small capsule in memory and hands it to UpdateCapsule(), to watch the mechanism a firmware update rides on without updating anything.");
    doc.blank();
    doc.kv("CapsuleGuid", format!("{LAB_CAPSULE}"));
    doc.dim("Made up for this program, so there is no driver in any firmware that would act on it.");
    doc.kv("Flags", format!("{:#010x}", PERSIST | POPULATE));
    flag_bullets(doc, PERSIST | POPULATE);
    doc.kv("Body", "one line of text with the time it was sent");
    doc.blank();
    match query(LAB_CAPSULE, PERSIST | POPULATE) {
        Ok(info) => {
            doc.kv_styled("This firmware", st::GOOD, &format!("would accept it, and then needs {}", reset_name(info.reset_type)));
            doc.blank();
            doc.p("Enter asks once more, sends it, and offers to restart. After the restart, the row below shows whether it arrived.");
        }
        Err(e) => {
            doc.kv_styled("This firmware", st::HEAD, &format!("answers {:?} when asked about it", e.status()));
            doc.blank();
            doc.p("Enter tries anyway and shows what the firmware says. Nothing will be staged.");
        }
    }
    doc.blank();
    doc.dim("What the firmware is asked to do is keep one page of memory across a reset and point to a copy of it afterwards. A firmware that follows the specification writes nothing to its flash but its own note that a capsule is waiting.");
}

fn refusal(status: Status) -> String {
    match status {
        Status::UNSUPPORTED => String::from("UNSUPPORTED: this firmware doesn't take a capsule of this kind. Builds of edk2 keep capsules across a reset only if the platform asks for that when it is built, and QEMU's doesn't."),
        Status::OUT_OF_RESOURCES => String::from("OUT_OF_RESOURCES: the firmware knows the kind but has no room for it now."),
        Status::INVALID_PARAMETER => String::from("INVALID_PARAMETER: the firmware didn't like the capsule's flags or size."),
        Status::DEVICE_ERROR => String::from("DEVICE_ERROR: the firmware failed while handling it."),
        s => format!("{s:?}"),
    }
}

fn send(scr: &mut Screen) {
    let stamp = match runtime::get_time() {
        Ok(t) => format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.year(), t.month(), t.day(), t.hour(), t.minute(), t.second()),
        Err(_) => String::from("a time the clock wouldn't give"),
    };
    let text = format!("weefy sent this at {stamp}, before the reset.");
    let total = HEADER + text.len();
    // One page for the capsule and, in its second half, the list of blocks
    // that says where the capsule is. Below 4 GiB, where the early code of
    // any firmware can reach. Boot services memory: it only has to last
    // until the reset.
    let Ok(page) = boot::allocate_pages(AllocateType::MaxAddress(0xffff_ffff), MemoryType::BOOT_SERVICES_DATA, 1) else {
        tui::notice(scr, "No memory", &[(st::BAD, String::from("AllocatePages() had no page below 4 GiB to give."))]);
        return;
    };
    let base = page.as_ptr();
    // SAFETY: the page is 4096 bytes that belong to this program, aligned
    // for both structures; the references made here don't outlive it.
    let (header, blocks) = unsafe {
        base.write_bytes(0, 4096);
        base.cast::<CapsuleHeader>().write(CapsuleHeader {
            capsule_guid: LAB_CAPSULE,
            header_size: HEADER as u32,
            flags: CapsuleFlags::from_bits_retain(PERSIST | POPULATE),
            capsule_image_size: total as u32,
        });
        core::ptr::copy_nonoverlapping(text.as_ptr(), base.add(HEADER), text.len());
        let list = base.add(2048).cast::<CapsuleBlockDescriptor>();
        // One block holding the whole capsule, then the end of the list.
        list.write(CapsuleBlockDescriptor {
            length: total as u64,
            address: base as u64,
        });
        list.add(1).write(CapsuleBlockDescriptor { length: 0, address: 0 });
        (&*base.cast::<CapsuleHeader>(), core::slice::from_raw_parts(list, 2))
    };
    let free = || {
        // SAFETY: the page came from allocate_pages above and the firmware
        // was not left holding its address.
        let _ = unsafe { boot::free_pages(page, 1) };
    };

    let info = match runtime::query_capsule_capabilities(&[header]) {
        Ok(info) => info,
        Err(e) => {
            // What UpdateCapsule() itself says, for the record: with the
            // same answer nothing has been staged.
            let direct = runtime::update_capsule(&[header], blocks).map_err(|e| e.status());
            if direct.is_ok() {
                staged(scr, base as usize, total, ResetType::WARM, "UpdateCapsule() accepted it, though QueryCapsuleCapabilities() had said no.");
                return;
            }
            free();
            tui::notice(
                scr,
                "The firmware said no",
                &[
                    (st::BAD, format!("QueryCapsuleCapabilities(): {}", refusal(e.status()))),
                    (st::NORMAL, String::new()),
                    (st::NORMAL, format!("UpdateCapsule() with the same capsule: {:?}.", direct.unwrap_err())),
                    (st::NORMAL, String::new()),
                    (st::DIM, String::from("Nothing was staged. On a machine whose firmware does firmware updates by capsule (most PCs sold with Windows), the same steps go through.")),
                ],
            );
            return;
        }
    };
    let key = tui::message(
        scr,
        "Send the test capsule?",
        &|w| {
            let mut d = Doc::new(w);
            d.styled(st::GOOD, "QueryCapsuleCapabilities() says this firmware will take it.");
            d.kv("Largest", util::size(info.maximum_capsule_size));
            d.kv("Then needs", reset_name(info.reset_type));
            d.kv("Capsule", format!("{total} bytes at {:#x}", base as usize));
            d.blank();
            d.p("Enter calls UpdateCapsule(). The firmware notes where the capsule is; nothing more happens until the machine is reset.");
            d
        },
        &[("Enter", "Send"), ("Esc", "Cancel")],
    );
    if key != Key::Enter {
        free();
        return;
    }
    match runtime::update_capsule(&[header], blocks) {
        Ok(()) => staged(scr, base as usize, total, info.reset_type, "UpdateCapsule() returned SUCCESS."),
        Err(e) => {
            free();
            tui::notice(scr, "UpdateCapsule() refused", &[(st::BAD, refusal(e.status())), (st::NORMAL, String::new()), (st::DIM, String::from("Nothing was staged."))]);
        }
    }
}

/// The capsule is with the firmware: restart, or leave it waiting.
fn staged(scr: &mut Screen, at: usize, total: usize, reset: ResetType, how: &str) {
    let key = tui::message(
        scr,
        "The capsule is staged",
        &|w| {
            let mut d = Doc::new(w);
            d.styled(st::GOOD, how);
            d.kv("Capsule", format!("{total} bytes at {at:#x}"));
            d.blank();
            d.p(&format!("It is still only in memory. Enter restarts the machine with ResetSystem(), asking for {}: the kind the firmware said this capsule needs.", reset_name(reset)));
            d.blank();
            d.p("When weefy is back, open this screen again: \"Did it arrive?\" looks for the capsule in the configuration table.");
            d.blank();
            d.dim("Esc leaves it waiting. It is lost if the machine is powered off, or once an OS reuses the memory.");
            d
        },
        &[("Enter", "Restart now"), ("Esc", "Later")],
    );
    if key == Key::Enter {
        scr.release();
        runtime::reset(reset, Status::SUCCESS, None);
    }
}

// --------------------------------------------------------------- the screen

#[derive(Clone, Copy)]
enum Row {
    Support,
    Query,
    Esrt(usize),
    NoEsrt,
    Fmp(usize, usize),
    FmpError(usize),
    NoFmp,
    Result(usize),
    NoResults,
    File(usize),
    NoFiles,
    Send,
    Landed,
}

/// One line for the main menu.
pub fn summary() -> String {
    let fmp = fmp_handles().len();
    match esrt() {
        Some(e) => format!("{} in the ESRT, {fmp} Firmware Management Protocol instances", e.len()),
        None => format!("no ESRT, {fmp} Firmware Management Protocol instances"),
    }
}

pub fn run(scr: &mut Screen) {
    let mut state = BrowseState::default();
    loop {
        let esrt = esrt();
        let fmps: Vec<Fmp> = fmp_handles().into_iter().map(read_fmp).collect();
        let results = result_names();
        let files = cap_files();
        let arrived = !landed().is_empty();

        let mut rows: Vec<(Option<Row>, String, tui::Style)> = Vec::new();
        let head = |rows: &mut Vec<(Option<Row>, String, tui::Style)>, label: &str| rows.push((None, label.into(), st::NORMAL));
        head(&mut rows, "This firmware");
        rows.push((Some(Row::Support), "What it supports".into(), st::NORMAL));
        rows.push((Some(Row::Query), "QueryCapsuleCapabilities".into(), st::NORMAL));
        head(&mut rows, "Updatable components (ESRT)");
        match &esrt {
            Some(entries) if !entries.is_empty() => {
                for (i, e) in entries.iter().enumerate() {
                    rows.push((Some(Row::Esrt(i)), format!("{} {:#x}", esrt_kind(e.kind), e.version), st::NORMAL));
                }
            }
            _ => rows.push((Some(Row::NoEsrt), "None".into(), st::DIM)),
        }
        head(&mut rows, "Firmware Management Protocol");
        for (i, f) in fmps.iter().enumerate() {
            if let Some(status) = f.error {
                rows.push((Some(Row::FmpError(i)), format!("{status:?}"), st::BAD));
            }
            for (j, im) in f.images.iter().enumerate() {
                let name = if im.id_name.is_empty() { format!("image {}", im.index) } else { im.id_name.clone() };
                rows.push((Some(Row::Fmp(i, j)), format!("{name} {:#x}", im.version), st::NORMAL));
            }
        }
        if fmps.is_empty() {
            rows.push((Some(Row::NoFmp), "None".into(), st::DIM));
        }
        head(&mut rows, "Results of past capsules");
        for (i, name) in results.iter().enumerate() {
            rows.push((Some(Row::Result(i)), name.clone(), st::NORMAL));
        }
        if results.is_empty() {
            rows.push((Some(Row::NoResults), "None".into(), st::DIM));
        }
        head(&mut rows, "Capsule files on this disk");
        for (i, f) in files.iter().enumerate() {
            let name = f.path.rsplit('\\').next().unwrap_or(&f.path);
            let style = if f.path.starts_with(ON_DISK_DIR) { st::HEAD } else { st::NORMAL };
            rows.push((Some(Row::File(i)), name.into(), style));
        }
        if files.is_empty() {
            rows.push((Some(Row::NoFiles), "None".into(), st::DIM));
        }
        head(&mut rows, "Try it");
        rows.push((Some(Row::Send), "Send a test capsule".into(), st::NORMAL));
        rows.push((Some(Row::Landed), if arrived { "Did it arrive? Yes".into() } else { "Did it arrive?".into() }, if arrived { st::GOOD } else { st::NORMAL }));
        let items: Vec<Item> = rows
            .iter()
            .map(|(row, label, style)| match row {
                None => Item::header(label.clone()),
                Some(_) => Item::new(label.clone()).styled(*style),
            })
            .collect();

        let out = tui::browse(scr, &mut state, "Capsules and firmware updates", "Capsules", &items, &[("r", "Refresh")], &mut |row, width| {
            let mut doc = Doc::new(width);
            match rows[row].0 {
                Some(Row::Support) => support_page(&mut doc, &esrt, &fmps),
                Some(Row::Query) => query_page(&mut doc),
                Some(Row::Esrt(i)) => {
                    if let Some(e) = esrt.as_ref().and_then(|e| e.get(i)) {
                        esrt_page(&mut doc, e, &fmps);
                    }
                }
                Some(Row::NoEsrt) => {
                    doc.styled(st::HEAD, if esrt.is_some() { "The ESRT is there, with no entries." } else { "This firmware publishes no ESRT." });
                    doc.blank();
                    doc.p("The EFI System Resource Table is a configuration table listing each piece of firmware a capsule can update: a GUID, the version installed, the lowest version allowed, and how the last attempt went. It is what an OS reads to know which updates apply to the machine.");
                    doc.blank();
                    doc.p("Without it the OS has nothing to match updates against, so tools such as fwupd offer none. Virtual machines and many small boards are like this: their firmware is replaced from outside, not by capsule.");
                }
                Some(Row::Fmp(i, j)) => fmp_page(&mut doc, &fmps[i], &fmps[i].images[j]),
                Some(Row::FmpError(i)) => {
                    doc.p("A handle carries the Firmware Management Protocol, but its GetImageInfo() didn't answer.");
                    doc.blank();
                    doc.kv("Handle", format!("{:#x}", fmps[i].handle));
                    doc.kv_styled("GetImageInfo()", st::BAD, &format!("{:?}", fmps[i].error.unwrap_or(Status::SUCCESS)));
                }
                Some(Row::NoFmp) => {
                    doc.styled(st::HEAD, "No handle carries the Firmware Management Protocol.");
                    doc.blank();
                    doc.p("The protocol is the driver's side of a firmware update. Each instance describes the images it looks after (GetImageInfo), and can check one (CheckImage), write one (SetImage) and read one back (GetImage). When a firmware management capsule arrives, the firmware finds the instance whose image type matches the payload's GUID and calls its SetImage().");
                    doc.blank();
                    doc.p("Firmware that takes no capsule updates has no instances. Some firmware that does take them still shows none here, because it loads the driver only when a capsule is being processed.");
                }
                Some(Row::Result(i)) => result_page(&mut doc, &results[i]),
                Some(Row::NoResults) => {
                    doc.styled(st::HEAD, "No capsule result variables.");
                    doc.blank();
                    doc.p("After it processes a capsule, a firmware that supports result reporting writes a variable named Capsule0000, Capsule0001 and so on: which capsule, when, and the status. CapsuleLast names the newest. An OS reads them after the restart to tell the user whether the update went in.");
                    doc.blank();
                    doc.kv("Namespace", format!("{CAPSULE_REPORT}"));
                    yes_no(&mut doc, "Supported here", os_indications_supported() & 16 != 0, "(OsIndicationsSupported bit 4)");
                }
                Some(Row::File(i)) => file_page(&mut doc, &files[i], &esrt, &fmps),
                Some(Row::NoFiles) => {
                    doc.styled(st::HEAD, "No capsule files found.");
                    doc.blank();
                    doc.p("Looked in two places on the disk this program started from:");
                    doc.bullet("a directory named capsules beside this program, for files you want to look at");
                    doc.bullet(&format!("{ON_DISK_DIR}, where an OS leaves a capsule for the firmware to apply on the next boot"));
                    doc.blank();
                    doc.p("Put a vendor's firmware update there (the .cap or .bin inside the package fwupd or the vendor's site gives you) and this screen takes it apart: the headers, the component it is for, who signed it, and whether this machine would want it. Nothing is sent to the firmware.");
                    doc.blank();
                    doc.dim("tools/mkcapsule.py makes sample capsules; run-qemu.sh and make-image.sh put them on the disk.");
                }
                Some(Row::Send) => send_page(&mut doc),
                Some(Row::Landed) => landed_page(&mut doc),
                None => {}
            }
            doc
        });
        match out {
            Outcome::Back => return,
            Outcome::Open(row) => match rows[row].0 {
                Some(Row::Send) => send(scr),
                _ => state.zoom = true,
            },
            Outcome::Char(_) => {}
        }
    }
}
