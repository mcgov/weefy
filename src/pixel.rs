//! A 320 x 200 picture in 16 colours, and the way it gets to the display.
//!
//! The intro draws into a `Canvas`: one byte a pixel, each a palette index.
//! `Display` scales that by a whole number onto the firmware's framebuffer
//! with the Graphics Output Protocol's Blt(), sending only what changed.
//! Everything here is integer arithmetic: there is no maths library in a
//! UEFI application, and pixel art has no use for one.

use crate::util;
use alloc::vec::Vec;
use uefi::Handle;
use uefi::boot::ScopedProtocol;
use uefi::proto::console::gop::{BltOp, BltPixel, BltRegion, GraphicsOutput};
use uefi::table;

pub const W: i32 = 320;
pub const H: i32 = 200;

/// The 16 colours. In-between shades are made by checkerboarding two.
pub const PALETTE: [(u8, u8, u8); 16] = [
    (0x0b, 0x0b, 0x12), // 0 black
    (0x1b, 0x1f, 0x3b), // 1 navy
    (0x2f, 0x4a, 0x8a), // 2 blue
    (0x2e, 0x8f, 0x8f), // 3 teal
    (0x4f, 0xae, 0x5a), // 4 green
    (0x9b, 0xe3, 0x6a), // 5 phosphor
    (0x5a, 0x2d, 0x1c), // 6 brown
    (0xb5, 0x48, 0x2a), // 7 rust
    (0xe0, 0x7a, 0x34), // 8 orange
    (0xf3, 0xe2, 0xc0), // 9 cream
    (0xff, 0xff, 0xff), // 10 white
    (0xb9, 0xbc, 0xc8), // 11 light grey
    (0x6d, 0x71, 0x86), // 12 grey
    (0xf2, 0xc8, 0x4b), // 13 yellow
    (0xd9, 0x5d, 0x8a), // 14 pink
    (0x5b, 0x3f, 0x8f), // 15 purple
];

/// The largest integer whose square is not more than `n`.
pub fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// `n / d`, rounded to the nearest integer, halves upwards.
pub fn round_div(n: i32, d: i32) -> i32 {
    (2 * n + d).div_euclid(2 * d)
}

/// `x` rounded to the nearest integer, halves upwards.
pub fn round(x: f32) -> i32 {
    let t = x + 0.5;
    let i = t as i32;
    if (i as f32) > t { i - 1 } else { i }
}

/// The sine of `x` radians, near enough for a wobble on a screen.
pub fn sin(x: f32) -> f32 {
    const PI: f32 = core::f32::consts::PI;
    let turns = round(x / (2.0 * PI)) as f32;
    let x = x - turns * 2.0 * PI;
    // Bhaskara's approximation on each half turn.
    let (x, sign) = if x < 0.0 { (-x, -1.0) } else { (x, 1.0) };
    sign * 16.0 * x * (PI - x) / (5.0 * PI * PI - 4.0 * x * (PI - x))
}

pub struct Canvas {
    pub px: Vec<u8>,
}

impl Canvas {
    pub fn new() -> Self {
        Canvas {
            px: alloc::vec![0; (W * H) as usize],
        }
    }

    /// A filled rectangle, clipped to the picture.
    pub fn r(&mut self, x: i32, y: i32, w: i32, h: i32, c: u8) {
        let (x0, y0, x1, y1) = (x.max(0), y.max(0), (x + w).min(W), (y + h).min(H));
        for yy in y0..y1 {
            for xx in x0..x1 {
                self.px[(yy * W + xx) as usize] = c;
            }
        }
    }

    pub fn p(&mut self, x: i32, y: i32, c: u8) {
        self.r(x, y, 1, 1, c);
    }

    /// A rectangle of checkerboard: every other pixel in `c`.
    pub fn d(&mut self, x: i32, y: i32, w: i32, h: i32, c: u8) {
        for j in 0..h {
            for i in 0..w {
                if (x + i + y + j) % 2 == 0 {
                    self.p(x + i, y + j, c);
                }
            }
        }
    }

    fn ellipse(&mut self, cx: i32, cy: i32, rx: i32, ry: i32, c: u8, dither: bool) {
        let (rx, ry) = (rx.max(0), ry.max(0));
        // Half a pixel is added to each radius, so everything is doubled
        // to stay in whole numbers.
        let (a, b) = ((2 * rx + 1) as u64, (2 * ry + 1) as u64);
        for y in -ry..=ry {
            let t = b * b - 4 * (y * y) as u64;
            let w = (isqrt(a * a * t) / (2 * b)) as i32;
            if !dither {
                self.r(cx - w, cy + y, 2 * w + 1, 1, c);
            } else {
                for x in -w..=w {
                    if (cx + x + cy + y).rem_euclid(2) == 0 {
                        self.p(cx + x, cy + y, c);
                    }
                }
            }
        }
    }

    /// A filled ellipse.
    pub fn e(&mut self, cx: i32, cy: i32, rx: i32, ry: i32, c: u8) {
        self.ellipse(cx, cy, rx, ry, c, false);
    }

    /// An ellipse of checkerboard.
    pub fn ed(&mut self, cx: i32, cy: i32, rx: i32, ry: i32, c: u8) {
        self.ellipse(cx, cy, rx, ry, c, true);
    }

    /// A circle's outline, one pixel thick.
    pub fn ring(&mut self, cx: i32, cy: i32, rad: i32, c: u8) {
        for y in -rad - 1..=rad + 1 {
            for x in -rad - 1..=rad + 1 {
                // rad - 0.6 < distance <= rad + 0.45, in twentieths.
                let d2 = 400 * (x * x + y * y);
                let (outer, inner) = (20 * rad + 9, 20 * rad - 12);
                if d2 <= outer * outer && (inner < 0 || d2 > inner * inner) {
                    self.p(cx + x, cy + y, c);
                }
            }
        }
    }

    /// A line drawn as discs of radius `t` (0: one pixel wide).
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, t: i32, c: u8) {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let d2 = (dx * dx + dy * dy) as u64;
        let root = isqrt(d2);
        let n = ((root + u64::from(root * root != d2)) as i32).max(1);
        for i in 0..=n {
            self.e(x0 + round_div(dx * i, n), y0 + round_div(dy * i, n), t, t, c);
        }
    }

    /// A rectangle's outline.
    pub fn frame(&mut self, x: i32, y: i32, w: i32, h: i32, c: u8) {
        self.r(x, y, w, 1, c);
        self.r(x, y + h - 1, w, 1, c);
        self.r(x, y, 1, h, c);
        self.r(x + w - 1, y, 1, h, c);
    }

    /// Text in the 5 x 7 font, each pixel `scale` across. A character
    /// advances 6 pixels, so a line of the picture holds 53.
    pub fn text(&mut self, x: i32, y: i32, s: &str, c: u8, scale: i32) {
        for (i, ch) in s.chars().enumerate() {
            let code = ch as usize;
            let glyph = &FONT[if (32..127).contains(&code) { code - 32 } else { '?' as usize - 32 }];
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..5 {
                    if bits & (0b10000 >> col) != 0 {
                        self.r(x + (i as i32 * 6 + col) * scale, y + row as i32 * scale, scale, scale, c);
                    }
                }
            }
        }
    }

    /// Text with a drop shadow, for the titles.
    pub fn title(&mut self, x: i32, y: i32, s: &str, c: u8, shadow: u8, scale: i32) {
        let off = (scale / 2).max(1);
        self.text(x + off, y + off, s, shadow, scale);
        self.text(x, y, s, c, scale);
    }
}

/// The width of `s` in pixels at scale 1.
pub fn text_width(s: &str) -> i32 {
    s.chars().count() as i32 * 6
}

/// The display the console is on, and what was last sent to it.
pub struct Display {
    gop: ScopedProtocol<GraphicsOutput>,
    scale: i32,
    /// Where the picture's corner goes, to centre it.
    origin: (i32, i32),
    shown: Vec<u8>,
    fresh: bool,
    size: (usize, usize),
}

impl Display {
    /// The framebuffer behind the console, if the console has one and it
    /// is at least 320 x 200. A firmware puts the Graphics Output Protocol
    /// on its console handle when the console is drawn on a display; on a
    /// serial console there is none, and this returns None.
    pub fn open() -> Option<Self> {
        // SAFETY: the system table is valid while boot services are active.
        let handle = unsafe { table::system_table_raw()?.as_ref().stdout_handle };
        // SAFETY: the console handle is a handle or NULL.
        let handle = unsafe { Handle::from_ptr(handle.cast()) }?;
        let gop = util::open::<GraphicsOutput>(handle)?;
        let (w, h) = gop.current_mode_info().resolution();
        let scale = (w as i32 / W).min(h as i32 / H);
        if scale < 1 {
            return None;
        }
        Some(Display {
            gop,
            scale,
            origin: ((w as i32 - W * scale) / 2, (h as i32 - H * scale) / 2),
            shown: alloc::vec![0; (W * H) as usize],
            fresh: true,
            size: (w, h),
        })
    }

    /// Black over the whole display: what is round the picture, and under
    /// it until the first one is sent.
    pub fn wipe(&mut self) {
        let _ = self.gop.blt(BltOp::VideoFill {
            color: BltPixel::new(0, 0, 0),
            dest: (0, 0),
            dims: self.size,
        });
        self.fresh = true;
    }

    /// Sends the canvas to the display: the smallest rectangle that holds
    /// every pixel that changed since the last time.
    pub fn present(&mut self, canvas: &Canvas) {
        let (mut x0, mut y0, mut x1, mut y1) = (W, H, -1, -1);
        if self.fresh {
            (x0, y0, x1, y1) = (0, 0, W - 1, H - 1);
        } else {
            for y in 0..H {
                let row = (y * W) as usize..((y + 1) * W) as usize;
                let (new, old) = (&canvas.px[row.clone()], &self.shown[row]);
                if new == old {
                    continue;
                }
                let first = new.iter().zip(old).position(|(a, b)| a != b).unwrap_or(0) as i32;
                let last = new.iter().zip(old).rposition(|(a, b)| a != b).unwrap_or(0) as i32;
                (x0, x1) = (x0.min(first), x1.max(last));
                (y0, y1) = (y0.min(y), y1.max(y));
            }
        }
        if x1 < 0 {
            return;
        }
        let s = self.scale;
        let width = ((x1 - x0 + 1) * s) as usize;
        // In bands, so that a large display doesn't need a large buffer.
        let mut buf: Vec<BltPixel> = Vec::new();
        let mut top = y0;
        while top <= y1 {
            let bottom = (top + 24).min(y1);
            buf.clear();
            for y in top..=bottom {
                let start = buf.len();
                for x in x0..=x1 {
                    let (r, g, b) = PALETTE[(canvas.px[(y * W + x) as usize] & 15) as usize];
                    for _ in 0..s {
                        buf.push(BltPixel::new(r, g, b));
                    }
                }
                for _ in 1..s {
                    buf.extend_from_within(start..start + width);
                }
            }
            let _ = self.gop.blt(BltOp::BufferToVideo {
                buffer: &buf,
                src: BltRegion::Full,
                dest: ((self.origin.0 + x0 * s) as usize, (self.origin.1 + top * s) as usize),
                dims: (width, ((bottom - top + 1) * s) as usize),
            });
            top = bottom + 1;
        }
        self.shown.copy_from_slice(&canvas.px);
        self.fresh = false;
    }
}

/// The font: 5 x 7 glyphs for the characters from space to tilde, a row a
/// byte, the leftmost pixel in bit 4.
const FONT: [[u8; 7]; 95] = [
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000], //  
    [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100], // !
    [0b01010, 0b01010, 0b01010, 0b00000, 0b00000, 0b00000, 0b00000], // "
    [0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010], // #
    [0b00100, 0b01111, 0b10100, 0b01110, 0b00101, 0b11110, 0b00100], // $
    [0b11000, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b00011], // %
    [0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101], // &
    [0b00100, 0b00100, 0b01000, 0b00000, 0b00000, 0b00000, 0b00000], // '
    [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010], // (
    [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000], // )
    [0b00000, 0b00100, 0b10101, 0b01110, 0b10101, 0b00100, 0b00000], // *
    [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000], // +
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00110, 0b00100, 0b01000], // ,
    [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000], // -
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00110, 0b00110], // .
    [0b00000, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b00000], // /
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111], // 2
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
    [0b00000, 0b00110, 0b00110, 0b00000, 0b00110, 0b00110, 0b00000], // :
    [0b00000, 0b00110, 0b00110, 0b00000, 0b00110, 0b00100, 0b01000], // ;
    [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010], // <
    [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000], // =
    [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000], // >
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100], // ?
    [0b01110, 0b10001, 0b00001, 0b01101, 0b10101, 0b10101, 0b01110], // @
    [0b01110, 0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001], // A
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110], // B
    [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110], // C
    [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100], // D
    [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111], // E
    [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000], // F
    [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111], // G
    [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001], // H
    [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // I
    [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100], // J
    [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001], // K
    [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111], // L
    [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001], // M
    [0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001], // N
    [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110], // O
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000], // P
    [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101], // Q
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001], // R
    [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110], // S
    [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100], // T
    [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110], // U
    [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100], // V
    [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010], // W
    [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001], // X
    [0b10001, 0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100], // Y
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111], // Z
    [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110], // [
    [0b00000, 0b10000, 0b01000, 0b00100, 0b00010, 0b00001, 0b00000], // backslash
    [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110], // ]
    [0b00100, 0b01010, 0b10001, 0b00000, 0b00000, 0b00000, 0b00000], // ^
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111], // _
    [0b01000, 0b00100, 0b00010, 0b00000, 0b00000, 0b00000, 0b00000], // `
    [0b00000, 0b00000, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111], // a
    [0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b11110], // b
    [0b00000, 0b00000, 0b01110, 0b10000, 0b10000, 0b10001, 0b01110], // c
    [0b00001, 0b00001, 0b01101, 0b10011, 0b10001, 0b10001, 0b01111], // d
    [0b00000, 0b00000, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110], // e
    [0b00110, 0b01001, 0b01000, 0b11100, 0b01000, 0b01000, 0b01000], // f
    [0b00000, 0b01111, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110], // g
    [0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001], // h
    [0b00100, 0b00000, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110], // i
    [0b00010, 0b00000, 0b00110, 0b00010, 0b00010, 0b10010, 0b01100], // j
    [0b10000, 0b10000, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010], // k
    [0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // l
    [0b00000, 0b00000, 0b11010, 0b10101, 0b10101, 0b10001, 0b10001], // m
    [0b00000, 0b00000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001], // n
    [0b00000, 0b00000, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110], // o
    [0b00000, 0b00000, 0b11110, 0b10001, 0b11110, 0b10000, 0b10000], // p
    [0b00000, 0b00000, 0b01101, 0b10011, 0b01111, 0b00001, 0b00001], // q
    [0b00000, 0b00000, 0b10110, 0b11001, 0b10000, 0b10000, 0b10000], // r
    [0b00000, 0b00000, 0b01110, 0b10000, 0b01110, 0b00001, 0b11110], // s
    [0b01000, 0b01000, 0b11100, 0b01000, 0b01000, 0b01001, 0b00110], // t
    [0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b10011, 0b01101], // u
    [0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100], // v
    [0b00000, 0b00000, 0b10001, 0b10001, 0b10101, 0b10101, 0b01010], // w
    [0b00000, 0b00000, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001], // x
    [0b00000, 0b00000, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110], // y
    [0b00000, 0b00000, 0b11111, 0b00010, 0b00100, 0b01000, 0b11111], // z
    [0b00010, 0b00100, 0b00100, 0b01000, 0b00100, 0b00100, 0b00010], // {
    [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100], // |
    [0b01000, 0b00100, 0b00100, 0b00010, 0b00100, 0b00100, 0b01000], // }
    [0b00000, 0b00000, 0b01000, 0b10101, 0b00010, 0b00000, 0b00000], // ~
];
