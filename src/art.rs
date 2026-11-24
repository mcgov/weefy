//! The intro's pictures: Dr. Weefy Fulgens and his lab, drawn from
//! rectangles, ellipses and lines every time they are shown. There are no
//! image files; an animation frame is the same drawing with a different `f`.

use crate::pixel::{Canvas, round, sin, text_width};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Happy,
    Serious,
    /// Mildly displeased.
    Meh,
    /// Visibly annoyed.
    Annoyed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Arms {
    Down,
    Wave,
    Point,
}

/// How Weefy is standing in one picture.
pub struct Pose {
    pub mood: Mood,
    pub arms: Arms,
    pub blink: bool,
    /// The waving paw is at the top of its swing.
    pub wave_up: bool,
    /// Where the pointer ends, when he points.
    pub target: (i32, i32),
    /// Legs and tail are drawn (they aren't behind the bench).
    pub whole: bool,
}

/// Weefy, with his head centred on (`cx`, `cy`). Every shape is placed in
/// head-sized units and scaled by `s` before it is drawn, so the large
/// Weefy of the title and the small one of the lab are one design.
pub fn weefy(g: &mut Canvas, cx: i32, cy: i32, s: f32, o: &Pose) {
    let sz = |v: f32| round(v * s);
    let x = |v: f32| cx + sz(v);
    let y = |v: f32| cy + sz(v);
    let t = |v: f32| sz(v).max(1);

    if o.whole {
        // The tail: a row of discs along an arc, striped.
        let seg = |i: i32| {
            let k = i as f32 / 11.0;
            (x(-12.0 - 27.0 * k), y(50.0 - 26.0 * sin(k * 1.7)))
        };
        for i in 0..=11 {
            let (tx, ty) = seg(i);
            g.e(tx, ty, sz(5.0) + 1, sz(5.0) + 1, 6);
        }
        for i in 0..=11 {
            let (tx, ty) = seg(i);
            let c = if i >= 10 {
                6
            } else if i % 3 == 1 {
                9
            } else {
                8
            };
            g.e(tx, ty, sz(5.0), sz(5.0), c);
        }
        for side in [-1.0, 1.0] {
            g.r(x(side * 7.0) - sz(4.0), y(46.0), sz(8.0), sz(13.0), 6);
            g.e(x(side * 8.0), y(60.0), sz(5.0), sz(2.5), 0);
        }
    }

    // The lab coat: grey edge, white cloth, a grey hem.
    g.e(x(0.0), y(33.0), sz(16.0) + 1, sz(20.0) + 1, 12);
    g.r(x(-16.0) - 1, y(33.0), sz(32.0) + 3, sz(20.0) + 1, 12);
    g.e(x(0.0), y(33.0), sz(16.0), sz(20.0), 10);
    g.r(x(-16.0), y(33.0), sz(32.0) + 1, sz(20.0), 10);
    g.r(x(-16.0), y(53.0) - t(1.0), sz(32.0) + 1, t(1.0), 11);
    // Fur at the collar, with a lapel either side.
    let collar = sz(13.0);
    for i in 0..collar {
        let half = round((collar - i) as f32 * 0.55);
        g.r(x(0.0) - half, y(13.0) + i, half * 2 + 1, 1, 6);
        g.r(x(0.0) - half - t(1.0), y(13.0) + i, t(1.0), 1, 11);
        g.r(x(0.0) + half + 1, y(13.0) + i, t(1.0), 1, 11);
    }
    g.r(x(0.0), y(26.0), t(0.6), sz(27.0), 11);
    g.e(x(0.0) + t(1.5), y(33.0), t(0.5), t(0.5), 12);
    g.e(x(0.0) + t(1.5), y(40.0), t(0.5), t(0.5), 12);
    // The pocket and its pen.
    g.r(x(5.0), y(35.0), sz(7.0), sz(6.0), 11);
    g.r(x(5.0) + 1, y(35.0) + 1, sz(7.0) - 2, sz(6.0) - 2, 10);
    g.r(x(7.0), y(31.0), t(1.0), sz(5.0), 2);
    g.r(x(7.0), y(31.0), t(1.0), t(1.0), 13);

    // An arm: a sleeve from shoulder to wrist, and a paw past the wrist.
    let arm = |g: &mut Canvas, x0: f32, y0: f32, x1: f32, y1: f32| {
        g.line(x(x0), y(y0), x(x1), y(y1), sz(3.5) + 1, 12);
        g.line(x(x0), y(y0), x(x1), y(y1), sz(3.5), 10);
        g.e(x(x1 + (x1 - x0) * 0.22), y(y1 + (y1 - y0) * 0.22), sz(3.6), sz(3.6), 6);
    };
    arm(g, -14.0, 20.0, -18.0, 35.0);
    match o.arms {
        Arms::Wave => arm(g, 14.0, 20.0, 25.0, if o.wave_up { 5.0 } else { 7.0 }),
        Arms::Point => {
            arm(g, 14.0, 21.0, 25.0, 15.0);
            g.line(x(28.0), y(13.0), o.target.0, o.target.1, 0, 9);
            g.e(o.target.0, o.target.1, 1, 1, 13);
        }
        Arms::Down => arm(g, 14.0, 20.0, 18.0, 35.0),
    }

    // Ears: white, with rust inside.
    for side in [-1.0, 1.0] {
        g.e(x(side * 14.0), y(-13.0), sz(7.0) + 1, sz(8.0) + 1, 6);
        g.e(x(side * 14.0), y(-13.0), sz(7.0), sz(8.0), 10);
        g.e(x(side * 14.0), y(-12.0), sz(4.0), sz(5.0), 7);
    }
    // The head, a darker crown, white cheeks and muzzle.
    g.e(x(0.0), y(0.0), sz(18.0) + 1, sz(15.0) + 1, 6);
    g.e(x(0.0), y(0.0), sz(18.0), sz(15.0), 8);
    g.e(x(0.0), y(-8.0), sz(11.0), sz(5.0), 7);
    for side in [-1.0, 1.0] {
        g.e(x(side * 11.0), y(5.0), sz(6.0), sz(5.0), 10);
    }
    g.e(x(0.0), y(7.0), sz(7.0), sz(5.0), 10);

    for side in [-1.0f32, 1.0] {
        // The stripe a red panda has from eye to mouth.
        g.line(x(side * 8.0), y(2.0), x(side * 12.0), y(9.0), sz(0.9), 7);
        // The white spot over each eye.
        g.e(x(side * 7.0), y(-9.0), sz(3.0), sz(1.6), 10);
        // Mildly displeased, he looks away.
        let ex = x(side * 7.0) + if o.mood == Mood::Meh { t(0.8) } else { 0 };
        let ey = y(-2.0);
        if o.blink {
            g.r(x(side * 7.0) - sz(2.0), ey, sz(4.0) + 1, t(0.7), 0);
        } else {
            g.e(ex, ey, sz(2.0), sz(2.0), 0);
            g.r(ex - t(0.8), ey - t(0.9), t(0.7), t(0.7), 10);
            // A lowered lid, for the moods that have had enough.
            let drop = match o.mood {
                Mood::Annoyed => sz(2.0),
                Mood::Meh => sz(1.2),
                _ => 0,
            };
            if drop > 0 {
                g.r(ex - sz(2.0) - 1, ey - sz(2.0) - 1, sz(4.0) + 3, drop + 1, 8);
                g.r(ex - sz(2.0) - 1, ey - sz(2.0) + drop, sz(4.0) + 3, t(0.6), 6);
            }
        }
        // Glasses, the rims doubled on the large Weefy.
        g.ring(x(side * 7.0), y(-2.0), sz(5.0), 1);
        if s >= 1.8 {
            g.ring(x(side * 7.0), y(-2.0), sz(5.0) - 1, 1);
        }
        // Brows, over the rims.
        match o.mood {
            Mood::Happy => {}
            Mood::Serious => g.line(x(side * 3.5), y(-9.0), x(side * 10.0), y(-9.6), sz(0.45), 6),
            Mood::Annoyed => g.line(x(side * 3.0), y(-7.6), x(side * 10.5), y(-11.0), sz(0.6), 6),
            // One flat, one raised.
            Mood::Meh if side < 0.0 => g.line(x(side * 3.5), y(-8.6), x(side * 10.0), y(-9.0), sz(0.45), 6),
            Mood::Meh => g.line(x(side * 3.5), y(-11.0), x(side * 10.0), y(-11.6), sz(0.45), 6),
        }
    }
    // The bridge of the glasses, the nose, and the line under it.
    g.r(x(-2.0), y(-3.0), sz(4.0) + 1, t(0.6), 1);
    g.e(x(0.0), y(4.0), sz(2.6), sz(1.6), 0);
    g.r(x(0.0), y(5.0), t(0.5), sz(2.5), 6);
    match o.mood {
        Mood::Happy => {
            g.line(x(0.0), y(8.0), x(-3.4), y(7.2), sz(0.3), 6);
            g.line(x(0.0), y(8.0), x(3.4), y(7.2), sz(0.3), 6);
            g.e(x(0.0), y(9.6), sz(1.4), sz(0.9), 14);
        }
        Mood::Serious => g.r(x(-2.5), y(8.0), sz(5.0) + 1, t(0.5), 6),
        Mood::Meh => g.line(x(-2.5), y(8.8), x(3.0), y(7.6), sz(0.3), 6),
        Mood::Annoyed => {
            g.line(x(0.0), y(7.6), x(-3.2), y(9.0), sz(0.35), 6);
            g.line(x(0.0), y(7.6), x(3.2), y(9.0), sz(0.35), 6);
            // The cartoon vein.
            let (vx, vy, u) = (x(19.0), y(-19.0), t(1.0));
            for (dx, dy, w, h) in [(-3, -1, 2, 1), (1, -1, 2, 1), (-3, 1, 2, 1), (1, 1, 2, 1), (-1, -3, 1, 2), (1, -3, 1, 2), (-1, 2, 1, 2), (1, 2, 1, 2)] {
                g.r(vx + dx * u, vy + dy * u, w * u, h * u, 14);
            }
        }
    }
}

/// A conical flask of something green, `h` tall, standing at `base`.
fn flask(g: &mut Canvas, cx: i32, base: i32, h: i32, f: u32) {
    let hf = h as f32;
    let neck = round(hf * 0.4);
    let body = h - neck;
    let nw = round(hf * 0.11).max(2);
    let spread = round(hf * 0.42);
    g.r(cx - nw - 1, base - h, nw * 2 + 3, 1, 10);
    for i in 0..neck {
        g.p(cx - nw, base - h + 1 + i, 10);
        g.p(cx + nw, base - h + 1 + i, 10);
    }
    let level = body as f32 * 0.35;
    for i in 0..body {
        let half = nw + round(i as f32 * (hf * 0.42) / body as f32);
        let y = base - body + i;
        if i as f32 > level {
            g.r(cx - half, y, half * 2 + 1, 1, if i as f32 > level + 1.0 { 5 } else { 4 });
        }
        g.p(cx - half, y, 10);
        g.p(cx + half, y, 10);
    }
    g.r(cx - nw - spread, base, (nw + spread) * 2 + 1, 1, 10);
    // Bubbles, rising two pixels a frame.
    let reach = round(body as f32 * 0.6).max(4) as u32;
    for b in 0..3u32 {
        let up = (f * 2 + b * 5) % reach;
        g.p(cx - 3 + b as i32 * 3, base - 2 - up as i32, 10);
    }
}

/// A motherboard lying on the bench.
fn motherboard(g: &mut Canvas, x: i32, y: i32) {
    g.r(x, y, 124, 17, 4);
    g.r(x, y + 17, 124, 2, 3);
    for i in 0..10 {
        g.r(x + 4 + i * 12, y + 14, 8, 1, 5);
    }
    // The processor socket, three memory modules and the clock battery.
    g.r(x + 6, y + 3, 16, 10, 11);
    g.r(x + 8, y + 5, 12, 6, 12);
    for i in 0..3 {
        g.r(x + 28, y + 2 + i * 4, 40, 2, 0);
        g.r(x + 30, y + 2 + i * 4, 36, 1, 12);
    }
    g.e(x + 80, y + 7, 4, 4, 11);
    g.e(x + 80, y + 7, 2, 2, 10);
    // The flash chip the firmware is stored in, with a programmer's clip
    // on it and the clip's wires leaving the picture.
    g.r(x + 96, y + 5, 10, 7, 0);
    for i in 0..4 {
        g.p(x + 97 + i * 2, y + 4, 11);
        g.p(x + 97 + i * 2, y + 12, 11);
    }
    g.r(x + 95, y + 1, 12, 4, 14);
    g.r(x + 97, y - 2, 8, 3, 14);
    for (i, c) in [13u8, 5, 10, 14].into_iter().enumerate() {
        let i = i as i32;
        g.line(x + 98 + i * 2, y - 2, x + 150, y - 14 - i * 3, 0, c);
    }
}

/// The logo screen.
pub fn title(g: &mut Canvas, f: u32) {
    g.r(0, 0, 320, 200, 1);
    g.r(0, 0, 320, 22, 15);
    g.d(0, 22, 320, 12, 15);
    // The spotlight.
    g.ed(84, 104, 84, 84, 2);
    g.e(84, 104, 72, 72, 2);
    g.ed(84, 104, 58, 58, 3);
    g.e(84, 104, 46, 46, 3);
    // Twinkles on the wall.
    for (i, (x, y)) in [(150, 12), (236, 8), (300, 20), (20, 14), (306, 140)].into_iter().enumerate() {
        let on = (f + i as u32 * 3) % 12 < 8;
        g.p(x, y, if on { 13 } else { 12 });
        if on {
            g.p(x - 1, y, 9);
            g.p(x + 1, y, 9);
            g.p(x, y - 1, 9);
            g.p(x, y + 1, 9);
        }
    }
    weefy(
        g,
        84,
        94,
        2.0,
        &Pose {
            mood: Mood::Happy,
            arms: Arms::Wave,
            blink: f % 18 == 17,
            wave_up: f % 4 < 2,
            target: (0, 0),
            whole: false,
        },
    );
    // The bench in front of him, and what is on it.
    g.r(0, 158, 320, 2, 8);
    g.r(0, 160, 320, 12, 7);
    g.r(0, 172, 320, 16, 6);
    let mut x = 6;
    while x < 320 {
        g.r(x, 178 + (x % 3) * 2, 15, 1, 0);
        x += 37;
    }
    flask(g, 24, 166, 34, f);
    flask(g, 142, 167, 20, f + 3);
    motherboard(g, 176, 148);
    g.r(0, 187, 320, 1, 13);
    g.r(0, 188, 320, 12, 0);

    // "Weefy teaches UEFI, with Dr. Weefy Fulgens."
    g.title(166, 40, "WEEFY", 13, 6, 4);
    g.text(166, 75, "T E A C H E S", 9, 1);
    g.title(166, 88, "UEFI", 5, 3, 5);
    g.text(166, 130, "with DR. WEEFY FULGENS", 11, 1);
    if f % 6 < 4 {
        let line = "PRESS ANY KEY TO ENTER THE LAB";
        g.text((320 - text_width(line)) / 2, 190, line, 13, 1);
    }
}

/// What the lab's wall display is showing.
#[derive(Clone)]
pub enum Wall {
    /// The boot phases, with the one this program runs in lit.
    Phases,
    /// Looking for an operating system.
    Checking,
    /// One was found: where from, its name, its loader's file.
    Found { from: alloc::string::String, name: alloc::string::String, file: alloc::string::String },
    /// None was.
    Nothing,
}

/// The lab, with Weefy in a mood and something on the wall. The dialogue
/// box is drawn empty: the words are the caller's.
pub fn lab(g: &mut Canvas, f: u32, mood: Mood, wall: &Wall) {
    // Wall, skirting board, tiled floor.
    g.r(0, 0, 320, 110, 3);
    g.d(0, 78, 320, 32, 2);
    g.r(0, 110, 320, 4, 1);
    for y in 0..4 {
        for x in 0..20 {
            g.r(x * 16, 114 + y * 8, 16, 8, if (x + y) % 2 == 1 { 12 } else { 11 });
        }
    }
    // The wall display in its frame.
    g.r(114, 6, 196, 92, 6);
    g.r(116, 8, 192, 88, 7);
    g.r(119, 11, 186, 82, 0);
    match wall {
        Wall::Phases => {
            g.text(126, 16, "BOOT PHASES", 11, 1);
            for (i, (name, x)) in [("SEC", 126), ("PEI", 162), ("DXE", 198), ("BDS", 234), ("OS", 270)].into_iter().enumerate() {
                let x_text = x + (28 - text_width(name)) / 2 + 1;
                match i {
                    // The boot manager's phase: where this program runs.
                    3 => {
                        g.r(x, 30, 28, 16, if f % 4 < 3 { 5 } else { 4 });
                        g.text(x_text, 35, name, 0, 1);
                    }
                    4 => {
                        g.frame(x, 30, 28, 16, 12);
                        g.text(x_text, 35, name, 12, 1);
                    }
                    _ => {
                        g.frame(x, 30, 28, 16, 5);
                        g.text(x_text, 35, name, 5, 1);
                    }
                }
                if i < 4 {
                    g.r(x + 29, 38, 6, 1, 5);
                    g.p(x + 33, 37, 5);
                    g.p(x + 33, 39, 5);
                }
            }
            // A bracket under DXE and BDS.
            g.r(198, 50, 64, 1, 13);
            g.r(198, 48, 1, 3, 13);
            g.r(261, 48, 1, 3, 13);
            g.r(229, 51, 1, 3, 13);
            g.text(172, 57, "weefy runs here", 13, 1);
            // The line the firmware's services end at.
            let mut y = 24;
            while y < 78 {
                g.r(266, y, 1, 2, 14);
                y += 3;
            }
            g.text(196, 82, "ExitBootServices()", 14, 1);
        }
        Wall::Checking => {
            g.text(126, 16, "BOOT OPTIONS", 11, 1);
            g.text(126, 34, "BootOrder", 5, 1);
            g.text(126, 46, "\\EFI\\*", 5, 1);
            for i in 0..5 {
                g.r(190 + i * 10, 64, 6, 6, if (f + i as u32) % 5 == 0 { 5 } else { 12 });
            }
        }
        Wall::Found { from, name, file } => {
            g.text(126, 16, "BOOT OPTIONS", 11, 1);
            g.line(130, 44, 136, 50, 1, 5);
            g.line(136, 50, 148, 36, 1, 5);
            g.text(158, 34, from, 5, 1);
            g.text(158, 46, name, 10, 1);
            g.text(126, 70, file, 12, 1);
        }
        Wall::Nothing => {
            g.text(126, 16, "BOOT OPTIONS", 11, 1);
            g.line(130, 36, 146, 52, 1, 14);
            g.line(146, 36, 130, 52, 1, 14);
            g.text(158, 34, "BootOrder: nothing", 14, 1);
            g.text(158, 46, "\\EFI: no loader", 14, 1);
        }
    }
    g.r(114, 98, 196, 3, 6);
    // A bench with an oscilloscope, a flask and a chip.
    g.r(196, 124, 120, 4, 7);
    g.r(196, 128, 120, 2, 6);
    g.r(200, 130, 4, 16, 6);
    g.r(308, 130, 4, 16, 6);
    g.r(262, 104, 44, 20, 12);
    g.r(264, 106, 26, 16, 0);
    g.frame(264, 106, 26, 16, 11);
    for x in 0..24 {
        g.p(265 + x, 114 + round(4.0 * sin((x as f32 + f as f32 * 2.0) / 3.0)), 5);
    }
    g.e(297, 110, 2, 2, 13);
    g.e(297, 118, 2, 2, 14);
    g.p(301, 107, 5);
    flask(g, 224, 123, 18, f);
    g.r(238, 119, 14, 5, 0);
    for i in 0..4 {
        g.p(240 + i * 3, 118, 11);
    }
    weefy(
        g,
        58,
        50,
        1.4,
        &Pose {
            mood,
            arms: match mood {
                Mood::Happy => Arms::Wave,
                Mood::Serious => Arms::Point,
                _ => Arms::Down,
            },
            blink: f % 18 == 17,
            wave_up: f % 4 < 2,
            target: (124, 58),
            whole: true,
        },
    );
    // The dialogue box, his name on one tab and the way out on the other.
    g.r(4, 143, 312, 54, 0);
    g.r(5, 144, 310, 52, 1);
    g.frame(5, 144, 310, 52, 10);
    g.frame(7, 146, 306, 48, 12);
    g.r(12, 138, 62, 12, 0);
    g.r(13, 139, 60, 10, 13);
    g.text(17, 141, "DR. WEEFY", 0, 1);
    g.r(246, 138, 62, 12, 0);
    g.r(247, 139, 60, 10, 12);
    g.text(251, 141, "ESC SKIPS", 10, 1);
}

/// The mark that says a line is finished and a key will bring the next.
pub fn more(g: &mut Canvas) {
    g.r(300, 186, 7, 1, 13);
    g.r(301, 187, 5, 1, 13);
    g.r(302, 188, 3, 1, 13);
    g.p(303, 189, 13);
}
