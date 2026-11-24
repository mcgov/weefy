//! weefy: a UEFI application that runs at boot, shows how UEFI works using
//! the machine it is running on, and then starts the regular OS.

#![no_main]
#![no_std]

extern crate alloc;

mod art;
mod bootopt;
mod bootview;
mod capsule;
mod guid_names;
mod guids;
mod handles;
mod intro;
mod learn;
mod memmap;
mod pixel;
mod secboot;
mod services;
mod systab;
mod tui;
mod util;
mod vars;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use bootview::Leave;
use core::time::Duration;
use tui::{BrowseState, Doc, Item, Key, Outcome, Screen, st};
use uefi::boot::{self, SearchType};
use uefi::prelude::*;
use uefi::proto::loaded_image::LoadedImage;
use uefi::{runtime, system};

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    tui::print_raw(&format!("\n\nweefy stopped on an internal error:\n{info}\n\nReturning to the firmware in 10 seconds.\n"));
    boot::stall(Duration::from_secs(10));
    // SAFETY: nothing of this program is used after this call.
    let _ = unsafe { boot::exit(boot::image_handle(), Status::ABORTED, 0, core::ptr::null_mut()) };
    loop {
        boot::stall(Duration::from_secs(1));
    }
}

/// The compiler turns a loop that looks for the end of a UCS-2 string into a
/// call to the C library's wcslen(), and there is no C library here.
///
/// # Safety
/// `s` must point to a NUL-terminated string of 16-bit units.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wcslen(s: *const u16) -> usize {
    let mut n = 0;
    // Volatile reads, so that this loop is not itself turned into wcslen().
    while unsafe { s.add(n).read_volatile() } != 0 {
        n += 1;
    }
    n
}

/// Settings from weefy.conf, a file of `name=value` lines beside the program.
#[derive(Default)]
struct Config {
    /// Seconds to wait before booting the default on its own. 0: wait for
    /// the user.
    timeout: u32,
    /// Part of the title or path of the target to boot by default.
    default: Option<String>,
    /// When the intro plays.
    intro: Intro,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Intro {
    /// Until the user chooses "[skip dialogue]".
    #[default]
    Auto,
    Off,
    Always,
}

fn load_config() -> Config {
    let mut config = Config::default();
    let Some(bytes) = util::read_own_file("weefy.conf") else {
        return config;
    };
    for line in String::from_utf8_lossy(&bytes).lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        match name.trim() {
            "timeout" => config.timeout = value.trim().parse().unwrap_or(0).min(3600),
            "default" => config.default = Some(value.trim().to_string()).filter(|s| !s.is_empty()),
            "intro" => {
                config.intro = match value.trim() {
                    "off" => Intro::Off,
                    "always" => Intro::Always,
                    _ => Intro::Auto,
                }
            }
            _ => {}
        }
    }
    config
}

/// Counts down to booting the default. True if the time ran out or Enter was
/// pressed; false if another key stopped it.
fn countdown(scr: &mut Screen, title: &str, seconds: u32) -> bool {
    tui::drain_keys();
    for tenths in (0..seconds * 10).rev() {
        if tenths % 10 == 9 || tenths + 1 == seconds * 10 {
            scr.clear();
            tui::title_bar(scr, "Starting");
            let w = scr.cols.min(70);
            let x = (scr.cols - w) / 2;
            let y = scr.rows / 2 - 3;
            scr.frame(x, y, w, 7, st::BORDER_FOCUS, "Booting", "");
            scr.put(x + 3, y + 2, st::BRIGHT, title, w - 6);
            scr.put(x + 3, y + 4, st::NORMAL, &format!("in {} seconds. Press any key to explore UEFI first.", tenths / 10 + 1), w - 6);
            tui::footer(scr, &[("Enter", "Boot now"), ("Any key", "Explore")]);
            scr.flush();
        }
        if let Some(key) = tui::poll_key() {
            return key == Key::Enter;
        }
        boot::stall(Duration::from_millis(100));
    }
    true
}

#[derive(Clone, Copy)]
enum Act {
    BootDefault,
    Intro,
    SkipIntro,
    Tour,
    SystemTable,
    BootServices,
    RuntimeServices,
    Handles,
    Protocols,
    Variables,
    Memory,
    Console,
    SecureBoot,
    Capsules,
    Boot,
}

fn menu_page(doc: &mut Doc, act: Act, config: &Config) {
    match act {
        Act::BootDefault => {
            let targets = bootopt::targets();
            match bootopt::default_target(&targets, config.default.as_deref()) {
                Some(t) => {
                    doc.p("Done exploring? This starts your operating system's loader, the same way the firmware's boot manager would have.");
                    doc.blank();
                    doc.kv("Will start", t.title.clone());
                    if let bootopt::Source::Entry(n) = t.source {
                        doc.kv("Boot option", format!("Boot{n:04X}"));
                    }
                    if let Some(p) = t.resolved.first() {
                        doc.kv("From", util::dp_bytes_text(p));
                    }
                    doc.blank();
                    doc.dim("Chosen as the first usable entry of BootOrder that isn't this program, or failing that the first loader found on a disk. The Boot screen lists the alternatives; weefy.conf can name another default.");
                }
                None => {
                    doc.styled(st::HEAD, "No operating system loader was found.");
                    doc.blank();
                    doc.p("No boot option other than this program resolves to a device that is present, and no file system has a loader under \\EFI. The Boot screen shows what was looked at, and lets you hand control back to the firmware.");
                }
            }
        }
        Act::Intro => {
            doc.p("Dr. Weefy Fulgens welcomes you to his lab: a logo screen and a few lines of dialogue, in which he checks whether you brought an operating system.");
            doc.blank();
            doc.p("It is drawn straight into the firmware's framebuffer with the Graphics Output Protocol, 320 by 200 in 16 colours, scaled up to fit. On a serial console the same lines come as text.");
            doc.blank();
            doc.dim("Any key moves it on; Esc leaves it.");
        }
        Act::SkipIntro => {
            doc.p("Whether the intro plays when weefy starts. Enter changes it.");
            doc.blank();
            let set = intro::skip_is_set();
            doc.kv("At the start", match (config.intro, set) {
                (Intro::Off, _) => "the intro doesn't play: weefy.conf says intro=off",
                (Intro::Always, _) => "the intro plays: weefy.conf says intro=always",
                (Intro::Auto, true) => "the intro doesn't play",
                (Intro::Auto, false) => "the intro plays",
            });
            doc.blank();
            doc.p("The choice is kept in a UEFI variable, the firmware's own settings store, so it lasts across restarts and power-offs. The Variables screen shows it once it is there.");
            doc.kv("Variable", intro::SKIP_VAR);
            doc.kv("Namespace", format!("{}", intro::WEEFY_VARS));
            doc.kv("Now", if set { "1: skip" } else { "absent: play" });
            doc.blank();
            doc.dim("Some firmware forgets its variables at power-off. There, put intro=off in weefy.conf, beside the program.");
        }
        Act::Tour => {
            doc.p("Thirteen short lessons on how UEFI works: what the firmware does between power-on and your OS, and the handful of ideas (tables, handles, protocols, variables, device paths) everything is built from.");
            doc.blank();
            doc.p("Each lesson ends with facts read from this machine and a pointer to the screen that shows the real thing.");
            doc.blank();
            doc.dim("New to UEFI? Start here.");
        }
        Act::SystemTable => {
            doc.p("The root of everything: the one pointer the firmware hands to each program it starts. It leads to the console, the two service tables and the configuration tables (ACPI, SMBIOS, device tree).");
            doc.blank();
            doc.kv("Firmware", String::from(system::firmware_vendor()));
            doc.kv("Revision", format!("{:#x}", system::firmware_revision()));
            doc.kv("UEFI version", format!("{}", system::uefi_revision()));
            doc.kv("Config tables", format!("{}", system::with_config_table(|t| t.len())));
        }
        Act::BootServices => {
            doc.p("The functions the firmware offers while it is in charge: memory allocation, events and timers, the handle database, loading and starting images.");
            doc.blank();
            doc.p("Each one is listed with what it does, the address this firmware put in the table for it, and which loaded image that address is inside.");
        }
        Act::RuntimeServices => {
            doc.p("The few functions that stay available to the running OS: variables, the clock, reset, firmware update capsules.");
            doc.blank();
            match runtime::get_time() {
                Ok(t) => doc.kv("GetTime() says", format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.year(), t.month(), t.day(), t.hour(), t.minute(), t.second())),
                Err(e) => doc.kv("GetTime() says", format!("{:?}", e.status())),
            }
        }
        Act::Handles => {
            doc.p("The handle database: every device, driver, image and service the firmware has created, each with the protocols installed on it.");
            doc.blank();
            if let Ok(all) = boot::locate_handle_buffer(SearchType::AllHandles) {
                doc.kv("Handles", format!("{}", all.len()));
            }
            doc.dim("Press / in the list to find a handle by what it is.");
        }
        Act::Protocols => {
            doc.p("The same database turned around: every protocol GUID in use, what it is for, and which handles carry it.");
            doc.blank();
            doc.dim("GUIDs are named from edk2's public declarations; the ones a firmware vendor keeps private stay as numbers.");
        }
        Act::Variables => {
            doc.p("The firmware's settings store: BootOrder and the boot options, the Secure Boot keys, the console devices, and whatever the firmware vendor and your OS have left there.");
            doc.blank();
            doc.kv("Variables", format!("{}", runtime::variable_keys().filter(|k| k.is_ok()).count()));
            doc.dim("Shown read-only, with the standard ones decoded.");
        }
        Act::Memory => {
            doc.p("Every range of physical memory and what it is used for, as GetMemoryMap() reports it: the list an OS builds its memory management from.");
        }
        Act::Console => {
            doc.p("The text modes of the console this program is drawing on, and the framebuffer behind it.");
            doc.blank();
            let (modes, current) = tui::text_modes();
            if let Some(m) = modes.iter().find(|m| m.index == current) {
                doc.kv("Current mode", format!("{} x {} characters", m.cols, m.rows));
            }
            doc.dim("If the text is small or the screen feels cramped, switch modes here.");
        }
        Act::SecureBoot => {
            doc.p("Secure Boot from both sides: the keys this firmware holds, how this program is signed, and what LoadImage() says to a signed, an unsigned and a tampered image.");
            doc.blank();
            doc.p("If the disk was made with make-image.sh --secure-boot and the machine is in Setup Mode, it can also enrol the keys this program was signed with, which turns Secure Boot on under your own key.");
            doc.blank();
            doc.kv("SetupMode", flag_text("SetupMode"));
            doc.kv("SecureBoot", flag_text("SecureBoot"));
        }
        Act::Capsules => {
            doc.p("How firmware gets updated: the capsule services, the table of updatable components (ESRT), the Firmware Management Protocol behind it, and the result variables the firmware leaves afterwards.");
            doc.blank();
            doc.p("It also reads capsule files found on this disk and shows their structure without submitting them, and can send one harmless test capsule to watch it cross a reset.");
            doc.blank();
            doc.kv("Components", capsule::summary());
        }
        Act::Boot => {
            doc.p("Everything that can be started: the firmware's boot options in BootOrder's order, and loaders found by searching the disks. Each shows the device path it stores and what that resolves to on this boot.");
            doc.blank();
            doc.p("Also the ways out: hand control back to the firmware's boot manager, restart, or power off.");
        }
    }
}

fn flag_text(name: &str) -> &'static str {
    match bootopt::global_var(name).and_then(|(d, _)| d.first().copied()) {
        Some(1) => "1",
        Some(0) => "0",
        Some(_) => "unexpected",
        None => "absent: no Secure Boot in this firmware",
    }
}

/// This copy was started by another weefy, from the Secure Boot lab.
fn is_stage2() -> bool {
    let wanted: Vec<u8> = secboot::STAGE2_OPTIONS.encode_utf16().chain(core::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect();
    util::open::<LoadedImage>(boot::image_handle()).is_some_and(|image| image.load_options_as_bytes() == Some(&wanted[..]))
}

#[entry]
fn main() -> Status {
    // The boot manager armed a five-minute watchdog before starting this
    // program. Reading takes longer than that.
    let _ = boot::set_watchdog_timer(0, 0x10000, None);
    let stage2 = is_stage2();
    tui::STAGE2.store(stage2, core::sync::atomic::Ordering::Relaxed);
    // Stage two is for looking around, not for booting on a timer.
    let config = if stage2 { Config::default() } else { load_config() };
    let mut scr = Screen::new();

    if config.timeout > 0 {
        let targets = bootopt::targets();
        if let Some(t) = bootopt::default_target(&targets, config.default.as_deref()) {
            if countdown(&mut scr, &t.title, config.timeout) {
                bootview::start(&mut scr, t);
            }
        }
    }

    // The first time, and until asked not to: the intro.
    let wanted = match config.intro {
        Intro::Off => false,
        Intro::Always => true,
        Intro::Auto => !intro::skip_is_set(),
    };
    if wanted && !stage2 {
        intro::play(&mut scr, config.default.as_deref());
    }

    let boot_label = match bootview::default_title(config.default.as_deref()) {
        Some(title) => format!("Boot {title}"),
        None => String::from("Boot the OS (none found)"),
    };
    let mut state = BrowseState::default();
    loop {
        let menu: [(Option<Act>, String); 19] = [
            (None, "Start here".into()),
            (Some(Act::Tour), "Take the tour".into()),
            (Some(Act::Intro), "Meet Dr. Weefy".into()),
            (Some(Act::SkipIntro), format!("[skip dialogue] {}", if intro::skip_is_set() { "yes" } else { "no" })),
            (None, "This machine".into()),
            (Some(Act::SystemTable), "System table".into()),
            (Some(Act::BootServices), "Boot services".into()),
            (Some(Act::RuntimeServices), "Runtime services".into()),
            (Some(Act::Handles), "Handles".into()),
            (Some(Act::Protocols), "Protocols".into()),
            (Some(Act::Variables), "Variables".into()),
            (Some(Act::Memory), "Memory map".into()),
            (Some(Act::Console), "Console".into()),
            (None, "Try it".into()),
            (Some(Act::SecureBoot), "Secure Boot lab".into()),
            (Some(Act::Capsules), "Capsules and firmware updates".into()),
            (None, "When you are done".into()),
            (Some(Act::BootDefault), boot_label.clone()),
            (Some(Act::Boot), "Boot something else, or leave".into()),
        ];
        let items: Vec<Item> = menu
            .iter()
            .map(|(act, label)| match act {
                None => Item::header(label.clone()),
                Some(Act::BootDefault) => Item::new(label.clone()).styled(st::GOOD),
                Some(_) => Item::new(label.clone()),
            })
            .collect();
        let hints: &[(&str, &str)] = if stage2 { &[] } else { &[("b", "Boot OS")] };
        let crumbs = if stage2 { "Home of stage two (Esc returns to stage one)" } else { "Home" };
        let out = tui::browse(&mut scr, &mut state, crumbs, "weefy", &items, hints, &mut |row, width| {
            let mut doc = Doc::new(width);
            if let Some(act) = menu[row].0 {
                menu_page(&mut doc, act, &config);
            }
            doc
        });
        let act = match out {
            // Stage two returns to the weefy that started it.
            Outcome::Back if stage2 => {
                scr.release();
                return Status::SUCCESS;
            }
            // Otherwise there is nowhere further back than the main menu.
            Outcome::Back => continue,
            Outcome::Open(row) => menu[row].0,
            Outcome::Char('b') => Some(Act::BootDefault),
            Outcome::Char(_) => None,
        };
        match act {
            Some(Act::Tour) => learn::run(&mut scr),
            Some(Act::Intro) => intro::play(&mut scr, config.default.as_deref()),
            Some(Act::SkipIntro) => {
                if let Err(e) = intro::set_skip(!intro::skip_is_set()) {
                    tui::notice(
                        &mut scr,
                        "The firmware wouldn't keep it",
                        &[
                            (st::BAD, format!("SetVariable() returned {:?}.", e.status())),
                            (st::NORMAL, String::new()),
                            (st::NORMAL, String::from("This firmware can't store the choice. Put intro=off in weefy.conf, beside the program, to do the same.")),
                        ],
                    );
                }
            }
            Some(Act::SystemTable) => systab::run(&mut scr),
            Some(Act::BootServices) => services::run_boot(&mut scr),
            Some(Act::RuntimeServices) => services::run_runtime(&mut scr),
            Some(Act::Handles) => handles::run_handles(&mut scr),
            Some(Act::Protocols) => handles::run_protocols(&mut scr),
            Some(Act::Variables) => vars::run(&mut scr),
            Some(Act::Memory) => memmap::run(&mut scr),
            Some(Act::Console) => memmap::run_console(&mut scr),
            Some(Act::SecureBoot) => secboot::run(&mut scr),
            Some(Act::Capsules) => capsule::run(&mut scr),
            Some(Act::BootDefault) => {
                let targets = bootopt::targets();
                match bootopt::default_target(&targets, config.default.as_deref()) {
                    Some(t) => bootview::start(&mut scr, t),
                    None => {
                        if let Leave::Exit(status) = bootview::run(&mut scr, config.default.as_deref()) {
                            scr.release();
                            return status;
                        }
                    }
                }
            }
            Some(Act::Boot) => {
                if let Leave::Exit(status) = bootview::run(&mut scr, config.default.as_deref()) {
                    scr.release();
                    return status;
                }
            }
            None => {}
        }
    }
}
