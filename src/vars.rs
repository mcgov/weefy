//! Every UEFI variable, with the ones the specification defines decoded.
//! Read-only.

use crate::bootopt;
use crate::guids;
use crate::tui::{self, BrowseState, Doc, Item, Outcome, Screen, st};
use crate::util;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use uefi::runtime::{self, VariableAttributes, VariableVendor};
use uefi::{CString16, Guid};

struct Var {
    name: CString16,
    vendor: Guid,
}

fn collect() -> Vec<Var> {
    let mut vars: Vec<Var> = runtime::variable_keys()
        .filter_map(|k| k.ok())
        .map(|k| Var {
            name: k.name,
            vendor: k.vendor.0,
        })
        .collect();
    // The specification's own namespace first, then the rest by name.
    let global = VariableVendor::GLOBAL_VARIABLE.0;
    vars.sort_by_cached_key(|v| (v.vendor != global, guids::short(&v.vendor).to_lowercase(), String::from(&*v.name).to_lowercase()));
    vars
}

fn attributes(doc: &mut Doc, a: VariableAttributes) {
    let mut parts = Vec::new();
    parts.push(if a.contains(VariableAttributes::NON_VOLATILE) {
        "NV: kept across resets"
    } else {
        "volatile: gone at power-off"
    });
    if a.contains(VariableAttributes::BOOTSERVICE_ACCESS) {
        parts.push("BS: readable before the OS starts");
    }
    parts.push(if a.contains(VariableAttributes::RUNTIME_ACCESS) {
        "RT: readable from the OS"
    } else {
        "no RT: hidden from the OS"
    });
    if a.contains(VariableAttributes::TIME_BASED_AUTHENTICATED_WRITE_ACCESS) || a.contains(VariableAttributes::AUTHENTICATED_WRITE_ACCESS) {
        parts.push("AT: writes must be signed");
    }
    if a.contains(VariableAttributes::HARDWARE_ERROR_RECORD) {
        parts.push("hardware error record");
    }
    doc.kv("Attributes", format!("{:#x}", a.bits()));
    for p in parts {
        doc.bullet(p);
    }
}

/// What a variable of the Global Variable namespace is for.
fn global_about(name: &str) -> &'static str {
    let numbered = |prefix: &str| name.len() == prefix.len() + 4 && name.starts_with(prefix) && name[prefix.len()..].chars().all(|c| c.is_ascii_hexdigit());
    if numbered("Boot") {
        return "A boot option: a description, the device path of what to load, and optional data passed to it. BootOrder refers to it by its four hex digits.";
    }
    if numbered("Driver") {
        return "A driver load option: a driver the firmware loads before it boots, listed in DriverOrder.";
    }
    if numbered("SysPrep") {
        return "A system preparation application, run before the boot options.";
    }
    if numbered("PlatformRecovery") || numbered("OsRecovery") {
        return "A recovery option, tried when every boot option has failed.";
    }
    if numbered("Key") {
        return "A hot key bound to a boot option.";
    }
    match name {
        "BootOrder" => "The boot options to try, in order, as a list of option numbers. The first one that loads and doesn't return is the OS you get.",
        "BootNext" => "A boot option to try first on the next boot only. The firmware deletes it before using it.",
        "BootCurrent" => "The boot option that was selected for this boot.",
        "BootOptionSupport" => "What the boot manager can do: hot keys, applications, recovery.",
        "DriverOrder" => "The order in which Driver#### options are loaded.",
        "SysPrepOrder" => "The order in which SysPrep#### applications run.",
        "Timeout" => "Seconds the boot manager waits before it boots the default. 0xFFFF waits forever; 0 doesn't wait.",
        "PlatformLang" => "The language in use, as an RFC 4646 code such as en-US.",
        "PlatformLangCodes" => "The languages the firmware supports.",
        "Lang" => "The language in use, in the old three-letter form.",
        "LangCodes" => "The languages the firmware supports, in the old three-letter form.",
        "ConIn" => "The device paths of the input devices that make up the console.",
        "ConOut" => "The device paths of the output devices that make up the console.",
        "ErrOut" => "The device paths of the devices errors are written to.",
        "ConInDev" => "Every device that could be a console input.",
        "ConOutDev" => "Every device that could be a console output.",
        "ErrOutDev" => "Every device that could be an error output.",
        "SecureBoot" => "1 if Secure Boot is enforcing signatures on this boot. Read-only; the firmware computes it.",
        "SetupMode" => "1 if no Platform Key is enrolled: the key variables can be written without a signature, and Secure Boot is off.",
        "AuditMode" => "1 if the firmware records signature failures without enforcing them.",
        "DeployedMode" => "1 if Secure Boot is locked into its most restrictive mode.",
        "PK" => "The Platform Key: the certificate whose owner may change KEK. Normally the manufacturer's.",
        "KEK" => "The Key Exchange Keys: the certificates whose owners may change db and dbx.",
        "PKDefault" | "KEKDefault" | "dbDefault" | "dbxDefault" | "dbtDefault" | "dbrDefault" => "The factory copy of a Secure Boot variable, for restoring the defaults.",
        "SignatureSupport" => "The signature types the firmware can verify, as a list of GUIDs.",
        "VendorKeys" => "1 if the Secure Boot keys are still exactly the manufacturer's.",
        "OsIndications" => "Requests from the OS to the firmware for the next boot, one bit each.",
        "OsIndicationsSupported" => "The OsIndications requests this firmware can honour.",
        "HwErrRecSupport" => "Whether the firmware stores hardware error records.",
        "devAuthBoot" | "devdbDefault" => "Device authentication (SPDM) state.",
        _ => "",
    }
}

fn other_about(name: &str, vendor: &Guid) -> &'static str {
    if *vendor == VariableVendor::IMAGE_SECURITY_DATABASE.0 {
        return match name {
            "db" => "Secure Boot's allow list: certificates whose signatures are accepted, and hashes of individual images.",
            "dbx" => "Secure Boot's revocation list: certificates and image hashes that are refused. It overrides db.",
            "dbt" => "Trusted timestamping certificates.",
            "dbr" => "Keys that authorize OS recovery.",
            _ => "",
        };
    }
    match name {
        "MokList" | "MokListRT" => "shim's Machine Owner Keys: certificates the machine's owner enrolled, trusted for kernels and modules in addition to the distribution's own.",
        "MokListX" | "MokListXRT" => "shim's revocation list.",
        "SbatLevel" | "SbatLevelRT" => "shim's generation-based revocation level (SBAT).",
        "LoaderInfo" => "The name and version of the boot loader that ran (Boot Loader Interface).",
        "LoaderEntrySelected" => "The boot loader menu entry that was booted.",
        "LoaderDevicePartUUID" => "The partition the boot loader was started from.",
        "LoaderTimeInitUSec" | "LoaderTimeExecUSec" => "Timing from the boot loader, in microseconds, for systemd-analyze.",
        "MTC" => "The high 32 bits of the monotonic counter.",
        "MemoryTypeInformation" => "How much memory of each type the last boot used, so the firmware can reserve the same up front and keep the memory map stable.",
        "certdb" | "certdbv" => "The firmware's record of which certificate protects each authenticated variable.",
        "VarErrorFlag" => "A flag the variable driver sets when the store runs out of space.",
        "NvVars" => "Marks that non-volatile variables were saved to a file on the EFI System Partition (firmware without flash for them).",
        _ => "",
    }
}

pub fn signature_lists(doc: &mut Doc, data: &[u8]) {
    let mut at = 0;
    let mut n = 0;
    while at + 28 <= data.len() {
        let ty = Guid::from_bytes(data[at..at + 16].try_into().unwrap());
        let list = util::rd32(data, at + 16).unwrap_or(0) as usize;
        let head = util::rd32(data, at + 20).unwrap_or(0) as usize;
        let each = util::rd32(data, at + 24).unwrap_or(0) as usize;
        if list < 28 || at + list > data.len() || each < 16 {
            break;
        }
        n += 1;
        let count = (list - 28 - head) / each;
        doc.h(&format!("Signature list {n}"));
        doc.kv("Type", guids::short(&ty));
        doc.kv("Entries", format!("{count}, {} bytes each", each - 16));
        let x509 = guids::lookup(&ty).map(|n| n.0) == Some("EfiCertX509");
        for i in 0..count.min(8) {
            let sig = &data[at + 28 + head + i * each..at + 28 + head + (i + 1) * each];
            let owner = Guid::from_bytes(sig[..16].try_into().unwrap());
            doc.kv("Owner", guids::full(&owner));
            if x509 {
                let names = common_names(&sig[16..]);
                if !names.is_empty() {
                    doc.kv("Names in it", names.join("; "));
                }
            } else if each - 16 <= 64 {
                doc.kv("Value", sig[16..].iter().map(|b| format!("{b:02x}")).collect::<String>());
            }
        }
        if count > 8 {
            doc.dim(&format!("... and {} more", count - 8));
        }
        at += list;
    }
    if n == 0 {
        doc.dim("No signature lists: empty.");
    }
}

/// The common names in a DER certificate (the issuer's and the subject's),
/// found by looking for the commonName attribute rather than by parsing.
pub fn common_names(der: &[u8]) -> Vec<String> {
    const CN: [u8; 5] = [0x06, 0x03, 0x55, 0x04, 0x03];
    let mut out: Vec<String> = Vec::new();
    let mut at = 0;
    while at + 7 < der.len() && out.len() < 4 {
        if der[at..at + 5] == CN {
            let len = der[at + 6] as usize;
            if len < 0x80 {
                if let Some(text) = der.get(at + 7..at + 7 + len) {
                    let name: String = text.iter().filter(|b| (0x20..0x7f).contains(*b)).map(|&b| b as char).collect();
                    if !name.is_empty() && !out.contains(&name) {
                        out.push(name);
                    }
                }
            }
            at += 7;
        } else {
            at += 1;
        }
    }
    out
}

fn bits(doc: &mut Doc, value: u64, names: &[&str]) {
    for (bit, name) in names.iter().enumerate() {
        if value & (1 << bit) != 0 {
            doc.bullet(name);
        }
    }
    if value == 0 {
        doc.dim("No bits set.");
    }
}

fn decode(doc: &mut Doc, name: &str, vendor: &Guid, data: &[u8]) {
    let global = *vendor == VariableVendor::GLOBAL_VARIABLE.0;
    let numbered = |prefix: &str| global && name.len() == prefix.len() + 4 && name.starts_with(prefix) && name[prefix.len()..].chars().all(|c| c.is_ascii_hexdigit());
    if numbered("Boot") || numbered("Driver") || numbered("SysPrep") || numbered("PlatformRecovery") {
        let Some(opt) = bootopt::parse_load_option(data) else {
            return;
        };
        doc.h("Decoded: EFI_LOAD_OPTION");
        doc.kv("Description", opt.description);
        let mut flags = Vec::new();
        flags.push(if opt.attributes & bootopt::LOAD_OPTION_ACTIVE != 0 { "active" } else { "inactive: skipped" });
        if opt.attributes & bootopt::LOAD_OPTION_HIDDEN != 0 {
            flags.push("hidden from menus");
        }
        if opt.attributes & bootopt::LOAD_OPTION_CATEGORY != 0 {
            flags.push("an application, not an OS");
        }
        doc.kv("Attributes", format!("{:#x}: {}", opt.attributes, flags.join(", ")));
        doc.kv("Device path", util::dp_bytes_text(&opt.path));
        doc.dim(bootopt::path_kind(&opt.path));
        if opt.optional.is_empty() {
            doc.kv("Optional data", "none");
        } else {
            doc.kv("Optional data", format!("{} bytes, passed to the image as its load options", opt.optional.len()));
            let (text, used) = util::ucs2_bytes(&opt.optional);
            if used >= opt.optional.len().saturating_sub(1) && text.chars().count() > 1 && text.chars().all(|c| (' '..='~').contains(&c)) {
                doc.kv("As text", text);
            }
        }
    } else if global && matches!(name, "BootOrder" | "DriverOrder" | "SysPrepOrder") {
        doc.h("Decoded: a list of option numbers");
        let prefix = name.strip_suffix("Order").unwrap_or("Boot");
        for (i, c) in data.chunks_exact(2).enumerate() {
            let n = u16::from_le_bytes([c[0], c[1]]);
            let title = bootopt::global_var(&format!("{prefix}{n:04X}"))
                .and_then(|(d, _)| bootopt::parse_load_option(&d))
                .map(|o| o.description)
                .unwrap_or_else(|| String::from("(no such variable)"));
            doc.kv(&format!("{}. {prefix}{n:04X}", i + 1), title);
        }
    } else if global && matches!(name, "BootNext" | "BootCurrent") {
        if let Some(n) = util::rd16(data, 0) {
            let title = bootopt::global_var(&format!("Boot{n:04X}"))
                .and_then(|(d, _)| bootopt::parse_load_option(&d))
                .map(|o| o.description)
                .unwrap_or_default();
            doc.h("Decoded");
            doc.kv("Option", format!("Boot{n:04X}  {title}"));
        }
    } else if global && name == "Timeout" {
        if let Some(n) = util::rd16(data, 0) {
            doc.h("Decoded");
            doc.kv("Seconds", if n == 0xffff { String::from("0xFFFF: wait forever") } else { format!("{n}") });
        }
    } else if global && matches!(name, "SecureBoot" | "SetupMode" | "AuditMode" | "DeployedMode" | "VendorKeys") {
        doc.h("Decoded");
        doc.kv("Value", match data.first() {
            Some(1) => "1: yes",
            Some(0) => "0: no",
            _ => "unexpected",
        });
    } else if global && matches!(name, "PlatformLang" | "PlatformLangCodes" | "Lang" | "LangCodes") {
        doc.h("Decoded: ASCII text");
        doc.kv("Value", data.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect::<String>());
    } else if global && matches!(name, "ConIn" | "ConOut" | "ErrOut" | "ConInDev" | "ConOutDev" | "ErrOutDev") {
        doc.h("Decoded: device paths");
        // Several paths in one: instances are separated by an
        // end-of-instance node (type 0x7f, subtype 1).
        let mut at = 0;
        while at + 4 <= data.len() {
            let body = util::dp_body(&data[at..]);
            if body.is_empty() {
                break;
            }
            doc.bullet(&util::dp_bytes_text(&data[at..]));
            at += body.len() + 4;
        }
    } else if global && matches!(name, "OsIndications" | "OsIndicationsSupported") {
        doc.h("Decoded: one request per bit");
        let v = util::rd64(data, 0).unwrap_or(0);
        bits(doc, v, &[
            "bit 0: boot into the firmware's setup screen",
            "bit 1: timestamp-based revocation",
            "bit 2: capsule delivery by a file on the ESP",
            "bit 3: firmware management capsules",
            "bit 4: capsule result variables",
            "bit 5: start OS recovery",
            "bit 6: start platform recovery",
            "bit 7: refresh JSON configuration data",
        ]);
    } else if global && name == "BootOptionSupport" {
        doc.h("Decoded");
        let v = util::rd32(data, 0).unwrap_or(0) as u64;
        bits(doc, v, &["bit 0: hot keys", "bit 1: application options", "", "", "bit 4: SysPrep options"]);
    } else if global && name == "SignatureSupport" {
        doc.h("Decoded: signature types");
        for c in data.chunks_exact(16) {
            doc.bullet(&guids::short(&Guid::from_bytes(c.try_into().unwrap())));
        }
    } else if (global && matches!(name, "PK" | "KEK" | "PKDefault" | "KEKDefault" | "dbDefault" | "dbxDefault" | "dbtDefault"))
        || (*vendor == VariableVendor::IMAGE_SECURITY_DATABASE.0 && matches!(name, "db" | "dbx" | "dbt" | "dbr"))
        || matches!(name, "MokListRT" | "MokListXRT" | "MokListTrustedRT")
    {
        signature_lists(doc, data);
    } else if data.len() >= 4 && data.len() % 2 == 0 {
        // Many vendor variables are just text.
        let (text, used) = util::ucs2_bytes(data);
        if used == data.len() && text.chars().count() >= 2 && text.chars().all(|c| (' '..='~').contains(&c)) {
            doc.h("Decoded: UCS-2 text");
            doc.kv("Value", text);
        }
    }
}

pub fn run(scr: &mut Screen) {
    let vars = collect();
    if vars.is_empty() {
        tui::message(
            scr,
            "Variables",
            &|w| {
                let mut d = Doc::new(w);
                d.p("GetNextVariableName() returned no variables. This firmware has an empty variable store, or doesn't offer variable services.");
                d
            },
            &[("Any key", "Back")],
        );
        return;
    }
    // Rows, with a header for each vendor namespace.
    let mut items = Vec::new();
    let mut map: Vec<Option<usize>> = Vec::new();
    let mut last: Option<Guid> = None;
    for (i, v) in vars.iter().enumerate() {
        if last != Some(v.vendor) {
            let name = guids::lookup(&v.vendor).map(|n| String::from(n.0)).or(guids::extra(&v.vendor).map(|n| String::from(n.0)));
            items.push(Item::header(name.unwrap_or_else(|| format!("{}", v.vendor))));
            map.push(None);
            last = Some(v.vendor);
        }
        items.push(Item::new(String::from(&*v.name)));
        map.push(Some(i));
    }
    let mut state = BrowseState::default();
    loop {
        let out = tui::browse(scr, &mut state, "Variables", "Variables", &items, &[("r", "Refresh")], &mut |row, width| {
            let mut doc = Doc::new(width);
            let Some(i) = map[row] else {
                return doc;
            };
            let v = &vars[i];
            let name = String::from(&*v.name);
            let about = if v.vendor == VariableVendor::GLOBAL_VARIABLE.0 { global_about(&name) } else { other_about(&name, &v.vendor) };
            if !about.is_empty() {
                doc.p(about);
                doc.blank();
            }
            doc.kv("Name", name.clone());
            doc.kv("Vendor GUID", guids::full(&v.vendor));
            if let Some(text) = guids::explain(&v.vendor) {
                doc.dim(text);
            }
            match runtime::get_variable_boxed(&v.name, &VariableVendor(v.vendor)) {
                Ok((data, attrs)) => {
                    attributes(&mut doc, attrs);
                    doc.kv("Size", format!("{} bytes", data.len()));
                    decode(&mut doc, &name, &v.vendor, &data);
                    doc.h("Raw value");
                    doc.hexdump(&data, 512);
                }
                Err(e) => doc.kv_styled("GetVariable()", st::BAD, &format!("{:?}", e.status())),
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
