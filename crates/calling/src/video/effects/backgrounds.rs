//! The bundled backgrounds, drawn here at any size (no picture files): gradients and plain
//! colours, soft abstract shapes, bokeh, geometric patterns and a few simple rooms (an office, a
//! bookshelf, a living room, a home office, a loft, a studio).

use crate::video::RgbFrame;

mod extra;

/// A background that comes with IronTeams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltIn {
    /// Remembered in the settings.
    pub id: &'static str,
    pub name: &'static str,
}

pub const BUILT_IN: [BuiltIn; 26] = [
    BuiltIn {
        id: "dusk",
        name: "Dusk",
    },
    BuiltIn {
        id: "aurora",
        name: "Aurora",
    },
    BuiltIn {
        id: "office",
        name: "Bright office",
    },
    BuiltIn {
        id: "bookshelf",
        name: "Bookshelf",
    },
    BuiltIn {
        id: "sage",
        name: "Sage",
    },
    BuiltIn {
        id: "slate",
        name: "Slate",
    },
    b("sunrise", "Sunrise"),
    b("ocean", "Ocean"),
    b("forest-mist", "Forest mist"),
    b("lavender", "Lavender haze"),
    b("midnight", "Midnight"),
    b("peach", "Peach"),
    b("mint", "Mint"),
    b("graphite", "Graphite"),
    b("bokeh-warm", "Warm bokeh"),
    b("bokeh-cool", "Cool bokeh"),
    b("city-lights", "City lights"),
    b("stripes", "Stripes"),
    b("mosaic", "Mosaic"),
    b("rings", "Rings"),
    b("waves", "Waves"),
    b("dots", "Dots"),
    b("living-room", "Living room"),
    b("home-office", "Home office"),
    b("loft", "Loft"),
    b("studio", "Studio"),
];

const fn b(id: &'static str, name: &'static str) -> BuiltIn {
    BuiltIn { id, name }
}

type Rgb = [f32; 3];

fn hex(c: u32) -> Rgb {
    [(c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff].map(|v| v as f32)
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

fn shade(c: Rgb, k: f32) -> Rgb {
    c.map(|v| v * k)
}

/// A picture being drawn: coordinates are fractions of the width and height.
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<Rgb>,
}

impl Canvas {
    fn new(w: usize, h: usize, paint: impl Fn(f32, f32) -> Rgb) -> Canvas {
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                px.push(paint(
                    (x as f32 + 0.5) / w as f32,
                    (y as f32 + 0.5) / h as f32,
                ));
            }
        }
        Canvas { w, h, px }
    }

    /// Fills the rectangle `x0..x1` by `y0..y1` with what `paint` gives at each point.
    fn rect(&mut self, (x0, y0, x1, y1): (f32, f32, f32, f32), paint: impl Fn(f32, f32) -> Rgb) {
        let span = |a: f32, b: f32, n: usize| {
            let lo = ((a * n as f32).round().max(0.0) as usize).min(n);
            let hi = ((b * n as f32).round().max(0.0) as usize).min(n);
            lo..hi
        };
        for y in span(y0, y1, self.h) {
            for x in span(x0, x1, self.w) {
                self.px[y * self.w + x] = paint(
                    (x as f32 + 0.5) / self.w as f32,
                    (y as f32 + 0.5) / self.h as f32,
                );
            }
        }
    }

    /// A filled circle around `(cx, cy)` with radius `r` (a fraction of the height).
    fn circle(&mut self, (cx, cy): (f32, f32), r: f32, c: Rgb) {
        let aspect = self.w as f32 / self.h.max(1) as f32;
        let rx = r / aspect;
        let inside = |x: f32, y: f32| ((x - cx) * aspect).powi(2) + (y - cy).powi(2) <= r * r;
        let (w, h) = (self.w, self.h);
        for y in 0..h {
            let fy = (y as f32 + 0.5) / h as f32;
            if (fy - cy).abs() > r {
                continue;
            }
            for x in 0..w {
                let fx = (x as f32 + 0.5) / w as f32;
                if (fx - cx).abs() <= rx && inside(fx, fy) {
                    self.px[y * w + x] = c;
                }
            }
        }
    }

    /// A disc around `(cx, cy)` with radius `r` (a fraction of the height) laid over what is
    /// there with `alpha`.
    fn disc(&mut self, (cx, cy): (f32, f32), r: f32, c: Rgb, alpha: f32) {
        let aspect = self.w as f32 / self.h.max(1) as f32;
        let from = |v: f32, n: usize| (v * n as f32).floor().max(0.0) as usize;
        let to = |v: f32, n: usize| ((v * n as f32).ceil().max(0.0) as usize).min(n);
        for y in from(cy - r, self.h)..to(cy + r, self.h) {
            let fy = (y as f32 + 0.5) / self.h as f32;
            for x in from(cx - r / aspect, self.w)..to(cx + r / aspect, self.w) {
                let fx = (x as f32 + 0.5) / self.w as f32;
                if ((fx - cx) * aspect).powi(2) + (fy - cy).powi(2) <= r * r {
                    let p = &mut self.px[y * self.w + x];
                    *p = mix(*p, c, alpha);
                }
            }
        }
    }

    fn into_frame(self) -> RgbFrame {
        let data = self
            .px
            .iter()
            .flat_map(|p| p.map(|v| v.round().clamp(0.0, 255.0) as u8))
            .collect();
        RgbFrame {
            width: self.w,
            height: self.h,
            data,
        }
    }
}

/// Darker towards the corners.
fn vignette(x: f32, y: f32) -> f32 {
    let d = ((x - 0.5).powi(2) + (y - 0.5).powi(2)).sqrt();
    1.0 - 0.35 * (d / 0.71).powi(2)
}

/// A pseudo-random number in 0..1 for `n` (the same every time).
fn noise(n: u32) -> f32 {
    let mut v = n.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
    v ^= v >> 15;
    v = v.wrapping_mul(0x2c1b_3c6d);
    v ^= v >> 12;
    (v & 0xffff) as f32 / 65535.0
}

fn dusk(w: usize, h: usize) -> Canvas {
    let (top, bottom, glow) = (hex(0x2b2f63), hex(0xf0a07c), hex(0xffd6a5));
    Canvas::new(w, h, |x, y| {
        let base = mix(top, bottom, y * 0.9 + x * 0.1);
        let d = ((x - 0.72) * 1.6).powi(2) + (y - 0.7).powi(2);
        mix(base, glow, 0.55 * (-d * 9.0).exp())
    })
}

fn aurora(w: usize, h: usize) -> Canvas {
    let (a, b, band) = (hex(0x0f5e63), hex(0x4c2a85), hex(0x6ee7b7));
    Canvas::new(w, h, |x, y| {
        let base = mix(a, b, (x + y) / 2.0);
        let wave = 0.38 + 0.08 * (x * 7.0).sin() + 0.05 * (x * 13.0 + 1.0).sin();
        let ribbon = (-((y - wave) * 9.0).powi(2)).exp();
        mix(base, band, 0.45 * ribbon)
    })
}

fn office(w: usize, h: usize) -> Canvas {
    let (wall_top, wall, floor) = (hex(0xf2eee8), hex(0xe2dbd0), hex(0xb9a68e));
    let mut c = Canvas::new(w, h, |_, y| {
        if y > 0.8 {
            mix(floor, shade(floor, 0.85), (y - 0.8) * 5.0)
        } else {
            mix(wall_top, wall, y / 0.8)
        }
    });
    // A window with light coming in, and its frame.
    let (sky_top, sky) = (hex(0xcfe4f2), hex(0xa9cbe3));
    c.rect((0.07, 0.1, 0.43, 0.64), |_, y| {
        mix(sky_top, sky, (y - 0.1) / 0.54)
    });
    let frame = hex(0xfbfaf7);
    for x in [0.07, 0.245, 0.43] {
        c.rect((x - 0.006, 0.1, x + 0.006, 0.64), |_, _| frame);
    }
    for y in [0.1, 0.37, 0.64] {
        c.rect((0.07, y - 0.01, 0.43, y + 0.01), |_, _| frame);
    }
    // A low cabinet with a plant on it.
    let wood = hex(0x9a7b5c);
    c.rect((0.6, 0.62, 0.95, 0.8), |_, y| {
        shade(wood, 1.0 - (y - 0.62) * 0.8)
    });
    c.rect((0.6, 0.62, 0.95, 0.635), |_, _| shade(wood, 1.15));
    let pot = hex(0x6b4f3a);
    c.rect((0.79, 0.53, 0.85, 0.62), |_, _| pot);
    for (dx, dy, r, k) in [
        (0.0, -0.05, 0.075, 1.0),
        (-0.035, 0.0, 0.06, 0.9),
        (0.035, -0.01, 0.06, 0.85),
        (0.01, -0.11, 0.05, 1.08),
    ] {
        c.circle((0.82 + dx, 0.46 + dy), r, shade(hex(0x5a8a5c), k));
    }
    // A picture on the wall.
    c.rect((0.62, 0.2, 0.8, 0.44), |_, _| hex(0xfbfaf7));
    c.rect((0.635, 0.225, 0.785, 0.415), |x, y| {
        mix(
            hex(0xe9b384),
            hex(0x7aa6a1),
            (x - 0.635) / 0.15 + (y - 0.225),
        )
    });
    c
}

fn bookshelf(w: usize, h: usize) -> Canvas {
    let back = hex(0x3a2a20);
    let mut c = Canvas::new(w, h, |x, y| shade(back, vignette(x, y) * 1.1));
    let board = hex(0x6a4c37);
    let palette = [
        0x8c3b3b, 0x2f5d62, 0xc9a45c, 0x4a5a85, 0x6f8f5a, 0xb86b4b, 0x7a4e7a, 0xd8cfc0, 0x34495e,
    ]
    .map(hex);
    let shelves = [0.06, 0.37, 0.68];
    for (s, &top) in shelves.iter().enumerate() {
        let bottom = top + 0.27;
        let mut x = 0.03;
        let mut n = s as u32 * 97;
        while x < 0.97 {
            n += 1;
            let width = 0.014 + 0.022 * noise(n);
            if noise(n + 500) < 0.08 {
                // A gap now and then.
                x += width * 1.5;
                continue;
            }
            let tall = 0.6 + 0.38 * noise(n + 1000);
            let colour = palette[(noise(n + 2000) * palette.len() as f32) as usize % palette.len()];
            let lean = noise(n + 3000) * 0.15;
            c.rect(
                (
                    x,
                    bottom - (bottom - top) * tall,
                    (x + width).min(0.97),
                    bottom,
                ),
                |_, y| shade(colour, 0.85 + lean + 0.1 * (y - top)),
            );
            x += width + 0.002;
        }
        c.rect((0.0, bottom, 1.0, bottom + 0.035), |_, y| {
            shade(board, 1.1 - (y - bottom) * 6.0)
        });
    }
    c
}

fn plain(w: usize, h: usize, colour: u32) -> Canvas {
    let c = hex(colour);
    Canvas::new(w, h, move |x, y| shade(c, vignette(x, y)))
}

/// The built-in background `id` drawn at `w` x `h`; `None` for an unknown id.
pub fn built_in(id: &str, w: usize, h: usize) -> Option<RgbFrame> {
    let canvas = match id {
        "dusk" => dusk(w, h),
        "aurora" => aurora(w, h),
        "office" => office(w, h),
        "bookshelf" => bookshelf(w, h),
        "sage" => plain(w, h, 0x9fb28c),
        "slate" => plain(w, h, 0x4d5b6e),
        "sunrise" => extra::sunrise(w, h),
        "ocean" => extra::ocean(w, h),
        "forest-mist" => extra::forest_mist(w, h),
        "lavender" => extra::lavender(w, h),
        "midnight" => extra::midnight(w, h),
        "peach" => extra::diagonal(w, h, 0xffd9c0, 0xff9e8f),
        "mint" => extra::diagonal(w, h, 0xd4f5e4, 0x4db6a4),
        "graphite" => extra::graphite(w, h),
        "bokeh-warm" => extra::bokeh(w, h, (0x3b1d1a, 0x1c1214), &extra::WARM, 38),
        "bokeh-cool" => extra::bokeh(w, h, (0x0e2a4a, 0x14183a), &extra::COOL, 38),
        "city-lights" => extra::bokeh(w, h, (0x101a33, 0x2a2140), &extra::CITY, 55),
        "stripes" => extra::stripes(w, h),
        "mosaic" => extra::mosaic(w, h),
        "rings" => extra::rings(w, h),
        "waves" => extra::waves(w, h),
        "dots" => extra::dots(w, h),
        "living-room" => extra::living_room(w, h),
        "home-office" => extra::home_office(w, h),
        "loft" => extra::loft(w, h),
        "studio" => extra::studio(w, h),
        _ => return None,
    };
    Some(canvas.into_frame())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_background_draws_at_the_size_asked() {
        for b in BUILT_IN {
            let f = built_in(b.id, 64, 36).unwrap();
            assert_eq!((f.width, f.height, f.data.len()), (64, 36, 64 * 36 * 3));
            // Not one flat colour (the plain ones have a vignette).
            let first = &f.data[..3];
            assert!(f.data.chunks(3).any(|p| p != first), "{}", b.id);
        }
        assert!(built_in("nope", 8, 8).is_none());
    }

    #[test]
    fn the_backgrounds_differ_and_are_drawn_the_same_each_time() {
        let a = built_in("bookshelf", 32, 18).unwrap();
        assert_eq!(a, built_in("bookshelf", 32, 18).unwrap());
        assert_ne!(a, built_in("office", 32, 18).unwrap());
        let ids: Vec<_> = BUILT_IN.iter().map(|b| b.id).collect();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(ids.len(), unique.len(), "an id twice");
        let mut names: Vec<_> = BUILT_IN.iter().map(|b| b.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), BUILT_IN.len(), "a name twice");
    }

    #[test]
    fn there_are_twenty_six_and_none_borrows_a_reserved_id() {
        assert_eq!(BUILT_IN.len(), 26);
        for b in BUILT_IN {
            assert!(!b.name.is_empty());
            for reserved in ["none", "blur", "strong-blur"] {
                assert_ne!(b.id, reserved);
            }
            assert!(
                !b.id.contains(':'),
                "{}: a prefix with a colon means a picture file",
                b.id
            );
        }
    }

    #[test]
    fn the_new_backgrounds_are_pictures_not_flat_and_differ_from_each_other() {
        let drawn: Vec<_> = BUILT_IN
            .iter()
            .map(|b| (b.id, built_in(b.id, 96, 54).unwrap()))
            .collect();
        for (i, (id, a)) in drawn.iter().enumerate() {
            for (other, b) in &drawn[i + 1..] {
                assert_ne!(a, b, "{id} and {other} are the same picture");
            }
        }
    }

    #[test]
    fn a_background_draws_at_the_size_the_camera_uses() {
        let f = built_in("living-room", 1280, 720).unwrap();
        assert_eq!(
            (f.width, f.height, f.data.len()),
            (1280, 720, 1280 * 720 * 3)
        );
    }
}
