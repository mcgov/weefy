//! The text UI: a cell buffer over the Simple Text Output protocol, key input
//! over Simple Text Input, and the one widget every screen is built from, a
//! list beside a detail pane.
//!
//! Only characters the UEFI specification requires a console to draw are
//! used (ASCII, box drawing, a few arrows and blocks), so the same screen
//! works on a graphics console and, approximated, on a serial terminal.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;
use uefi::proto::console::text::{Key as EfiKey, ScanCode};
use uefi::{boot, system, table};
use uefi_raw::protocol::console::SimpleTextOutputProtocol;

/// A foreground and background colour, as the attribute byte Simple Text
/// Output takes: foreground in the low four bits, background in the next three.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Style(pub u8);

impl Style {
    pub const fn new(fg: u8, bg: u8) -> Self {
        Style(fg | (bg << 4))
    }
}

mod color {
    pub const BLACK: u8 = 0;
    pub const BLUE: u8 = 1;
    pub const CYAN: u8 = 3;
    pub const LIGHTGRAY: u8 = 7;
    pub const DARKGRAY: u8 = 8;
    pub const LIGHTGREEN: u8 = 10;
    pub const LIGHTCYAN: u8 = 11;
    pub const LIGHTRED: u8 = 12;
    pub const YELLOW: u8 = 14;
    pub const WHITE: u8 = 15;
}

/// The palette.
pub mod st {
    use super::Style;
    use super::color::*;
    pub const NORMAL: Style = Style::new(LIGHTGRAY, BLACK);
    pub const DIM: Style = Style::new(DARKGRAY, BLACK);
    pub const BRIGHT: Style = Style::new(WHITE, BLACK);
    pub const HEAD: Style = Style::new(YELLOW, BLACK);
    pub const KEY: Style = Style::new(LIGHTCYAN, BLACK);
    pub const GOOD: Style = Style::new(LIGHTGREEN, BLACK);
    pub const BAD: Style = Style::new(LIGHTRED, BLACK);
    pub const BORDER: Style = Style::new(DARKGRAY, BLACK);
    pub const BORDER_FOCUS: Style = Style::new(LIGHTCYAN, BLACK);
    pub const TITLE: Style = Style::new(BLACK, CYAN);
    pub const TITLE_BRIGHT: Style = Style::new(WHITE, CYAN);
    pub const SELECT: Style = Style::new(WHITE, BLUE);
    pub const SELECT_IDLE: Style = Style::new(BLACK, LIGHTGRAY);
    pub const FOOT: Style = Style::new(LIGHTGRAY, BLACK);
    pub const FOOT_KEY: Style = Style::new(BLACK, CYAN);
}

// Box drawing and symbols from the UEFI specification's required set.
pub const H: char = '\u{2500}';
pub const V: char = '\u{2502}';
const TL: char = '\u{250c}';
const TR: char = '\u{2510}';
const BL: char = '\u{2514}';
const BR: char = '\u{2518}';
pub const BLOCK: char = '\u{2588}';
pub const SHADE: char = '\u{2591}';
pub const ARROW_UP: char = '\u{2191}';
pub const ARROW_DOWN: char = '\u{2193}';
pub const TRI_RIGHT: char = '\u{25ba}';
pub const TRI_UP: char = '\u{25b2}';
pub const TRI_DOWN: char = '\u{25bc}';

/// The character to draw for `c`: itself if every UEFI console can draw it.
fn drawable(c: char) -> u16 {
    match c {
        ' '..='~' => c as u16,
        '\u{2500}' | '\u{2502}' | '\u{250c}' | '\u{2510}' | '\u{2514}' | '\u{2518}'
        | '\u{251c}' | '\u{2524}' | '\u{252c}' | '\u{2534}' | '\u{253c}' | '\u{2550}'
        | '\u{2551}' | '\u{2588}' | '\u{2591}' | '\u{2190}'..='\u{2193}' | '\u{25b2}'
        | '\u{25ba}' | '\u{25bc}' | '\u{25c4}' => c as u16,
        '\t' => b' ' as u16,
        _ => b'?' as u16,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cell {
    ch: u16,
    attr: u8,
}

const BLANK: Cell = Cell {
    ch: b' ' as u16,
    attr: st::NORMAL.0,
};

fn stdout() -> *mut SimpleTextOutputProtocol {
    // SAFETY: the system table is set before main runs and stays valid while
    // boot services are active, which is this program's whole life.
    unsafe { table::system_table_raw().expect("no system table").as_ref().stdout }
}

/// One text mode of the console.
#[derive(Clone, Copy)]
pub struct TextMode {
    pub index: usize,
    pub cols: usize,
    pub rows: usize,
}

/// The console's text modes, and the index of the current one.
pub fn text_modes() -> (Vec<TextMode>, usize) {
    let out = stdout();
    let mut modes = Vec::new();
    // SAFETY: `out` is the firmware's console protocol; its mode pointer is
    // valid while boot services are active.
    unsafe {
        let mode = &*(*out).mode;
        for index in 0..mode.max_mode.max(0) as usize {
            let (mut cols, mut rows) = (0usize, 0usize);
            if ((*out).query_mode)(out, index, &mut cols, &mut rows).is_success() {
                modes.push(TextMode { index, cols, rows });
            }
        }
        (modes, mode.mode.max(0) as usize)
    }
}

/// The screen: what should be shown, and what the console shows now. Drawing
/// goes to the first; `flush` sends the difference.
pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    cur: Vec<Cell>,
    shown: Vec<Cell>,
    force: bool,
}

impl Screen {
    pub fn new() -> Self {
        let mut s = Screen {
            cols: 0,
            rows: 0,
            cur: Vec::new(),
            shown: Vec::new(),
            force: true,
        };
        s.reset();
        s
    }

    /// Takes the console over again: reads its size, hides the cursor and
    /// redraws everything on the next flush. For the start, after a mode
    /// change, and after another program has used the console.
    pub fn reset(&mut self) {
        let out = stdout();
        let (modes, current) = text_modes();
        let (cols, rows) = modes
            .iter()
            .find(|m| m.index == current)
            .map(|m| (m.cols, m.rows))
            .unwrap_or((80, 25));
        self.cols = cols.max(40);
        self.rows = rows.max(10);
        self.cur = vec![BLANK; self.cols * self.rows];
        self.shown = vec![BLANK; self.cols * self.rows];
        self.force = true;
        // SAFETY: plain calls into the console protocol.
        unsafe {
            let _ = ((*out).enable_cursor)(out, false.into());
            let _ = ((*out).set_attribute)(out, st::NORMAL.0 as usize);
            let _ = ((*out).clear_screen)(out);
        }
    }

    /// Switches the console to another text mode.
    pub fn set_mode(&mut self, index: usize) -> bool {
        let out = stdout();
        // SAFETY: a plain call into the console protocol.
        let ok = unsafe { ((*out).set_mode)(out, index).is_success() };
        self.reset();
        ok
    }

    /// Hands the console to another program: default colours, a clear screen
    /// and a visible cursor.
    pub fn release(&mut self) {
        let out = stdout();
        // SAFETY: plain calls into the console protocol.
        unsafe {
            let _ = ((*out).set_attribute)(out, st::NORMAL.0 as usize);
            let _ = ((*out).clear_screen)(out);
            let _ = ((*out).enable_cursor)(out, true.into());
        }
        self.force = true;
    }

    pub fn clear(&mut self) {
        self.cur.fill(BLANK);
    }

    /// Draws `s` at (x, y), clipped to `max` cells and to the screen. Returns
    /// the column after the last one drawn.
    pub fn put(&mut self, x: usize, y: usize, style: Style, s: &str, max: usize) -> usize {
        if y >= self.rows {
            return x;
        }
        let end = (x.saturating_add(max)).min(self.cols);
        let mut cx = x;
        for c in s.chars() {
            if cx >= end {
                break;
            }
            self.cur[y * self.cols + cx] = Cell {
                ch: drawable(c),
                attr: style.0,
            };
            cx += 1;
        }
        cx
    }

    pub fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, style: Style, c: char) {
        let cell = Cell {
            ch: drawable(c),
            attr: style.0,
        };
        for yy in y..(y + h).min(self.rows) {
            for xx in x..(x + w).min(self.cols) {
                self.cur[yy * self.cols + xx] = cell;
            }
        }
    }

    /// A box with a title in its top edge and a note in its bottom edge.
    pub fn frame(&mut self, x: usize, y: usize, w: usize, h: usize, style: Style, title: &str, note: &str) {
        if w < 2 || h < 2 {
            return;
        }
        self.fill(x, y, w, 1, style, H);
        self.fill(x, y + h - 1, w, 1, style, H);
        self.fill(x, y, 1, h, style, V);
        self.fill(x + w - 1, y, 1, h, style, V);
        self.fill(x, y, 1, 1, style, TL);
        self.fill(x + w - 1, y, 1, 1, style, TR);
        self.fill(x, y + h - 1, 1, 1, style, BL);
        self.fill(x + w - 1, y + h - 1, 1, 1, style, BR);
        if !title.is_empty() && w > 6 {
            let end = self.put(x + 2, y, style, " ", 1);
            let end = self.put(end, y, st::BRIGHT, title, w - 6);
            self.put(end, y, style, " ", 1);
        }
        let n = note.chars().count();
        if n > 0 && w > n + 4 {
            self.put(x + w - n - 3, y + h - 1, style, " ", 1);
            self.put(x + w - n - 2, y + h - 1, style, note, n);
            self.put(x + w - 2, y + h - 1, style, " ", 1);
        }
    }

    /// Sends what changed since the last flush to the console.
    pub fn flush(&mut self) {
        let out = stdout();
        let mut buf: Vec<u16> = Vec::with_capacity(self.cols + 1);
        for y in 0..self.rows {
            // Writing the bottom-right cell would scroll the console.
            let limit = if y == self.rows - 1 { self.cols - 1 } else { self.cols };
            let row = y * self.cols;
            let mut x = 0;
            while x < limit {
                if !self.force && self.cur[row + x] == self.shown[row + x] {
                    x += 1;
                    continue;
                }
                let attr = self.cur[row + x].attr;
                let start = x;
                buf.clear();
                while x < limit
                    && self.cur[row + x].attr == attr
                    && (self.force || self.cur[row + x] != self.shown[row + x])
                {
                    buf.push(self.cur[row + x].ch);
                    x += 1;
                }
                buf.push(0);
                // SAFETY: plain calls into the console protocol with a
                // NUL-terminated UCS-2 string.
                unsafe {
                    let _ = ((*out).set_cursor_position)(out, start, y);
                    let _ = ((*out).set_attribute)(out, attr as usize);
                    let _ = ((*out).output_string)(out, buf.as_ptr());
                }
            }
        }
        self.shown.copy_from_slice(&self.cur);
        self.force = false;
    }
}

/// Prints plain text through the console, outside the cell buffer: for the
/// moments around starting another program.
pub fn print_raw(s: &str) {
    let out = stdout();
    let mut buf: Vec<u16> = Vec::new();
    for c in s.chars() {
        if c == '\n' {
            buf.push(b'\r' as u16);
            buf.push(b'\n' as u16);
        } else {
            buf.push(drawable(c));
        }
    }
    buf.push(0);
    // SAFETY: a NUL-terminated UCS-2 string for the console protocol.
    unsafe {
        let _ = ((*out).output_string)(out, buf.as_ptr());
    }
}

// --- Keys --------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    PgUp,
    PgDn,
    Home,
    End,
    Enter,
    Esc,
    Tab,
    Backspace,
    Char(char),
    Other,
}

/// A key, if one is waiting.
pub fn poll_key() -> Option<Key> {
    let key = system::with_stdin(|i| i.read_key()).ok().flatten()?;
    Some(match key {
        EfiKey::Printable(c) => match char::from(c) {
            '\r' | '\n' => Key::Enter,
            '\t' => Key::Tab,
            '\u{8}' | '\u{7f}' => Key::Backspace,
            '\u{1b}' => Key::Esc,
            c if c >= ' ' => Key::Char(c),
            _ => Key::Other,
        },
        EfiKey::Special(s) => match s {
            ScanCode::UP => Key::Up,
            ScanCode::DOWN => Key::Down,
            ScanCode::LEFT => Key::Left,
            ScanCode::RIGHT => Key::Right,
            ScanCode::PAGE_UP => Key::PgUp,
            ScanCode::PAGE_DOWN => Key::PgDn,
            ScanCode::HOME => Key::Home,
            ScanCode::END => Key::End,
            ScanCode::ESCAPE => Key::Esc,
            _ => Key::Other,
        },
    })
}

/// Waits for a key.
pub fn read_key() -> Key {
    loop {
        if let Some(k) = poll_key() {
            return k;
        }
        match system::with_stdin(|i| i.wait_for_key_event()) {
            Ok(ev) => {
                let _ = boot::wait_for_event(&[ev]);
            }
            Err(_) => boot::stall(Duration::from_millis(10)),
        }
    }
}

/// Throws away keys typed ahead.
pub fn drain_keys() {
    while poll_key().is_some() {}
}

// --- Styled text ---------------------------------------------------------

#[derive(Clone)]
pub struct Span {
    pub style: Style,
    pub text: String,
}

#[derive(Clone, Default)]
pub struct Line {
    pub spans: Vec<Span>,
}

impl Line {
    pub fn of(style: Style, text: impl Into<String>) -> Self {
        Line {
            spans: vec![Span {
                style,
                text: text.into(),
            }],
        }
    }

    pub fn push(mut self, style: Style, text: impl Into<String>) -> Self {
        self.spans.push(Span {
            style,
            text: text.into(),
        });
        self
    }
}

/// Breaks `text` into lines of at most `width` characters at spaces.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    let mut line = String::new();
    let mut len = 0;
    for word in text.split_whitespace() {
        let mut word = word;
        let mut wlen = word.chars().count();
        if len > 0 && len + 1 + wlen > width {
            out.push(core::mem::take(&mut line));
            len = 0;
        }
        // A word longer than a line (a path, a GUID list) is cut.
        while wlen > width {
            let cut = word.char_indices().nth(width).map(|(i, _)| i).unwrap_or(word.len());
            out.push(word[..cut].to_string());
            word = &word[cut..];
            wlen -= width;
        }
        if len > 0 {
            line.push(' ');
            len += 1;
        }
        line.push_str(word);
        len += wlen;
    }
    if len > 0 || out.is_empty() {
        out.push(line);
    }
    out
}

/// A page of styled lines being written for a pane `width` cells wide.
pub struct Doc {
    pub lines: Vec<Line>,
    pub width: usize,
}

impl Doc {
    pub fn new(width: usize) -> Self {
        Doc {
            lines: Vec::new(),
            width: width.max(20),
        }
    }

    pub fn blank(&mut self) {
        self.lines.push(Line::default());
    }

    /// A heading, with a blank line before it unless it starts the page.
    pub fn h(&mut self, text: &str) {
        if !self.lines.is_empty() {
            self.blank();
        }
        self.lines.push(Line::of(st::HEAD, text));
    }

    /// A wrapped paragraph.
    pub fn p(&mut self, text: &str) {
        self.styled(st::NORMAL, text);
    }

    pub fn dim(&mut self, text: &str) {
        self.styled(st::DIM, text);
    }

    pub fn styled(&mut self, style: Style, text: &str) {
        for l in wrap(text, self.width) {
            self.lines.push(Line::of(style, l));
        }
    }

    /// A line as it is, not wrapped: for diagrams and tables.
    pub fn raw(&mut self, style: Style, text: impl Into<String>) {
        self.lines.push(Line::of(style, text));
    }

    pub fn line(&mut self, line: Line) {
        self.lines.push(line);
    }

    /// A bullet point with a hanging indent.
    pub fn bullet(&mut self, text: &str) {
        for (i, l) in wrap(text, self.width.saturating_sub(4)).into_iter().enumerate() {
            let lead = if i == 0 { "  - " } else { "    " };
            self.lines.push(Line::of(st::DIM, lead).push(st::NORMAL, l));
        }
    }

    /// "name  value", the value wrapped under itself.
    pub fn kv(&mut self, key: &str, value: impl AsRef<str>) {
        self.kv_styled(key, st::NORMAL, value.as_ref());
    }

    pub fn kv_styled(&mut self, key: &str, style: Style, value: &str) {
        const KW: usize = 15;
        let klen = key.chars().count();
        let pad = if klen < KW { KW - klen } else { 1 };
        let indent = klen + pad;
        let room = self.width.saturating_sub(indent);
        if room < 12 {
            self.lines.push(Line::of(st::KEY, key));
            for l in wrap(value, self.width.saturating_sub(2)) {
                self.lines.push(Line::of(st::NORMAL, "  ").push(style, l));
            }
            return;
        }
        for (i, l) in wrap(value, room).into_iter().enumerate() {
            let line = if i == 0 {
                Line::of(st::KEY, key).push(st::NORMAL, " ".repeat(pad))
            } else {
                Line::of(st::NORMAL, " ".repeat(indent))
            };
            self.lines.push(line.push(style, l));
        }
    }

    /// A hex dump of at most `max` bytes.
    pub fn hexdump(&mut self, data: &[u8], max: usize) {
        let per = if self.width >= 76 { 16 } else { 8 };
        for (n, chunk) in data[..data.len().min(max)].chunks(per).enumerate() {
            let mut hex = String::new();
            let mut asc = String::new();
            for (i, b) in chunk.iter().enumerate() {
                if i == 8 {
                    hex.push(' ');
                }
                hex.push_str(&alloc::format!("{b:02x} "));
                asc.push(if (0x20..0x7f).contains(b) { *b as char } else { '.' });
            }
            let want = per * 3 + if per > 8 { 1 } else { 0 };
            while hex.len() < want {
                hex.push(' ');
            }
            self.lines.push(
                Line::of(st::DIM, alloc::format!("{:04x}  ", n * per))
                    .push(st::NORMAL, hex)
                    .push(st::DIM, " ")
                    .push(st::KEY, asc),
            );
        }
        if data.len() > max {
            self.dim(&alloc::format!("... {} more bytes", data.len() - max));
        }
    }
}

// --- The browser ---------------------------------------------------------

/// A row of a browser's list. A header row labels the rows under it and
/// can't be selected.
pub struct Item {
    pub label: String,
    pub style: Style,
    pub header: bool,
}

impl Item {
    pub fn new(label: impl Into<String>) -> Self {
        Item {
            label: label.into(),
            style: st::NORMAL,
            header: false,
        }
    }

    pub fn header(label: impl Into<String>) -> Self {
        Item {
            label: label.into(),
            style: st::HEAD,
            header: true,
        }
    }

    pub fn styled(mut self, style: Style) -> Self {
        self.style = style;
        self
    }
}

/// Where a browser is: kept by the caller so that coming back from a deeper
/// screen lands on the same row.
#[derive(Default)]
pub struct BrowseState {
    pub sel: usize,
    top: usize,
    scroll: usize,
    /// The detail pane has the keys.
    detail_focus: bool,
    /// The detail pane fills the screen.
    pub zoom: bool,
    find: String,
}

pub enum Outcome {
    /// Leave this screen.
    Back,
    /// Enter was pressed on this row.
    Open(usize),
    /// A key the browser has no use for.
    Char(char),
}

/// Set in a copy of this program that another copy started.
pub static STAGE2: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Draws the bar at the top of every screen.
pub fn title_bar(scr: &mut Screen, crumbs: &str) {
    scr.fill(0, 0, scr.cols, 1, st::TITLE, ' ');
    let x = scr.put(1, 0, st::TITLE_BRIGHT, "weefy", 8);
    let x = scr.put(x, 0, st::TITLE, " ", 1);
    let x = scr.put(x, 0, st::TITLE, &V.to_string(), 1);
    scr.put(x + 1, 0, st::TITLE, crumbs, scr.cols.saturating_sub(x + 2));
    let right = if STAGE2.load(core::sync::atomic::Ordering::Relaxed) {
        "stage two: started by weefy"
    } else {
        "the UEFI explorer"
    };
    if scr.cols > x + crumbs.chars().count() + right.len() + 6 {
        scr.put(scr.cols - right.len() - 1, 0, st::TITLE, right, right.len());
    }
}

/// Draws the key hints at the bottom of every screen.
pub fn footer(scr: &mut Screen, hints: &[(&str, &str)]) {
    let y = scr.rows - 1;
    scr.fill(0, y, scr.cols, 1, st::FOOT, ' ');
    let mut x = 0;
    for (key, what) in hints {
        if x + key.chars().count() + what.chars().count() + 3 >= scr.cols {
            break;
        }
        x = scr.put(x, y, st::FOOT_KEY, &alloc::format!(" {key} "), 20);
        x = scr.put(x, y, st::FOOT, &alloc::format!(" {what}  "), 40);
    }
}

fn draw_line(scr: &mut Screen, x: usize, y: usize, w: usize, line: &Line) {
    let mut cx = x;
    for span in &line.spans {
        cx = scr.put(cx, y, span.style, &span.text, (x + w).saturating_sub(cx));
    }
}

fn step(items: &[Item], from: usize, down: bool, count: usize) -> usize {
    let mut at = from;
    let mut last_ok = from;
    let mut moved = 0;
    loop {
        if down {
            if at + 1 >= items.len() {
                break;
            }
            at += 1;
        } else {
            if at == 0 {
                break;
            }
            at -= 1;
        }
        if !items[at].header {
            last_ok = at;
            moved += 1;
            if moved == count {
                break;
            }
        }
    }
    last_ok
}

fn find_from(items: &[Item], start: usize, needle: &str) -> Option<usize> {
    if needle.is_empty() || items.is_empty() {
        return None;
    }
    let needle = needle.to_ascii_lowercase();
    (0..items.len())
        .map(|i| (start + i) % items.len())
        .find(|&i| !items[i].header && items[i].label.to_ascii_lowercase().contains(&needle))
}

/// Asks for a line of text on the bottom row. None if Esc was pressed.
pub fn prompt(scr: &mut Screen, label: &str) -> Option<String> {
    let mut text = String::new();
    loop {
        let y = scr.rows - 1;
        scr.fill(0, y, scr.cols, 1, st::FOOT, ' ');
        let x = scr.put(0, y, st::FOOT_KEY, &alloc::format!(" {label} "), 30);
        let x = scr.put(x + 1, y, st::BRIGHT, &text, scr.cols.saturating_sub(x + 3));
        scr.put(x, y, st::BRIGHT, "_", 1);
        scr.flush();
        match read_key() {
            Key::Enter => return Some(text),
            Key::Esc => return None,
            Key::Backspace => {
                text.pop();
            }
            Key::Char(c) if text.len() < 60 => text.push(c),
            _ => {}
        }
    }
}

/// The list-and-detail screen. `detail(row, width)` writes the page for a
/// row; it is called again whenever the row, the width or the data (`r`)
/// changes. Returns when the user leaves, opens a row, or presses a key the
/// browser doesn't handle itself.
pub fn browse(
    scr: &mut Screen,
    state: &mut BrowseState,
    crumbs: &str,
    list_title: &str,
    items: &[Item],
    extra_hints: &[(&str, &str)],
    detail: &mut dyn FnMut(usize, usize) -> Doc,
) -> Outcome {
    if items.is_empty() {
        return Outcome::Back;
    }
    if state.sel >= items.len() {
        state.sel = items.len() - 1;
    }
    if items[state.sel].header {
        state.sel = step(items, state.sel, true, 1);
    }
    let mut cache: Option<(usize, usize, Doc)> = None;

    loop {
        LAST_BOX.store(0, core::sync::atomic::Ordering::Relaxed);
        let body_h = scr.rows - 2;
        let list_w = if state.zoom { 0 } else { (scr.cols * 2 / 5).clamp(24, 46) };
        let det_x = list_w;
        let det_w = scr.cols - list_w;
        let inner_h = body_h.saturating_sub(2);
        // Prose wider than this is hard to read on a large display.
        let text_w = det_w.saturating_sub(4).min(100);

        // Keep the selection in view.
        if state.sel < state.top {
            state.top = state.sel;
        }
        if state.sel >= state.top + inner_h {
            state.top = state.sel + 1 - inner_h;
        }
        // Show a header that sits just above the first row.
        if state.top > 0 && items[state.top - 1].header && state.sel < state.top + inner_h - 1 {
            state.top -= 1;
        }

        let stale = !matches!(&cache, Some((s, w, _)) if *s == state.sel && *w == text_w);
        if stale {
            cache = Some((state.sel, text_w, detail(state.sel, text_w)));
        }
        let doc = &cache.as_ref().unwrap().2;
        let max_scroll = doc.lines.len().saturating_sub(inner_h);
        state.scroll = state.scroll.min(max_scroll);

        scr.clear();
        title_bar(scr, crumbs);

        if !state.zoom {
            let focus = !state.detail_focus;
            let border = if focus { st::BORDER_FOCUS } else { st::BORDER };
            let selectable = items.iter().filter(|i| !i.header).count();
            let ordinal = items[..=state.sel].iter().filter(|i| !i.header).count();
            let note = alloc::format!("{ordinal}/{selectable}");
            scr.frame(0, 1, list_w, body_h, border, list_title, &note);
            for row in 0..inner_h {
                let i = state.top + row;
                if i >= items.len() {
                    break;
                }
                let item = &items[i];
                let y = 2 + row;
                let w = list_w - 2;
                if i == state.sel {
                    let style = if focus { st::SELECT } else { st::SELECT_IDLE };
                    scr.fill(1, y, w, 1, style, ' ');
                    scr.put(2, y, style, &item.label, w - 2);
                } else if item.header {
                    scr.put(1, y, item.style, &item.label, w);
                } else {
                    scr.put(2, y, item.style, &item.label, w - 2);
                }
            }
            if state.top > 0 {
                scr.put(list_w - 1, 2, border, &TRI_UP.to_string(), 1);
            }
            if state.top + inner_h < items.len() {
                scr.put(list_w - 1, body_h - 1, border, &TRI_DOWN.to_string(), 1);
            }
        }

        let det_focus = state.zoom || state.detail_focus;
        let border = if det_focus { st::BORDER_FOCUS } else { st::BORDER };
        let note = if max_scroll > 0 {
            alloc::format!(
                "{}-{}/{}",
                state.scroll + 1,
                (state.scroll + inner_h).min(doc.lines.len()),
                doc.lines.len()
            )
        } else {
            String::new()
        };
        let det_title: String = items[state.sel].label.trim().chars().take(det_w.saturating_sub(10)).collect();
        scr.frame(det_x, 1, det_w, body_h, border, &det_title, &note);
        for row in 0..inner_h {
            match doc.lines.get(state.scroll + row) {
                Some(line) => draw_line(scr, det_x + 2, 2 + row, text_w, line),
                None => break,
            }
        }
        if state.scroll > 0 {
            scr.put(det_x + det_w - 1, 2, border, &TRI_UP.to_string(), 1);
        }
        if state.scroll < max_scroll {
            scr.put(det_x + det_w - 1, body_h - 1, border, &TRI_DOWN.to_string(), 1);
        }

        let up_down = alloc::format!("{ARROW_UP}{ARROW_DOWN}");
        let mut hints: Vec<(&str, &str)> = Vec::new();
        if det_focus {
            hints.push((&up_down, "Scroll"));
            hints.push(("Esc", if state.zoom { "Close" } else { "List" }));
        } else {
            hints.push((&up_down, "Move"));
            hints.push(("Enter", "Open"));
            hints.push(("Tab", "Pane"));
            hints.push(("/", "Find"));
            hints.push(("Esc", "Back"));
        }
        hints.extend_from_slice(extra_hints);
        footer(scr, &hints);
        scr.flush();

        let key = read_key();
        let page = inner_h.saturating_sub(1).max(1);
        if det_focus {
            match key {
                Key::Up | Key::Char('k') => state.scroll = state.scroll.saturating_sub(1),
                Key::Down | Key::Char('j') => state.scroll = (state.scroll + 1).min(max_scroll),
                Key::PgUp => state.scroll = state.scroll.saturating_sub(page),
                Key::PgDn | Key::Char(' ') => state.scroll = (state.scroll + page).min(max_scroll),
                Key::Home | Key::Char('g') => state.scroll = 0,
                Key::End | Key::Char('G') => state.scroll = max_scroll,
                Key::Esc | Key::Backspace | Key::Char('q') | Key::Left | Key::Tab | Key::Enter => {
                    state.zoom = false;
                    state.detail_focus = false;
                }
                Key::Char('r') => cache = None,
                _ => {}
            }
            continue;
        }
        let before = state.sel;
        match key {
            Key::Up | Key::Char('k') => state.sel = step(items, state.sel, false, 1),
            Key::Down | Key::Char('j') => state.sel = step(items, state.sel, true, 1),
            Key::PgUp => state.sel = step(items, state.sel, false, page),
            Key::PgDn | Key::Char(' ') => state.sel = step(items, state.sel, true, page),
            Key::Home | Key::Char('g') => {
                state.sel = 0;
                if items[0].header {
                    state.sel = step(items, 0, true, 1);
                }
                state.top = 0;
            }
            Key::End | Key::Char('G') => {
                state.sel = items.len() - 1;
                if items[state.sel].header {
                    state.sel = step(items, state.sel, false, 1);
                }
            }
            Key::Tab | Key::Right | Key::Char('l') => state.detail_focus = true,
            Key::Enter => return Outcome::Open(state.sel),
            Key::Esc | Key::Backspace | Key::Left | Key::Char('q') | Key::Char('h') => return Outcome::Back,
            Key::Char('r') => cache = None,
            Key::Char('/') => {
                if let Some(text) = prompt(scr, "Find") {
                    state.find = text;
                    if let Some(i) = find_from(items, state.sel, &state.find) {
                        state.sel = i;
                    }
                }
            }
            Key::Char('n') => {
                if let Some(i) = find_from(items, state.sel + 1, &state.find) {
                    state.sel = i;
                }
            }
            Key::Char(c) => return Outcome::Char(c),
            _ => {}
        }
        if state.sel != before {
            state.scroll = 0;
        }
    }
}

/// Where the last message box is, if nothing has redrawn the screen since:
/// x, y, width and height, 16 bits each. A second box shown straight after
/// a first clears it away rather than leaving its edges showing.
static LAST_BOX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// A box in the middle of the screen. Returns the key that closed it.
pub fn message(scr: &mut Screen, title: &str, doc_for: &dyn Fn(usize) -> Doc, hints: &[(&str, &str)]) -> Key {
    use core::sync::atomic::Ordering::Relaxed;
    let w = (scr.cols * 3 / 4).clamp(40, 76).min(scr.cols);
    let doc = doc_for(w - 4);
    let h = (doc.lines.len() + 2).min(scr.rows - 4);
    let x = (scr.cols - w) / 2;
    let y = 1 + (scr.rows - 2 - h) / 2;
    let last = LAST_BOX.swap(((x as u64) << 48) | ((y as u64) << 32) | ((w as u64) << 16) | h as u64, Relaxed);
    if last != 0 {
        let part = |shift: u32| ((last >> shift) & 0xffff) as usize;
        scr.fill(part(48), part(32), part(16), part(0), st::NORMAL, ' ');
    }
    scr.fill(x, y, w, h, st::NORMAL, ' ');
    scr.frame(x, y, w, h, st::BORDER_FOCUS, title, "");
    for (row, line) in doc.lines.iter().take(h - 2).enumerate() {
        draw_line(scr, x + 2, y + 1 + row, w - 4, line);
    }
    footer(scr, hints);
    scr.flush();
    drain_keys();
    read_key()
}

/// A message box of plain lines, each with a style; an empty one is a gap.
pub fn notice(scr: &mut Screen, title: &str, lines: &[(Style, String)]) -> Key {
    message(
        scr,
        title,
        &|w| {
            let mut d = Doc::new(w);
            for (style, text) in lines {
                if text.is_empty() {
                    d.blank();
                } else {
                    d.styled(*style, text);
                }
            }
            d
        },
        &[("Any key", "Continue")],
    )
}
