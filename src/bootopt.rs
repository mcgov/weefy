//! Finding what can be booted, and booting it.
//!
//! This is the boot manager's job done by hand: read BootOrder and the
//! Boot#### variables, turn each stored device path into one that names a
//! file on a device that is present, LoadImage() it and StartImage() it.

use crate::tui::{self, Screen};
use crate::util::{self, DP_END, dp_body, dp_nodes};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use uefi::boot::{self, LoadImageSource, SearchType};
use uefi::proto::BootPolicy;
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::file::{Directory, File, FileAttribute, FileMode, FileSystemVolumeLabel};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::runtime::{self, VariableAttributes, VariableVendor};
use uefi::{Handle, Identify, Status};

/// The suffix of the removable-media loader's name on this architecture:
/// \EFI\BOOT\BOOT<this>.EFI.
pub const ARCH: &str = if cfg!(target_arch = "aarch64") {
    "AA64"
} else if cfg!(target_arch = "x86_64") {
    "X64"
} else if cfg!(target_arch = "riscv64") {
    "RISCV64"
} else {
    "IA32"
};

pub fn default_loader() -> String {
    format!("\\EFI\\BOOT\\BOOT{ARCH}.EFI")
}

pub const LOAD_OPTION_ACTIVE: u32 = 0x1;
pub const LOAD_OPTION_HIDDEN: u32 = 0x8;
pub const LOAD_OPTION_CATEGORY: u32 = 0x1f00;

/// An EFI_LOAD_OPTION: what a Boot#### variable holds.
pub struct LoadOption {
    pub attributes: u32,
    pub description: String,
    /// The device path list. The first path is the one to boot.
    pub path: Vec<u8>,
    /// Bytes handed to the image as its load options (a command line, often).
    pub optional: Vec<u8>,
}

pub fn parse_load_option(data: &[u8]) -> Option<LoadOption> {
    let attributes = util::rd32(data, 0)?;
    let path_len = util::rd16(data, 4)? as usize;
    let (description, desc_len) = util::ucs2_bytes(data.get(6..)?);
    let path_at = 6 + desc_len;
    let path = data.get(path_at..path_at + path_len)?.to_vec();
    let optional = data[path_at + path_len..].to_vec();
    Some(LoadOption {
        attributes,
        description,
        path,
        optional,
    })
}

/// A variable in the UEFI specification's own namespace.
pub fn global_var(name: &str) -> Option<(Vec<u8>, VariableAttributes)> {
    runtime::get_variable_boxed(&util::cstring16(name), &VariableVendor::GLOBAL_VARIABLE)
        .ok()
        .map(|(data, attrs)| (data.into_vec(), attrs))
}

pub fn global_u16(name: &str) -> Option<u16> {
    util::rd16(&global_var(name)?.0, 0)
}

pub fn boot_order() -> Vec<u16> {
    global_var("BootOrder")
        .map(|(d, _)| d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
        .unwrap_or_default()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The Boot#### variable with this number.
    Entry(u16),
    /// A loader found by looking through the file systems.
    File,
}

/// Something that can be started.
pub struct Target {
    pub title: String,
    pub source: Source,
    pub attributes: u32,
    /// The device path as stored in the boot option (or as found).
    pub stored: Vec<u8>,
    pub optional: Vec<u8>,
    /// Full device paths to try, in order.
    pub resolved: Vec<Vec<u8>>,
    /// It is this program.
    pub is_self: bool,
}

impl Target {
    /// A boot option the firmware would boot on its own: active, and an OS
    /// rather than a firmware application.
    pub fn is_regular(&self) -> bool {
        match self.source {
            Source::Entry(_) => {
                self.attributes & LOAD_OPTION_ACTIVE != 0
                    && self.attributes & (LOAD_OPTION_HIDDEN | LOAD_OPTION_CATEGORY) == 0
            }
            Source::File => true,
        }
    }

    pub fn can_start(&self) -> bool {
        !self.is_self && !self.resolved.is_empty()
    }
}

fn fs_handles() -> Vec<(Handle, Vec<u8>)> {
    boot::locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))
        .map(|hs| hs.iter().filter_map(|&h| Some((h, util::handle_dp(h)?))).collect())
        .unwrap_or_default()
}

fn join(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(p);
    }
    out.extend_from_slice(&DP_END);
    out
}

/// Two hard drive nodes name the same partition: same number, same signature.
fn same_partition(a: &[u8], b: &[u8]) -> bool {
    a.len() >= 42 && b.len() >= 42 && a[4..8] == b[4..8] && a[24..40] == b[24..40] && a[41] == b[41]
}

/// Turns a boot option's device path into full paths that LoadImage() can
/// use on this boot.
///
/// A boot option rarely stores the whole path. An installed OS stores a
/// short form that starts at the partition, HD(1,GPT,<guid>)/\EFI\..., so
/// that it still works when the disk moves to another port: the partition is
/// found by its signature. A removable device stores no file at all, and the
/// default loader's name is implied.
pub fn resolve(stored: &[u8]) -> Vec<Vec<u8>> {
    let nodes = dp_nodes(stored);
    let body = dp_body(stored);
    let has_file = nodes.iter().any(|n| n.0 == 4 && n.1 == 4);
    let default_file = util::dp_file_node(&default_loader());
    let mut out = Vec::new();
    match nodes.first() {
        None => {}
        // Short form, from the partition on.
        Some(&(4, 1, hd)) => {
            let rest = &body[hd.len()..];
            for (_, dp) in fs_handles() {
                if dp_nodes(&dp).iter().any(|n| n.0 == 4 && n.1 == 1 && same_partition(n.2, hd)) {
                    if has_file {
                        out.push(join(&[dp_body(&dp), rest]));
                    } else {
                        out.push(join(&[dp_body(&dp), &default_file]));
                    }
                }
            }
        }
        // Short form, only a file name: any file system that has it.
        Some(&(4, 4, _)) => {
            for (_, dp) in fs_handles() {
                out.push(join(&[dp_body(&dp), body]));
            }
        }
        _ if has_file => out.push(join(&[body])),
        // A device: the default loader on each file system under it. With no
        // file system under it, the device itself: a network card or a
        // firmware volume file loads without one.
        _ => {
            for (_, dp) in fs_handles() {
                if dp_body(&dp).starts_with(body) {
                    out.push(join(&[dp_body(&dp), &default_file]));
                }
            }
            if out.is_empty() {
                out.push(join(&[body]));
            }
        }
    }
    out
}

/// This program's own full device path as lower-case text, to recognize
/// boot options that would start it again.
fn own_path() -> Option<String> {
    let image = util::open::<LoadedImage>(boot::image_handle())?;
    let device = util::handle_dp(image.device()?)?;
    let file = image.file_path()?.as_bytes().to_vec();
    Some(util::dp_bytes_text(&join(&[dp_body(&device), dp_body(&file)])).to_lowercase())
}

fn is_own(path: &[u8], own: &Option<String>) -> bool {
    match own {
        Some(own) => util::dp_bytes_text(path).to_lowercase() == *own,
        None => false,
    }
}

fn open_dir(dir: &mut Directory, name: &str) -> Option<Directory> {
    dir.open(&util::cstring16(name), FileMode::Read, FileAttribute::empty())
        .ok()?
        .into_directory()
}

fn list_dir(dir: &mut Directory) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let _ = dir.reset_entry_readout();
    while let Ok(Some(info)) = dir.read_entry_boxed() {
        let name = String::from(info.file_name());
        if name != "." && name != ".." {
            out.push((name, info.is_directory()));
        }
        if out.len() >= 64 {
            break;
        }
    }
    out
}

/// The label of the file system on a handle, or a stand-in.
pub fn volume_label(handle: Handle) -> Option<String> {
    let mut fs = util::open::<SimpleFileSystem>(handle)?;
    let mut root = fs.open_volume().ok()?;
    let info = root.get_boxed_info::<FileSystemVolumeLabel>().ok()?;
    let label = String::from(info.volume_label());
    (!label.trim().is_empty()).then_some(label)
}

/// Looks through every file system for OS loaders: the main loader in each
/// directory under \EFI, and the removable-media default.
fn scan_files() -> Vec<(String, Vec<u8>)> {
    let arch = ARCH.to_lowercase();
    // The loader a vendor directory is entered through, most likely first.
    let preferred = [
        format!("shim{arch}.efi"),
        format!("systemd-boot{arch}.efi"),
        format!("grub{arch}.efi"),
        String::from("bootmgfw.efi"),
        format!("boot{arch}.efi"),
        format!("refind_{arch}.efi"),
    ];
    let mut found = Vec::new();
    for (handle, dp) in fs_handles() {
        let Some(mut fs) = util::open::<SimpleFileSystem>(handle) else {
            continue;
        };
        let Ok(mut root) = fs.open_volume() else {
            continue;
        };
        let label = volume_label(handle).unwrap_or_else(|| String::from("unlabelled volume"));
        let Some(mut efi) = open_dir(&mut root, "EFI") else {
            continue;
        };
        for (dir_name, is_dir) in list_dir(&mut efi) {
            if !is_dir {
                continue;
            }
            let Some(mut dir) = open_dir(&mut efi, &dir_name) else {
                continue;
            };
            let files: Vec<String> = list_dir(&mut dir)
                .into_iter()
                .filter(|(n, d)| !d && n.to_lowercase().ends_with(".efi"))
                .map(|(n, _)| n)
                .collect();
            let main = preferred
                .iter()
                .find_map(|p| files.iter().find(|f| f.to_lowercase() == *p));
            let chosen: Vec<&String> = match main {
                Some(f) => alloc::vec![f],
                None => files.iter().take(4).collect(),
            };
            for file in chosen {
                let path = format!("\\EFI\\{dir_name}\\{file}");
                let full = join(&[dp_body(&dp), &util::dp_file_node(&path)]);
                found.push((format!("{path} on {label}"), full));
            }
        }
    }
    found
}

/// Everything that can be started: the boot options in BootOrder's order,
/// then loaders found on disk that no boot option already names.
pub fn targets() -> Vec<Target> {
    let own = own_path();
    let mut out: Vec<Target> = Vec::new();
    for number in boot_order() {
        let Some((data, _)) = global_var(&format!("Boot{number:04X}")) else {
            continue;
        };
        let Some(opt) = parse_load_option(&data) else {
            continue;
        };
        let resolved = resolve(&opt.path);
        let is_self = resolved.iter().any(|p| is_own(p, &own));
        out.push(Target {
            title: opt.description,
            source: Source::Entry(number),
            attributes: opt.attributes,
            stored: opt.path,
            optional: opt.optional,
            resolved,
            is_self,
        });
    }
    for (title, full) in scan_files() {
        let text = util::dp_bytes_text(&full).to_lowercase();
        let known = out
            .iter()
            .any(|t| t.resolved.iter().any(|p| util::dp_bytes_text(p).to_lowercase() == text));
        let is_self = is_own(&full, &own);
        if known && !is_self {
            continue;
        }
        if is_self && out.iter().any(|t| t.is_self) {
            continue;
        }
        out.push(Target {
            title,
            source: Source::File,
            attributes: LOAD_OPTION_ACTIVE,
            stored: full.clone(),
            optional: Vec::new(),
            resolved: alloc::vec![full],
            is_self,
        });
    }
    out
}

/// The target to boot when the user just asks for their OS: the one named by
/// `prefer` (a part of its title or path) if given, else the first regular
/// boot option that isn't this program, else the first loader found on disk.
pub fn default_target<'a>(targets: &'a [Target], prefer: Option<&str>) -> Option<&'a Target> {
    if let Some(want) = prefer.map(|s| s.to_lowercase()).filter(|s| !s.is_empty()) {
        let hit = targets.iter().find(|t| {
            t.can_start()
                && (t.title.to_lowercase().contains(&want)
                    || util::dp_bytes_text(&t.stored).to_lowercase().contains(&want)
                    || matches!(t.source, Source::Entry(n) if format!("boot{n:04x}") == want))
        });
        if hit.is_some() {
            return hit;
        }
    }
    targets.iter().find(|t| t.can_start() && t.is_regular())
}

/// Loads and starts a target. Returns the status the image exited with if it
/// came back, or why it couldn't be started.
pub fn launch(scr: &mut Screen, target: &Target) -> Result<Status, String> {
    let own = own_path();
    let mut last_error = String::from("no device on this machine matches its device path");
    for path in &target.resolved {
        if is_own(path, &own) {
            continue;
        }
        let Ok(dp) = <&DevicePath>::try_from(&path[..]) else {
            continue;
        };
        let image = match boot::load_image(
            boot::image_handle(),
            LoadImageSource::FromDevicePath {
                device_path: dp,
                boot_policy: BootPolicy::BootSelection,
            },
        ) {
            Ok(image) => image,
            Err(e) => {
                last_error = match e.status() {
                    Status::SECURITY_VIOLATION | Status::ACCESS_DENIED => {
                        String::from("Secure Boot refused the image: its signature is not in the allowed database (db), or is revoked (dbx)")
                    }
                    Status::NOT_FOUND => String::from("LoadImage() found no file at its device path"),
                    Status::UNSUPPORTED | Status::LOAD_ERROR => {
                        String::from("LoadImage() rejected the file: not a UEFI image for this CPU")
                    }
                    s => format!("LoadImage() returned {s:?}"),
                };
                continue;
            }
        };

        // The optional data becomes the image's load options. It must stay
        // where it is for as long as the image runs.
        let options = target.optional.clone().leak();
        if !options.is_empty() {
            if let Some(mut loaded) = util::open::<LoadedImage>(image) {
                // SAFETY: the buffer is leaked, so it outlives the image.
                unsafe { loaded.set_load_options(options.as_ptr(), options.len() as u32) };
            }
        }

        scr.release();
        tui::print_raw(&format!("weefy: starting {}\n", target.title));
        if let Source::Entry(number) = target.source {
            // What a boot manager does: record which option is being booted.
            let _ = runtime::set_variable(
                &util::cstring16("BootCurrent"),
                &VariableVendor::GLOBAL_VARIABLE,
                VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS,
                &number.to_le_bytes(),
            );
        }
        // The specification's five minutes for a boot option to get going:
        // if it hangs before ExitBootServices(), the machine resets.
        let _ = boot::set_watchdog_timer(300, 0x10000, None);
        let result = boot::start_image(image);
        let _ = boot::set_watchdog_timer(0, 0x10000, None);
        tui::drain_keys();
        scr.reset();
        return Ok(match result {
            Ok(()) => Status::SUCCESS,
            Err(e) => e.status(),
        });
    }
    Err(last_error)
}

/// A line that says what a device path's first node is, for the boot screen.
pub fn path_kind(stored: &[u8]) -> &'static str {
    let nodes = dp_nodes(stored);
    let has_file = nodes.iter().any(|n| n.0 == 4 && n.1 == 4);
    match nodes.first() {
        Some(&(4, 1, _)) => "Short form: it starts at a partition, found by its signature on whatever port the disk is on.",
        Some(&(4, 4, _)) => "Short form: only a file name, looked for on every file system.",
        Some(&(4, 6, _)) | Some(&(4, 7, _)) => "A file inside the firmware image itself: a built-in application.",
        Some(&(3, 0x18, _)) => "A URI: HTTP boot.",
        Some(_) if has_file => "A full path: from the root of the machine down to a file.",
        Some(_) => "A device with no file named: the firmware looks for the default loader on it, or asks the device itself (network boot).",
        None => "Empty.",
    }
}

/// Text for a status an image exited with.
pub fn status_text(status: Status) -> String {
    format!("{status:?}").to_string()
}
