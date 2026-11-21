//! The Boot screen: what can be started, and the ways to leave.

use crate::bootopt::{self, Source, Target};
use crate::tui::{self, BrowseState, Doc, Item, Key, Outcome, Screen, st};
use crate::util;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use uefi::runtime::{self, ResetType, VariableAttributes, VariableVendor};
use uefi::Status;

/// How the program ends, if it does.
pub enum Leave {
    /// Back to the menu.
    Stay,
    /// Return this status to the firmware's boot manager.
    Exit(Status),
}

enum Row {
    Target(usize),
    NextOption,
    FirmwareMenu,
    Setup,
    Restart,
    PowerOff,
}

fn firmware_ui_supported() -> bool {
    bootopt::global_var("OsIndicationsSupported")
        .and_then(|(d, _)| util::rd64(&d, 0))
        .is_some_and(|v| v & 1 != 0)
}

fn confirm(scr: &mut Screen, title: &str, text: &str) -> bool {
    let key = tui::message(
        scr,
        title,
        &|w| {
            let mut d = Doc::new(w);
            d.p(text);
            d
        },
        &[("Enter", "Yes"), ("Esc", "No")],
    );
    key == Key::Enter
}

/// Starts a target and reports what happened if it came back.
pub fn start(scr: &mut Screen, target: &Target) {
    match bootopt::launch(scr, target) {
        Ok(status) => {
            let title = target.title.clone();
            tui::message(
                scr,
                "It came back",
                &|w| {
                    let mut d = Doc::new(w);
                    d.p(&format!("{title} returned control with status {}.", bootopt::status_text(status)));
                    d.blank();
                    d.p("An OS loader that boots never returns: it calls ExitBootServices() and the firmware's part is over. A program that exits lands back in its caller's StartImage(), which here was this program's.");
                    d
                },
                &[("Any key", "Continue")],
            );
        }
        Err(why) => {
            let title = target.title.clone();
            tui::message(
                scr,
                "Could not start it",
                &|w| {
                    let mut d = Doc::new(w);
                    d.p(&format!("{title}:"));
                    d.styled(st::BAD, &why);
                    d
                },
                &[("Any key", "Continue")],
            );
        }
    }
}

fn target_page(doc: &mut Doc, t: &Target, is_default: bool) {
    match t.source {
        Source::Entry(n) => {
            doc.p(&format!("A boot option: the variable Boot{n:04X}, listed in BootOrder. This is what the firmware's own boot manager would start."));
            doc.blank();
            doc.kv("Variable", format!("Boot{n:04X}"));
            doc.kv("Description", t.title.clone());
            let mut flags = Vec::new();
            flags.push(if t.attributes & bootopt::LOAD_OPTION_ACTIVE != 0 { "active" } else { "inactive" });
            if t.attributes & bootopt::LOAD_OPTION_HIDDEN != 0 {
                flags.push("hidden");
            }
            if t.attributes & bootopt::LOAD_OPTION_CATEGORY != 0 {
                flags.push("application");
            }
            doc.kv("Attributes", flags.join(", "));
            if bootopt::global_u16("BootCurrent") == Some(n) {
                doc.kv_styled("BootCurrent", st::GOOD, "this is the option the firmware booted to get here");
            }
        }
        Source::File => {
            doc.p("A loader found by looking through the file systems. No boot option names it; that is normal for removable media, and for an OS whose boot option was lost with the firmware's settings.");
            doc.blank();
        }
    }
    doc.kv("Stored path", util::dp_bytes_text(&t.stored));
    doc.dim(bootopt::path_kind(&t.stored));
    if t.resolved.is_empty() {
        doc.kv_styled("Resolves to", st::BAD, "nothing: no device present matches it");
    }
    for (i, p) in t.resolved.iter().enumerate() {
        doc.kv(if i == 0 { "Resolves to" } else { "or" }, util::dp_bytes_text(p));
    }
    if !t.optional.is_empty() {
        doc.kv("Optional data", format!("{} bytes, passed as load options", t.optional.len()));
    }
    doc.blank();
    if t.is_self {
        doc.styled(st::HEAD, "This is the program you are using. Starting it again would only nest another copy.");
    } else if t.can_start() {
        if is_default {
            doc.styled(st::GOOD, "This is what \"Boot the OS\" on the main menu starts.");
        }
        doc.p("Enter loads it with LoadImage(), arms the five-minute watchdog and calls StartImage().");
    }
}

pub fn run(scr: &mut Screen, prefer: Option<&str>) -> Leave {
    let mut state = BrowseState::default();
    loop {
        let targets = bootopt::targets();
        let default = bootopt::default_target(&targets, prefer).map(|d| d as *const Target);
        let mut items = Vec::new();
        let mut rows: Vec<Option<Row>> = Vec::new();
        let section = |items: &mut Vec<Item>, rows: &mut Vec<Option<Row>>, title: &str, source_is_entry: bool| {
            let mut first = true;
            for (i, t) in targets.iter().enumerate() {
                if matches!(t.source, Source::Entry(_)) != source_is_entry {
                    continue;
                }
                if first {
                    items.push(Item::header(title));
                    rows.push(None);
                    first = false;
                }
                let style = if t.is_self {
                    st::DIM
                } else if Some(t as *const Target) == default {
                    st::GOOD
                } else if !t.can_start() {
                    st::DIM
                } else {
                    st::NORMAL
                };
                let label = match t.source {
                    Source::Entry(n) => format!("{n:04X} {}", t.title),
                    Source::File => t.title.clone(),
                };
                items.push(Item::new(label).styled(style));
                rows.push(Some(Row::Target(i)));
            }
        };
        section(&mut items, &mut rows, "Boot options, in BootOrder", true);
        section(&mut items, &mut rows, "Loaders found on disk", false);
        items.push(Item::header("Leave"));
        rows.push(None);
        items.push(Item::new("Continue the boot order"));
        rows.push(Some(Row::NextOption));
        items.push(Item::new("Firmware's own menu"));
        rows.push(Some(Row::FirmwareMenu));
        if firmware_ui_supported() {
            items.push(Item::new("Restart into firmware setup"));
            rows.push(Some(Row::Setup));
        }
        items.push(Item::new("Restart"));
        rows.push(Some(Row::Restart));
        items.push(Item::new("Power off"));
        rows.push(Some(Row::PowerOff));

        let out = tui::browse(scr, &mut state, "Boot", "Boot", &items, &[], &mut |row, width| {
            let mut doc = Doc::new(width);
            match &rows[row] {
                Some(Row::Target(i)) => target_page(&mut doc, &targets[*i], Some(&targets[*i] as *const Target) == default),
                Some(Row::NextOption) => {
                    doc.p("Exits this program with an error status. To the firmware's boot manager that means this boot option failed, and it goes on to the next one in BootOrder, as if this program had not been there.");
                    doc.blank();
                    doc.kv("Exit status", "EFI_ABORTED");
                    let order = bootopt::boot_order();
                    let current = bootopt::global_u16("BootCurrent");
                    if let Some(pos) = current.and_then(|c| order.iter().position(|&n| n == c)) {
                        match order.get(pos + 1) {
                            Some(&next) => {
                                let title = bootopt::global_var(&format!("Boot{next:04X}"))
                                    .and_then(|(d, _)| bootopt::parse_load_option(&d))
                                    .map(|o| o.description)
                                    .unwrap_or_default();
                                doc.kv("Next in order", format!("Boot{next:04X}  {title}"));
                            }
                            None => doc.kv("Next in order", "nothing: this is the last option. The firmware will fall back to its own menu or default search."),
                        }
                    }
                }
                Some(Row::FirmwareMenu) => {
                    doc.p("Exits this program with a success status. The specification lets a boot manager treat a boot option that returns success as a request to stop and show its own menu; edk2-based firmware does.");
                    doc.blank();
                    doc.kv("Exit status", "EFI_SUCCESS");
                }
                Some(Row::Setup) => {
                    doc.p("Sets bit 0 of the OsIndications variable and restarts. The firmware sees the request on the way up and opens its setup screen instead of booting. An OS does the same when you choose \"restart into firmware settings\".");
                    doc.blank();
                    doc.styled(st::HEAD, "This writes a variable: the only one this program changes besides BootCurrent.");
                }
                Some(Row::Restart) => {
                    doc.p("Calls the runtime service ResetSystem() with EfiResetCold: a full restart, as if the power had been cycled.");
                }
                Some(Row::PowerOff) => {
                    doc.p("Calls the runtime service ResetSystem() with EfiResetShutdown.");
                }
                None => {}
            }
            doc
        });
        let row = match out {
            Outcome::Back => return Leave::Stay,
            Outcome::Open(row) => row,
            Outcome::Char(_) => continue,
        };
        match &rows[row] {
            Some(Row::Target(i)) => {
                let t = &targets[*i];
                if t.can_start() {
                    start(scr, t);
                } else {
                    state.zoom = true;
                }
            }
            Some(Row::NextOption) => return Leave::Exit(Status::ABORTED),
            Some(Row::FirmwareMenu) => return Leave::Exit(Status::SUCCESS),
            Some(Row::Setup) => {
                if confirm(scr, "Restart into firmware setup", "Set OsIndications bit 0 and restart the machine now?") {
                    let attrs = VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;
                    let current = bootopt::global_var("OsIndications").and_then(|(d, _)| util::rd64(&d, 0)).unwrap_or(0);
                    let result = runtime::set_variable(&util::cstring16("OsIndications"), &VariableVendor::GLOBAL_VARIABLE, attrs, &(current | 1).to_le_bytes());
                    match result {
                        Ok(()) => {
                            scr.release();
                            runtime::reset(ResetType::COLD, Status::SUCCESS, None);
                        }
                        Err(e) => {
                            let why = format!("SetVariable() returned {:?}.", e.status());
                            tui::message(scr, "Could not set OsIndications", &|w| {
                                let mut d = Doc::new(w);
                                d.styled(st::BAD, &why);
                                d
                            }, &[("Any key", "Continue")]);
                        }
                    }
                }
            }
            Some(Row::Restart) => {
                if confirm(scr, "Restart", "Restart the machine now?") {
                    scr.release();
                    runtime::reset(ResetType::COLD, Status::SUCCESS, None);
                }
            }
            Some(Row::PowerOff) => {
                if confirm(scr, "Power off", "Power the machine off now?") {
                    scr.release();
                    runtime::reset(ResetType::SHUTDOWN, Status::SUCCESS, None);
                }
            }
            None => {}
        }
    }
}

/// The title of the default target, for the main menu.
pub fn default_title(prefer: Option<&str>) -> Option<String> {
    let targets = bootopt::targets();
    bootopt::default_target(&targets, prefer).map(|t| t.title.clone())
}
