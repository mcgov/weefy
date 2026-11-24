//! The intro: Dr. Weefy Fulgens welcomes you to his lab. A logo screen, then
//! a short dialogue that checks whether there is an operating system to
//! boot. It plays on the first run; "[skip dialogue]" on the main menu
//! stores a variable that keeps it from playing again.
//!
//! On a console with a display it is a cartoon, drawn by `art` and shown
//! through `pixel`. On a serial console the same lines are message boxes.

use crate::art::{self, Mood, Wall};
use crate::bootopt::{self, Source};
use crate::pixel::{Canvas, Display};
use crate::tui::{self, Doc, Key, Screen, st};
use crate::util;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::time::Duration;
use uefi::runtime::{self, VariableAttributes, VariableVendor};
use uefi::{Guid, Status, boot, guid};

/// The namespace of this program's own variables.
pub const WEEFY_VARS: Guid = guid!("041ed577-1392-406e-9c7f-ffd38fc9f11a");
/// The variable that says the intro has been seen and isn't wanted again:
/// one byte, 1.
pub const SKIP_VAR: &str = "SkipIntro";

/// How long one pass of the drawing loop waits, and how many passes make an
/// animation frame: about four frames a second.
const TICK: Duration = Duration::from_millis(40);
const TICKS_PER_FRAME: u32 = 6;
/// Characters across the dialogue box.
const COLUMNS: usize = 47;
/// "(pause)", in ticks.
const PAUSE: u32 = 30;
/// The least time the wall display spends looking for an OS, in ticks.
const LOOKING: u32 = 45;

pub fn skip_is_set() -> bool {
    runtime::get_variable_boxed(&util::cstring16(SKIP_VAR), &VariableVendor(WEEFY_VARS)).is_ok_and(|(data, _)| data.first() == Some(&1))
}

/// Stores or removes the variable. It fails on firmware that can't keep
/// variables; `intro=off` in weefy.conf does the same job there.
pub fn set_skip(on: bool) -> uefi::Result {
    let (name, vendor) = (util::cstring16(SKIP_VAR), VariableVendor(WEEFY_VARS));
    if on {
        let attrs = VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;
        runtime::set_variable(&name, &vendor, attrs, &[1])
    } else {
        match runtime::delete_variable(&name, &vendor) {
            Err(e) if e.status() == Status::NOT_FOUND => Ok(()),
            other => other,
        }
    }
}

/// One screen of dialogue.
struct Say {
    mood: Mood,
    wall: Wall,
    text: String,
    /// After a pause, a change of mood and something more.
    then: Option<(Mood, String)>,
}

fn say(mood: Mood, text: &str) -> Say {
    Say {
        mood,
        wall: Wall::Phases,
        text: text.into(),
        then: None,
    }
}

fn opening() -> Vec<Say> {
    alloc::vec![
        say(Mood::Happy, "Welcome to my UEFI lab!"),
        say(Mood::Serious, "We have an important job here:"),
        say(Mood::Happy, "Booting your operating system!"),
        say(Mood::Serious, "You brought an OS didn't you?"),
    ]
}

/// The operating system this program would boot, as the wall display
/// shows it: where it was found, its name, its loader's file.
struct Found {
    from: String,
    name: String,
    file: String,
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.into();
    }
    let mut out: String = s.chars().take(max - 2).collect();
    out.push_str("..");
    out
}

/// "(check tables for an OS)": the default boot target, found the way the
/// Boot screen finds it.
fn look_for_os(prefer: Option<&str>) -> Option<Found> {
    let targets = bootopt::targets();
    let t = bootopt::default_target(&targets, prefer)?;
    Some(Found {
        from: match t.source {
            Source::Entry(n) => format!("Boot{n:04X}"),
            Source::File => String::from("found on a disk"),
        },
        name: t.title.clone(),
        file: t.resolved.first().and_then(|p| util::dp_file_name(p)).unwrap_or_default(),
    })
}

fn closing(found: &Option<Found>) -> Vec<Say> {
    let wall = match found {
        Some(f) => Wall::Found {
            from: clip(&f.from, 24),
            name: clip(&f.name, 24),
            file: clip(&f.file, 29),
        },
        None => Wall::Nothing,
    };
    let mut out = Vec::new();
    match found {
        Some(f) => out.push(Say {
            mood: Mood::Happy,
            wall: wall.clone(),
            text: format!("Great! I can see you have installed {}.", clip(&f.name, 60)),
            then: None,
        }),
        None => out.push(Say {
            mood: Mood::Meh,
            wall: wall.clone(),
            text: "Oh. Well... That's ok! It's fine.".into(),
            then: Some((Mood::Annoyed, "it's fine.".into())),
        }),
    }
    out.push(Say {
        mood: Mood::Happy,
        wall,
        text: "Well, feel free to look around!".into(),
        then: None,
    });
    out.push(say(Mood::Serious, "You can skip this dialogue in the future, if you want. Select [skip dialogue] from the menu."));
    out.push(say(Mood::Meh, "You don't have to."));
    out.push(say(Mood::Happy, "Let me know if you have any questions."));
    out
}

/// Plays the intro, then gives the console back to the text screens.
pub fn play(scr: &mut Screen, prefer: Option<&str>) {
    tui::drain_keys();
    match Display::open() {
        Some(display) => cartoon(display, prefer),
        None => plain(scr, prefer),
    }
    tui::drain_keys();
    scr.reset();
}

// --------------------------------------------------------------- the cartoon

/// The state of the drawing loop.
struct Stage {
    display: Display,
    canvas: Canvas,
    tick: u32,
}

impl Stage {
    fn frame(&self) -> u32 {
        self.tick / TICKS_PER_FRAME
    }

    /// Shows the canvas, waits one tick, and returns a key if one came.
    fn step(&mut self) -> Option<Key> {
        self.display.present(&self.canvas);
        let key = tui::poll_key();
        boot::stall(TICK);
        self.tick += 1;
        key
    }
}

/// The first `count` characters of some lines of text, in the dialogue box
/// from line `row` down.
fn write(g: &mut Canvas, lines: &[String], row: usize, count: usize) {
    let mut left = count;
    for (i, line) in lines.iter().enumerate() {
        if left == 0 {
            break;
        }
        let part: String = line.chars().take(left).collect();
        left -= part.chars().count();
        g.text(14, 156 + ((row + i) * 11) as i32, &part, 10, 1);
    }
}

fn length(lines: &[String]) -> usize {
    lines.iter().map(|l| l.chars().count()).sum()
}

/// One screen of dialogue, typed out a character a tick. A key finishes the
/// typing, or moves on if it has finished. False: Esc, skip the rest.
fn speak(stage: &mut Stage, s: &Say) -> bool {
    let first = tui::wrap(&s.text, COLUMNS);
    let second = s.then.as_ref().map(|(_, text)| tui::wrap(text, COLUMNS)).unwrap_or_default();
    let (total, total2) = (length(&first), length(&second));
    // Characters shown of each part, and how long the pause has lasted.
    let (mut shown, mut shown2, mut paused) = (0, 0, 0);
    loop {
        let turned = s.then.is_some() && paused >= PAUSE;
        let mood = match &s.then {
            Some((mood, _)) if turned => *mood,
            _ => s.mood,
        };
        let done = shown >= total && (s.then.is_none() || shown2 >= total2);
        let frame = stage.frame();
        art::lab(&mut stage.canvas, frame, mood, &s.wall);
        write(&mut stage.canvas, &first, 0, shown);
        write(&mut stage.canvas, &second, first.len(), shown2);
        if done && stage.frame() % 4 < 2 {
            art::more(&mut stage.canvas);
        }
        match stage.step() {
            Some(Key::Esc) => return false,
            Some(_) if done => return true,
            Some(_) => {
                shown = total;
                if s.then.is_some() {
                    paused = PAUSE;
                    shown2 = total2;
                }
            }
            None if shown < total => shown += 1,
            None if s.then.is_some() && paused < PAUSE => paused += 1,
            None if shown2 < total2 => shown2 += 1,
            None => {}
        }
    }
}

fn cartoon(display: Display, prefer: Option<&str>) {
    let mut stage = Stage {
        display,
        canvas: Canvas::new(),
        tick: 0,
    };
    // For whoever is watching a serial port that shares the console.
    tui::print_raw("\nweefy: the intro is playing on the display. Esc skips it.\n");
    stage.display.wipe();

    // The logo screen.
    loop {
        let frame = stage.frame();
        art::title(&mut stage.canvas, frame);
        match stage.step() {
            Some(Key::Esc) => return,
            Some(_) => break,
            None => {}
        }
    }
    for s in opening() {
        if !speak(&mut stage, &s) {
            return;
        }
    }
    // The wall display looks for an OS. The looking is done after the
    // first picture of it is up, since reading the disks can take a moment.
    let start = stage.tick;
    let mut found = None;
    let mut looked = false;
    while stage.tick - start < LOOKING {
        let frame = stage.frame();
        art::lab(&mut stage.canvas, frame, Mood::Serious, &Wall::Checking);
        let dots = ((stage.tick - start) / 10 % 4) as usize;
        stage.canvas.text(14, 156, &"...."[..dots], 10, 1);
        match stage.step() {
            Some(Key::Esc) => return,
            Some(_) if looked => break,
            _ => {}
        }
        if !looked {
            found = look_for_os(prefer);
            looked = true;
        }
    }
    for s in closing(&found) {
        if !speak(&mut stage, &s) {
            return;
        }
    }
}

// ------------------------------------------------------- the same, in text

fn mood_name(mood: Mood) -> &'static str {
    match mood {
        Mood::Happy => "happy",
        Mood::Serious => "serious",
        Mood::Meh => "mildly displeased",
        Mood::Annoyed => "visibly annoyed",
    }
}

/// The dialogue as message boxes, for a console with no display.
fn plain(scr: &mut Screen, prefer: Option<&str>) {
    scr.clear();
    tui::title_bar(scr, "Weefy teaches UEFI, with Dr. Weefy Fulgens");
    let hints = [("Enter", "Next"), ("Esc", "Skip")];
    let mut lines = opening();
    lines.extend(closing(&look_for_os(prefer)));
    for s in lines {
        let key = tui::message(
            scr,
            "Dr. Weefy Fulgens",
            &|w| {
                let mut d = Doc::new(w);
                d.dim(&format!("[weefy {}]", mood_name(s.mood)));
                d.styled(st::BRIGHT, &s.text);
                if let Some((mood, text)) = &s.then {
                    d.blank();
                    d.dim(&format!("[weefy {}]", mood_name(*mood)));
                    d.styled(st::BRIGHT, text);
                }
                d
            },
            &hints,
        );
        if key == Key::Esc {
            return;
        }
    }
}
