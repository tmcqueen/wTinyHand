//! Software rendering with tiny-skia: the hand on its rod, ink strokes, laser dot, toolbar.
//!
//! The Wayland buffer is BGRA while tiny-skia works in RGBA. Rather than swapping a whole
//! screen of pixels every frame, every image is channel-swapped once at load and every
//! colour goes through `bgr()`, so tiny-skia unknowingly paints BGRA directly.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use tiny_skia::*;

pub const PURPLE: u32 = 0x8754F2;
pub const RED: u32 = 0xFF291F;
pub const LASER: &str = "laser";

/// id, title, ink colour. Same list and order as upstream's PointerStyle.
pub const STYLES: &[(&str, &str, u32)] = &[
    ("lavender", "Lavender toy", PURPLE),
    ("glove", "Cursor glove", PURPLE),
    ("coral", "Coral vinyl", RED),
    ("plush", "Plush", 0x2F86F0),
    ("robot", "Robot", 0x16A87A),
    ("pink", "Bubblegum", 0xEE4F98),
    ("blue", "Sky", 0x2F86F0),
    ("mint", "Mint", 0x16A87A),
    ("lemon", "Lemon", 0xF2B705),
    ("black", "Midnight", 0x1D1D21),
    ("chrome", "Chrome", 0x6B7380),
    ("holo", "Holo", PURPLE),
    ("fuzzy", "Fuzzy", PURPLE),
    ("wood", "Wood", 0x9A5E2C),
    ("marble", "Marble", 0x6B7380),
    ("terrazzo", "Terrazzo", 0xF0561D),
    ("glass", "Glass", 0x1597E5),
    ("jelly", "Jelly", 0xA33FE6),
    ("pastel", "Pastel", 0xEE4F98),
    ("pixel", "Pixel", 0x1D1D21),
    ("lowpoly", "Low poly", PURPLE),
    ("knit", "Knit", PURPLE),
    ("skeleton", "Skeleton", 0x9A5E2C),
    ("astro", "Astronaut", 0x2F86F0),
    ("space", "Space", RED),
    ("monster", "Monster", 0x3E9E2E),
    ("water", "Water", 0x1597E5),
    ("lava", "Lava", 0xF0561D),
    ("grass", "Grass", 0x3E9E2E),
    ("cactus", "Cactus", 0x3E9E2E),
    ("choco", "Chocolate", 0x9A5E2C),
    ("cloud", "Cloud", 0x2F86F0),
    ("mech", "Mech", 0xF0561D),
    ("neon", "Neon", 0xE0309A),
    ("leaf", "Leaf", 0x3E9E2E),
    ("cookie", "Gingerbread", 0x9A5E2C),
];
pub const DEFAULT_PINNED: &[&str] = &["lavender", "glove", "coral", "plush", "robot", LASER];

pub fn is_style(id: &str) -> bool {
    id == LASER || STYLES.iter().any(|s| s.0 == id)
}

pub fn ink_color(id: &str) -> u32 {
    STYLES.iter().find(|s| s.0 == id).map(|s| s.2).unwrap_or(RED)
}

fn bgr(hex: u32, alpha: f32) -> Color {
    let (r, g, b) = ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    Color::from_rgba8(b, g, r, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

fn paint(hex: u32, alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(bgr(hex, alpha));
    p.anti_alias = true;
    p
}

fn load_bgra(path: &std::path::Path) -> Option<Pixmap> {
    let mut pix = Pixmap::load_png(path).ok()?;
    for px in pix.data_mut().chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Some(pix)
}

/// Box-filter downscale by an integer factor.
fn shrink(src: &Pixmap, n: usize) -> Pixmap {
    let (w, h) = (src.width() as usize / n, src.height() as usize / n);
    let sw = src.width() as usize;
    let mut out = Pixmap::new(w as u32, h as u32).unwrap();
    let (s, d) = (src.data(), out.data_mut());
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0u32; 4];
            for yy in 0..n {
                for xx in 0..n {
                    let i = ((y * n + yy) * sw + x * n + xx) * 4;
                    for c in 0..4 {
                        acc[c] += s[i + c] as u32;
                    }
                }
            }
            let o = (y * w + x) * 4;
            for c in 0..4 {
                d[o + c] = (acc[c] / (n * n) as u32) as u8;
            }
        }
    }
    out
}

#[derive(Clone, Copy)]
pub struct Metrics {
    pub aspect: f32,
    pub hand_top: f32,
    pub tip: f32,
    pub wrist: f32,
    pub rod_end: f32,
    pub tip_x: f32,
    pub rod_x: f32,
}

/// A slice of the hand image (or its shadow) and the rectangle of the full image it covers,
/// as fractions of the image's width and height.
struct Part {
    pix: Pixmap,
    fx0: f32,
    fx1: f32,
    fy0: f32,
    fy1: f32,
    rod: bool,
}

fn part(
    src: &Pixmap,
    y0: usize,
    y1: usize,
    threshold: u8,
    rod: bool,
    fx: &dyn Fn(f32) -> f32,
    fy: &dyn Fn(f32) -> f32,
) -> Option<Part> {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let y1 = y1.min(h);
    let d = src.data();
    let (mut lo, mut hi) = (w, 0);
    for y in y0..y1 {
        for x in 0..w {
            if d[(y * w + x) * 4 + 3] > threshold {
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
    }
    if lo > hi {
        return None;
    }
    let lo = lo.saturating_sub(1);
    let hi = (hi + 2).min(w);
    let pw = hi - lo;
    let mut pix = Pixmap::new(pw as u32, (y1 - y0) as u32)?;
    for y in y0..y1 {
        let row = &d[(y * w + lo) * 4..(y * w + hi) * 4];
        let o = (y - y0) * pw * 4;
        pix.data_mut()[o..o + row.len()].copy_from_slice(row);
    }
    Some(Part { pix, fx0: fx(lo as f32), fx1: fx(hi as f32), fy0: fy(y0 as f32), fy1: fy(y1 as f32), rod })
}

pub struct Hand {
    pub m: Metrics,
    parts: Vec<Part>,
    shadows: Vec<Part>,
}

pub struct Library {
    dir: PathBuf,
    metrics: HashMap<String, Metrics>,
    shadow_div: f32,
    shadow_pad: f32,
    hands: HashMap<String, Hand>,
    thumbs: HashMap<String, Pixmap>,
}

impl Library {
    pub fn open(dir: PathBuf) -> Result<Library, String> {
        let text = std::fs::read_to_string(dir.join("hands.json"))
            .map_err(|e| format!("can't read {}: {e}", dir.join("hands.json").display()))?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let mut metrics = HashMap::new();
        if let Some(hands) = v["hands"].as_object() {
            for (id, h) in hands {
                let f = |k: &str| h[k].as_f64().unwrap_or(0.0) as f32;
                metrics.insert(
                    id.clone(),
                    Metrics {
                        aspect: f("aspect"),
                        hand_top: f("handTop"),
                        tip: f("tip"),
                        wrist: f("wrist"),
                        rod_end: f("rodEnd"),
                        tip_x: f("tipX"),
                        rod_x: f("rodX"),
                    },
                );
            }
        }
        Ok(Library {
            dir,
            metrics,
            shadow_div: v["shadowDiv"].as_f64().unwrap_or(4.0) as f32,
            shadow_pad: v["shadowPad"].as_f64().unwrap_or(16.0) as f32,
            hands: HashMap::new(),
            thumbs: HashMap::new(),
        })
    }

    pub fn tap_sound(&self) -> PathBuf {
        self.dir.join("tap.wav")
    }

    pub fn icon(&self, size: usize) -> Option<(usize, Vec<u8>)> {
        let full = Pixmap::load_png(self.dir.join("icon.png")).ok()?;
        let small = shrink(&full, (full.width() as usize / size).max(1));
        let mut argb = Vec::with_capacity(small.data().len());
        for px in small.pixels() {
            let c = px.demultiply();
            argb.extend_from_slice(&[c.alpha(), c.red(), c.green(), c.blue()]);
        }
        Some((small.width() as usize, argb))
    }

    pub fn load_thumbs(&mut self) {
        if !self.thumbs.is_empty() {
            return;
        }
        for (id, _, _) in STYLES {
            if let Some(p) = load_bgra(&self.dir.join(format!("thumbs/{id}.png"))) {
                self.thumbs.insert(id.to_string(), shrink(&p, 2));
            }
        }
    }

    pub fn hand(&mut self, id: &str) -> Option<&Hand> {
        if !self.hands.contains_key(id) {
            let hand = self.load(id)?;
            self.hands.clear(); // keep only the current one in memory
            self.hands.insert(id.to_string(), hand);
        }
        self.hands.get(id)
    }

    fn load(&self, id: &str) -> Option<Hand> {
        let m = *self.metrics.get(id)?;
        let img = load_bgra(&self.dir.join(format!("hands/{id}.png")))?;
        let (w, h) = (img.width() as f32, img.height() as f32);
        let wrist = (m.wrist * h).round() as usize;
        let rod_end = (m.rod_end * h).round() as usize;
        let fx = |x: f32| x / w;
        let fy = |y: f32| y / h;
        let mut parts = Vec::new();
        // The hand overlaps the rod by two rows so no seam shows where they meet.
        parts.extend(part(&img, wrist, rod_end, 0, true, &fx, &fy));
        parts.extend(part(&img, (m.hand_top * h) as usize, wrist + 2, 0, false, &fx, &fy));

        let mut shadows = Vec::new();
        if let Some(sh) = load_bgra(&self.dir.join(format!("shadows/{id}.png"))) {
            let (div, pad) = (self.shadow_div, self.shadow_pad);
            let sx = |x: f32| (x - pad) * div / w;
            let sy = |y: f32| (y - pad) * div / h;
            let row = |f: f32| (pad + f * h / div).round() as usize;
            shadows.extend(part(&sh, row(m.wrist), row(m.rod_end), 2, true, &sx, &sy));
            shadows.extend(part(&sh, 0, row(m.wrist), 2, false, &sx, &sy));
        }
        Some(Hand { m, parts, shadows })
    }
}

/// Where the arm is this frame, in buffer pixels (y down).
pub struct Pose {
    rot: f32,
    rod_len: f32,
    s: f32,
    width: f32,
    gx: f32,
    gy: f32,
    lift: f32,
}

pub fn pose(m: &Metrics, cursor: (f32, f32), grip_x: f32, h: f32, size: f32, press: f32) -> Pose {
    // Worked out in y-up coordinates like upstream, with the grip below the bottom edge.
    let grip_y = -0.12 * h;
    let (dx, dy) = (cursor.0 - grip_x, (h - cursor.1) - grip_y);
    let reach = dx.hypot(dy);
    let aim = dy.atan2(dx);
    let hand_len = size * h * (1.0 - 0.06 * press);
    let s = hand_len / (m.wrist - m.hand_top);
    let width = s * m.aspect;
    let hand_tip = (m.wrist - m.tip) * s;
    let tip_off = (m.tip_x - m.rod_x) * width;
    let along = (reach * reach - tip_off * tip_off).max(0.0).sqrt();
    let rod_len = (along - hand_tip).max(0.05 * h);
    let tip_angle = (rod_len + hand_tip).atan2(tip_off);
    Pose { rot: aim - tip_angle, rod_len, s, width, gx: grip_x, gy: h * 1.12, lift: 0.06 * h * (1.0 - 0.8 * press) }
}

fn draw_part(pm: &mut PixmapMut, p: &Part, m: &Metrics, pose: &Pose, off: (f32, f32), opacity: f32, shadow: bool) {
    // Arm-local coordinates: origin at the grip, y pointing up the rod. Above the wrist the
    // image keeps its proportions; below it, the rod stretches to whatever length is needed.
    let on_rod = |fy: f32| pose.rod_len * (m.rod_end - fy) / (m.rod_end - m.wrist);
    let on_hand = |fy: f32| pose.rod_len + (m.wrist - fy) * pose.s;
    let local_y = |fy: f32| {
        // Shadow slices are cut at whole pixel rows, a hair off the wrist, so map each edge by
        // which side of the wrist it is really on; otherwise the two slices leave a gap.
        let rod = if shadow { fy > m.wrist } else { p.rod };
        if rod { on_rod(fy) } else { on_hand(fy) }
    };
    let x0 = (p.fx0 - m.rod_x) * pose.width;
    let x1 = (p.fx1 - m.rod_x) * pose.width;
    // The image's rod slice tucks a pixel under the hand; shadow slices must meet exactly,
    // since any overlap of two translucent slices shows as a dark band.
    let top = local_y(p.fy0) + if p.rod && !shadow { 1.0 } else { 0.0 };
    let bottom = local_y(p.fy1);
    let kx = (x1 - x0) / p.pix.width() as f32;
    let ky = (top - bottom) / p.pix.height() as f32;
    // local -> device: x' = a*x + c*y + e, y' = b*x + d*y + f (rotate, then flip y).
    let (sin, cos) = pose.rot.sin_cos();
    let (a, b, c, d) = (cos, -sin, -sin, -cos);
    let (e, f) = (pose.gx + off.0, pose.gy + off.1);
    let t = Transform::from_row(a * kx, b * kx, -c * ky, -d * ky, a * x0 + c * top + e, b * x0 + d * top + f);
    let pp = PixmapPaint { opacity, blend_mode: BlendMode::SourceOver, quality: FilterQuality::Bilinear };
    pm.draw_pixmap(0, 0, p.pix.as_ref(), &pp, t, None);
}

pub fn draw_hand(pm: &mut PixmapMut, hand: &Hand, pose: &Pose, shadow: bool) {
    if shadow {
        for p in &hand.shadows {
            draw_part(pm, p, &hand.m, pose, (pose.lift * 0.6, pose.lift), 0.3, true);
        }
    }
    for p in &hand.parts {
        draw_part(pm, p, &hand.m, pose, (0.0, 0.0), 1.0, false);
    }
}

pub fn draw_laser(pm: &mut PixmapMut, c: (f32, f32), h: f32, size: f32, press: f32) {
    let r = (h * 0.0038 * size / 0.13).max(4.0) * (1.0 + 0.2 * press);
    let glow = r * (3.4 + press);
    let center = Point::from_xy(c.0, c.1);
    let stops = vec![
        GradientStop::new(0.0, bgr(RED, 0.75)),
        GradientStop::new(0.35, bgr(RED, 0.4)),
        GradientStop::new(1.0, bgr(RED, 0.0)),
    ];
    if let (Some(shader), Some(path)) = (
        RadialGradient::new(center, center, glow, stops, SpreadMode::Pad, Transform::identity()),
        PathBuilder::from_circle(c.0, c.1, glow),
    ) {
        let p = Paint { shader, anti_alias: true, ..Paint::default() };
        pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
    }
    if let Some(path) = PathBuilder::from_circle(c.0, c.1, r) {
        pm.fill_path(&path, &paint(RED, 1.0), FillRule::Winding, Transform::identity(), None);
    }
    if let Some(path) = PathBuilder::from_circle(c.0, c.1, r * 0.45) {
        pm.fill_path(&path, &paint(0xFFDBD1, 0.95), FillRule::Winding, Transform::identity(), None);
    }
}

// ---------------------------------------------------------------- ink

pub struct Stroke {
    pub pts: Vec<(f32, f32)>,
    pub color: u32,
    pub width: f32,
    pub arrow: bool,
    pub ended: Option<Instant>,
}

const FADE: f32 = 0.6;

impl Stroke {
    /// 1 while drawing and during the hold, easing to 0 over the fade; None once gone.
    pub fn alpha(&self, now: Instant, hold: f32) -> Option<f32> {
        let Some(end) = self.ended else { return Some(1.0) };
        let t = (now.duration_since(end).as_secs_f32() - hold) / FADE;
        if t >= 1.0 {
            None
        } else {
            Some(1.0 - t.max(0.0).powi(2))
        }
    }

    fn path(&self) -> Option<Path> {
        let pts = &self.pts;
        let first = *pts.first()?;
        let mut pb = PathBuilder::new();
        pb.move_to(first.0, first.1);
        for i in 1..pts.len() {
            let (prev, p) = (pts[i - 1], pts[i]);
            pb.quad_to(prev.0, prev.1, (prev.0 + p.0) / 2.0, (prev.1 + p.1) / 2.0);
        }
        let end = *pts.last()?;
        pb.line_to(end.0, end.1);
        if self.arrow {
            // Aim the head along the last stretch, so a wobble at the very end doesn't twist it.
            let size = (self.width * 3.2).max(14.0);
            let dist = |p: &(f32, f32)| (end.0 - p.0).hypot(end.1 - p.1);
            let back = pts.iter().rev().find(|p| dist(p) >= size * 1.2).copied().unwrap_or(first);
            let len = dist(&back);
            if len > 1.0 {
                let head = size.min(len * 0.8);
                let angle = (end.1 - back.1).atan2(end.0 - back.0);
                for side in [-1.0f32, 1.0] {
                    let a = angle + std::f32::consts::PI + side * std::f32::consts::PI / 7.0;
                    pb.move_to(end.0, end.1);
                    pb.line_to(end.0 + a.cos() * head, end.1 + a.sin() * head);
                }
            }
        }
        pb.finish()
    }

    pub fn draw(&self, pm: &mut PixmapMut, alpha: f32, scale: f32) {
        let Some(path) = self.path() else { return };
        let stroke = tiny_skia::Stroke {
            width: self.width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        let shadow = Transform::from_translate(scale, 1.5 * scale);
        pm.stroke_path(&path, &paint(0, 0.18 * alpha), &stroke, shadow, None);
        pm.stroke_path(&path, &paint(self.color, alpha), &stroke, Transform::identity(), None);
    }
}

// ---------------------------------------------------------------- toolbar

#[derive(Clone, PartialEq, Debug)]
pub enum Action {
    Style(String),
    More,
    Through,
    Sound,
    Exit,
}

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x && p.0 < self.x + self.w && p.1 >= self.y && p.1 < self.y + self.h
    }
    fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

pub struct Layout {
    pub bar: Rect,
    pub grid: Option<Rect>,
    pub items: Vec<(Rect, Action)>,
}

impl Layout {
    pub fn new(w: f32, h: f32, k: f32, pinned: &[String], popup: bool) -> Layout {
        let (btn, gap, pad, sep) = (44.0 * k, 4.0 * k, 8.0 * k, 10.0 * k);
        let mut actions: Vec<Action> = pinned.iter().map(|s| Action::Style(s.clone())).collect();
        actions.extend([Action::More, Action::Through, Action::Sound, Action::Exit]);
        let n = actions.len() as f32;
        let bar_w = 2.0 * pad + n * btn + (n - 1.0) * gap + sep;
        let bar = Rect { x: ((w - bar_w) / 2.0).round(), y: (h - btn - 2.0 * pad - 28.0 * k).round(), w: bar_w, h: btn + 2.0 * pad };
        let mut items = Vec::new();
        let mut x = bar.x + pad;
        for a in actions {
            if a == Action::Through {
                x += sep;
            }
            items.push((Rect { x, y: bar.y + pad, w: btn, h: btn }, a));
            x += btn + gap;
        }
        let grid = popup.then(|| {
            let cols = 9usize;
            let rows = STYLES.len().div_ceil(cols);
            let gw = 2.0 * pad + cols as f32 * btn + (cols as f32 - 1.0) * gap;
            let gh = 2.0 * pad + rows as f32 * btn + (rows as f32 - 1.0) * gap;
            let g = Rect { x: ((w - gw) / 2.0).round(), y: bar.y - gh - 10.0 * k, w: gw, h: gh };
            for (i, (id, _, _)) in STYLES.iter().enumerate() {
                let (c, r) = ((i % cols) as f32, (i / cols) as f32);
                let rect = Rect { x: g.x + pad + c * (btn + gap), y: g.y + pad + r * (btn + gap), w: btn, h: btn };
                items.push((rect, Action::Style(id.to_string())));
            }
            g
        });
        Layout { bar, grid, items }
    }

    pub fn over_ui(&self, p: (f32, f32)) -> bool {
        self.bar.contains(p) || self.grid.is_some_and(|g| g.contains(p))
    }

    pub fn hit(&self, p: (f32, f32)) -> Option<usize> {
        self.items.iter().position(|(r, _)| r.contains(p))
    }
}

fn rrect(r: Rect, rad: f32) -> Option<Path> {
    let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
    let k = rad * 0.5523;
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + rad, y0);
    pb.line_to(x1 - rad, y0);
    pb.cubic_to(x1 - rad + k, y0, x1, y0 + rad - k, x1, y0 + rad);
    pb.line_to(x1, y1 - rad);
    pb.cubic_to(x1, y1 - rad + k, x1 - rad + k, y1, x1 - rad, y1);
    pb.line_to(x0 + rad, y1);
    pb.cubic_to(x0 + rad - k, y1, x0, y1 - rad + k, x0, y1 - rad);
    pb.line_to(x0, y0 + rad);
    pb.cubic_to(x0, y0 + rad - k, x0 + rad - k, y0, x0 + rad, y0);
    pb.close();
    pb.finish()
}

fn fill(pm: &mut PixmapMut, path: Option<Path>, hex: u32, alpha: f32) {
    if let Some(path) = path {
        pm.fill_path(&path, &paint(hex, alpha), FillRule::Winding, Transform::identity(), None);
    }
}

fn line(pm: &mut PixmapMut, path: Option<Path>, hex: u32, width: f32) {
    if let Some(path) = path {
        let s = tiny_skia::Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
        pm.stroke_path(&path, &paint(hex, 1.0), &s, Transform::identity(), None);
    }
}

pub struct ToolbarState<'a> {
    pub style: &'a str,
    pub sound_on: bool,
    pub through: bool,
    pub popup: bool,
    pub hover: Option<usize>,
}

pub fn draw_toolbar(pm: &mut PixmapMut, lib: &Library, layout: &Layout, st: &ToolbarState, k: f32) {
    const BG: u32 = 0x1C1C20;
    const FG: u32 = 0xF2F2F5;
    fill(pm, rrect(layout.bar, 16.0 * k), BG, 0.92);
    if let Some(g) = layout.grid {
        fill(pm, rrect(g, 16.0 * k), BG, 0.92);
    }
    for (i, (r, action)) in layout.items.iter().enumerate() {
        let selected = match action {
            Action::Style(id) => id == st.style,
            Action::More => st.popup,
            Action::Through => st.through,
            _ => false,
        };
        if selected {
            fill(pm, rrect(*r, 10.0 * k), 0xFFFFFF, 0.22);
        } else if st.hover == Some(i) {
            fill(pm, rrect(*r, 10.0 * k), 0xFFFFFF, 0.10);
        }
        let (cx, cy) = r.center();
        let at = |dx: f32, dy: f32| (cx + dx * k, cy + dy * k);
        match action {
            Action::Style(id) if id == LASER => {
                fill(pm, PathBuilder::from_circle(cx, cy, 11.0 * k), RED, 0.3);
                fill(pm, PathBuilder::from_circle(cx, cy, 6.5 * k), RED, 1.0);
                fill(pm, PathBuilder::from_circle(cx, cy, 2.8 * k), 0xFFDBD1, 0.95);
            }
            Action::Style(id) => {
                if let Some(t) = lib.thumbs.get(id) {
                    let side = r.w - 6.0 * k;
                    let s = side / t.width() as f32;
                    let tf = Transform::from_row(s, 0.0, 0.0, s, cx - side / 2.0, cy - side / 2.0);
                    let pp = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
                    pm.draw_pixmap(0, 0, t.as_ref(), &pp, tf, None);
                }
            }
            Action::More => {
                for dx in [-8.0, 0.0, 8.0] {
                    let (x, y) = at(dx, 0.0);
                    fill(pm, PathBuilder::from_circle(x, y, 2.4 * k), FG, 1.0);
                }
            }
            Action::Through => {
                // An ordinary arrow cursor: clicks go to the apps underneath.
                let shape = [(-6.0, -9.5), (-6.0, 7.0), (-2.2, 3.6), (0.8, 9.8), (3.4, 8.6), (0.5, 2.6), (5.8, 2.6)];
                let mut pb = PathBuilder::new();
                for (i, (dx, dy)) in shape.iter().enumerate() {
                    let (x, y) = at(*dx, *dy);
                    if i == 0 {
                        pb.move_to(x, y)
                    } else {
                        pb.line_to(x, y)
                    }
                }
                pb.close();
                fill(pm, pb.finish(), FG, 1.0);
            }
            Action::Sound => {
                let mut pb = PathBuilder::new();
                let body = [(-10.0, -4.0), (-5.0, -4.0), (1.0, -9.0), (1.0, 9.0), (-5.0, 4.0), (-10.0, 4.0)];
                for (i, (dx, dy)) in body.iter().enumerate() {
                    let (x, y) = at(*dx, *dy);
                    if i == 0 {
                        pb.move_to(x, y)
                    } else {
                        pb.line_to(x, y)
                    }
                }
                pb.close();
                fill(pm, pb.finish(), FG, 1.0);
                let mut pb = PathBuilder::new();
                if st.sound_on {
                    for (x, y, c) in [(5.0, 4.5, 8.5), (8.5, 8.0, 14.5)] {
                        let (a, b, ctl) = (at(x, -y), at(x, y), at(c, 0.0));
                        pb.move_to(a.0, a.1);
                        pb.quad_to(ctl.0, ctl.1, b.0, b.1);
                    }
                } else {
                    for (a, b) in [(at(5.5, -4.0), at(13.0, 4.0)), (at(13.0, -4.0), at(5.5, 4.0))] {
                        pb.move_to(a.0, a.1);
                        pb.line_to(b.0, b.1);
                    }
                }
                line(pm, pb.finish(), FG, 1.8 * k);
            }
            Action::Exit => {
                let mut pb = PathBuilder::new();
                for (a, b) in [(at(-6.0, -6.0), at(6.0, 6.0)), (at(6.0, -6.0), at(-6.0, 6.0))] {
                    pb.move_to(a.0, a.1);
                    pb.line_to(b.0, b.1);
                }
                line(pm, pb.finish(), FG, 2.2 * k);
            }
        }
    }
}
