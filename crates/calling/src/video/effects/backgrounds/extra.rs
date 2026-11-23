//! The bundled backgrounds that came after the first six: soft gradients, bokeh, geometric
//! patterns and a few simple rooms. Like the others they are drawn from shapes at any size.

use super::{Canvas, hex, mix, noise, shade, vignette};
use std::f32::consts::TAU;

pub(super) fn sunrise(w: usize, h: usize) -> Canvas {
    let (top, mid, low, sun) = (hex(0x3a3d7c), hex(0xe8758a), hex(0xffc98b), hex(0xfff0c4));
    Canvas::new(w, h, |x, y| {
        let base = if y < 0.6 {
            mix(top, mid, y / 0.6)
        } else {
            mix(mid, low, (y - 0.6) / 0.4)
        };
        let d = ((x - 0.5) * 1.7).powi(2) + (y - 0.82).powi(2);
        mix(base, sun, 0.7 * (-d * 14.0).exp())
    })
}

pub(super) fn ocean(w: usize, h: usize) -> Canvas {
    let (deep, shallow, foam) = (hex(0x0b3c5d), hex(0x2a9d8f), hex(0xd9f2ee));
    Canvas::new(w, h, |x, y| {
        let wave = 0.5 + 0.5 * (x * 9.0 + y * 20.0).sin();
        mix(mix(deep, shallow, y), foam, 0.07 * wave * y)
    })
}

pub(super) fn forest_mist(w: usize, h: usize) -> Canvas {
    let (sky_top, sky_low) = (hex(0xe6f1e9), hex(0xb7d4c0));
    let (near, far) = (hex(0x2c5a45), hex(0x9cc3a9));
    Canvas::new(w, h, |x, y| {
        let mut c = mix(sky_top, sky_low, y);
        for k in 0..3 {
            let k = k as f32;
            let ridge = 0.42
                + 0.15 * k
                + 0.05 * (x * (5.0 + 2.0 * k) + 2.0 * k).sin()
                + 0.02 * (x * 17.0 + k).sin();
            if y > ridge {
                c = shade(mix(far, near, k / 2.0), 1.0 - (y - ridge) * 0.5);
            }
        }
        c
    })
}

pub(super) fn lavender(w: usize, h: usize) -> Canvas {
    let blobs = [
        (0.2, 0.3, 0.35, hex(0xf4b6d7)),
        (0.75, 0.25, 0.3, hex(0xb8a1f0)),
        (0.55, 0.8, 0.4, hex(0x8fc1f2)),
    ];
    Canvas::new(w, h, |x, y| {
        let mut c = hex(0xe9e2f8);
        for &(bx, by, r, col) in &blobs {
            let d = ((x - bx) * 1.78).powi(2) + (y - by).powi(2);
            c = mix(c, col, 0.8 * (-d / (r * r) * 2.0).exp());
        }
        c
    })
}

pub(super) fn midnight(w: usize, h: usize) -> Canvas {
    let mut c = Canvas::new(w, h, |_, y| mix(hex(0x070b1f), hex(0x233a6e), y * y));
    for n in 0..70u32 {
        let at = (noise(n * 3 + 1), noise(n * 3 + 2) * 0.85);
        c.disc(at, 0.002 + 0.003 * noise(n * 3 + 3), hex(0xf5f2e6), 0.9);
    }
    c
}

/// A soft diagonal blend from `a` (top left) to `b` (bottom right).
pub(super) fn diagonal(w: usize, h: usize, a: u32, b: u32) -> Canvas {
    let (a, b) = (hex(a), hex(b));
    Canvas::new(w, h, |x, y| mix(a, b, x * 0.6 + y * 0.8))
}

pub(super) fn graphite(w: usize, h: usize) -> Canvas {
    let (centre, edge) = (hex(0x5b6068), hex(0x1d2026));
    Canvas::new(w, h, |x, y| {
        let d = ((x - 0.5) * 1.6).powi(2) + (y - 0.5).powi(2);
        mix(centre, edge, (d * 2.2).min(1.0))
    })
}

/// Out-of-focus lights: `count` translucent discs of the `lights` colours over a gradient.
pub(super) fn bokeh(
    w: usize,
    h: usize,
    (top, bottom): (u32, u32),
    lights: &[u32],
    count: u32,
) -> Canvas {
    let (top, bottom) = (hex(top), hex(bottom));
    let mut c = Canvas::new(w, h, |_, y| mix(top, bottom, y));
    for i in 0..count {
        let at = (noise(i * 5 + 11), noise(i * 5 + 12));
        let r = 0.03 + 0.08 * noise(i * 5 + 13);
        let colour = lights[(noise(i * 5 + 14) * lights.len() as f32) as usize % lights.len()];
        c.disc(at, r, hex(colour), 0.22 + 0.25 * noise(i * 5 + 15));
    }
    c
}

pub(super) fn stripes(w: usize, h: usize) -> Canvas {
    let (a, b) = (hex(0xcfe3df), hex(0x8fbfb8));
    Canvas::new(w, h, |x, y| {
        let band = ((x * 1.78 + y) * 8.0).floor() as i32;
        shade(if band % 2 == 0 { a } else { b }, 1.05 - 0.2 * y)
    })
}

pub(super) fn mosaic(w: usize, h: usize) -> Canvas {
    let (a, b) = (hex(0x5d7fb5), hex(0xb4cdee));
    Canvas::new(w, h, |x, y| {
        let (cx, cy) = ((x * 16.0) as u32, (y * 9.0) as u32);
        let n = noise(cy * 31 + cx);
        let edge = (x * 16.0).fract().min((y * 9.0).fract());
        shade(mix(a, b, n), if edge < 0.04 { 0.8 } else { 1.0 })
    })
}

pub(super) fn rings(w: usize, h: usize) -> Canvas {
    let (dark, light) = (hex(0x3b1f4a), hex(0xf08a6c));
    Canvas::new(w, h, |x, y| {
        let d = (((x - 0.5) * 1.78).powi(2) + (y - 0.5).powi(2)).sqrt();
        mix(
            dark,
            light,
            (0.5 + 0.5 * (d * 30.0).cos()) * (1.0 - d * 0.9),
        )
    })
}

pub(super) fn waves(w: usize, h: usize) -> Canvas {
    let (sky_top, sky_low) = (hex(0xfbe7c6), hex(0xf8c291));
    let (front, back) = (hex(0xe58f65), hex(0x3d405b));
    Canvas::new(w, h, |x, y| {
        let mut c = mix(sky_top, sky_low, y);
        for k in 0..4 {
            let k = k as f32;
            let edge = 0.4 + 0.14 * k + 0.04 * (x * TAU * (1.0 + k * 0.5) + k).sin();
            if y > edge {
                c = mix(front, back, k / 3.0);
            }
        }
        c
    })
}

pub(super) fn dots(w: usize, h: usize) -> Canvas {
    let (ground, dot) = (hex(0x1f2a44), hex(0x7f9cf5));
    Canvas::new(w, h, |x, y| {
        let (gx, gy) = (x * 16.0, y * 9.0);
        let (fx, fy) = (gx.fract() - 0.5, gy.fract() - 0.5);
        if (fx * fx + fy * fy).sqrt() < 0.3 {
            mix(ground, dot, 0.35 + 0.65 * noise(gy as u32 * 64 + gx as u32))
        } else {
            ground
        }
    })
}

/// A window on the wall: sky behind a light frame with a cross bar.
fn window(c: &mut Canvas, (x0, y0, x1, y1): (f32, f32, f32, f32)) {
    let (sky_top, sky, frame) = (hex(0xcfe4f2), hex(0xa9cbe3), hex(0xfbfaf7));
    c.rect((x0, y0, x1, y1), |_, y| {
        mix(sky_top, sky, (y - y0) / (y1 - y0))
    });
    let mid = (x0 + x1) / 2.0;
    c.rect((mid - 0.006, y0, mid + 0.006, y1), |_, _| frame);
    for y in [y0, (y0 + y1) / 2.0, y1] {
        c.rect((x0, y - 0.01, x1, y + 0.01), |_, _| frame);
    }
    for x in [x0, x1] {
        c.rect((x - 0.006, y0, x + 0.006, y1), |_, _| frame);
    }
}

/// A leafy plant in a pot, its pot's top edge at `(x, y)`.
fn plant(c: &mut Canvas, (x, y): (f32, f32), size: f32) {
    c.rect(
        (x - 0.03 * size, y, x + 0.03 * size, y + 0.09 * size),
        |_, _| hex(0x6b4f3a),
    );
    for (dx, dy, r, k) in [
        (0.0, -0.06, 0.075, 1.0),
        (-0.035, -0.02, 0.06, 0.9),
        (0.035, -0.03, 0.06, 0.85),
        (0.01, -0.12, 0.05, 1.08),
    ] {
        c.circle(
            (x + dx * size, y + dy * size),
            r * size,
            shade(hex(0x5a8a5c), k),
        );
    }
}

pub(super) fn living_room(w: usize, h: usize) -> Canvas {
    let (wall, floor) = (hex(0xd9d2c7), hex(0x9c7f62));
    let mut c = Canvas::new(w, h, |_, y| {
        if y > 0.72 {
            mix(floor, shade(floor, 0.8), (y - 0.72) * 3.5)
        } else {
            mix(shade(wall, 1.05), wall, y / 0.72)
        }
    });
    window(&mut c, (0.08, 0.1, 0.36, 0.58));
    // A rug, and the sofa with its cushions.
    c.rect((0.22, 0.86, 0.88, 0.97), |_, _| hex(0xc77d62));
    c.rect((0.235, 0.875, 0.865, 0.955), |_, _| hex(0xe3a98a));
    let sofa = hex(0x4d6a7a);
    c.rect((0.42, 0.48, 0.9, 0.72), |_, y| {
        shade(sofa, 1.1 - (y - 0.48) * 0.8)
    });
    c.rect((0.38, 0.62, 0.94, 0.8), |_, _| shade(sofa, 0.92));
    c.rect((0.36, 0.56, 0.43, 0.8), |_, _| shade(sofa, 0.8));
    c.rect((0.89, 0.56, 0.96, 0.8), |_, _| shade(sofa, 0.8));
    c.rect((0.46, 0.53, 0.65, 0.66), |_, _| shade(sofa, 1.2));
    c.rect((0.66, 0.53, 0.85, 0.66), |_, _| shade(sofa, 1.2));
    c.rect((0.4, 0.8, 0.42, 0.83), |_, _| hex(0x3a2a20));
    c.rect((0.9, 0.8, 0.92, 0.83), |_, _| hex(0x3a2a20));
    // A picture above it, a floor lamp and a plant.
    c.rect((0.55, 0.12, 0.77, 0.36), |_, _| hex(0xfbfaf7));
    c.rect((0.565, 0.145, 0.755, 0.335), |x, y| {
        mix(
            hex(0xe9b384),
            hex(0x7aa6a1),
            (x - 0.565) / 0.19 + (y - 0.145),
        )
    });
    c.disc((0.975, 0.4), 0.14, hex(0xfff1c9), 0.18);
    c.rect((0.972, 0.42, 0.978, 0.82), |_, _| hex(0x3a3a3a));
    c.rect((0.95, 0.34, 1.0, 0.44), |_, _| hex(0xf6dfa8));
    plant(&mut c, (0.04, 0.72), 1.0);
    c
}

pub(super) fn home_office(w: usize, h: usize) -> Canvas {
    let (wall, floor) = (hex(0xc9d3d6), hex(0xa58b6f));
    let mut c = Canvas::new(w, h, |_, y| {
        if y > 0.82 {
            mix(floor, shade(floor, 0.85), (y - 0.82) * 5.0)
        } else {
            mix(shade(wall, 1.08), wall, y / 0.82)
        }
    });
    window(&mut c, (0.06, 0.1, 0.28, 0.5));
    // A shelf of books and a plant.
    let board = hex(0x6a4c37);
    c.rect((0.7, 0.3, 0.96, 0.315), |_, _| board);
    let palette = [0x8c3b3b, 0x2f5d62, 0xc9a45c, 0x4a5a85, 0x6f8f5a, 0xd8cfc0].map(hex);
    let mut x = 0.72;
    let mut n = 7;
    while x < 0.94 {
        n += 1;
        let width = 0.012 + 0.014 * noise(n);
        let tall = 0.07 + 0.05 * noise(n + 100);
        let colour = palette[(noise(n + 200) * palette.len() as f32) as usize % palette.len()];
        c.rect((x, 0.3 - tall, x + width, 0.3), |_, _| colour);
        x += width + 0.002;
    }
    plant(&mut c, (0.1, 0.62), 0.8);
    // The desk with a screen on it, and the back of a chair.
    c.rect((0.4, 0.74, 0.62, 1.0), |_, y| {
        shade(hex(0x3d4452), 1.1 - (y - 0.74))
    });
    let wood = hex(0x9a7b5c);
    c.rect((0.12, 0.64, 0.88, 0.67), |_, _| shade(wood, 1.12));
    c.rect((0.13, 0.67, 0.15, 0.88), |_, _| shade(wood, 0.8));
    c.rect((0.85, 0.67, 0.87, 0.88), |_, _| shade(wood, 0.8));
    c.rect((0.4, 0.36, 0.66, 0.58), |_, _| hex(0x20242b));
    c.rect((0.41, 0.375, 0.65, 0.565), |x, y| {
        mix(
            hex(0x5b8bd0),
            hex(0x9bc1f0),
            (x - 0.41) / 0.24 * 0.5 + (y - 0.375),
        )
    });
    c.rect((0.52, 0.58, 0.54, 0.64), |_, _| hex(0x20242b));
    c.rect((0.47, 0.63, 0.59, 0.64), |_, _| hex(0x20242b));
    // A desk lamp.
    c.disc((0.78, 0.5), 0.12, hex(0xfff1c9), 0.16);
    c.rect((0.775, 0.5, 0.782, 0.64), |_, _| hex(0x3a3a3a));
    c.rect((0.75, 0.47, 0.81, 0.5), |_, _| hex(0xf6dfa8));
    c
}

pub(super) fn loft(w: usize, h: usize) -> Canvas {
    let floor = hex(0x8a6a4c);
    let mut c = Canvas::new(w, h, |x, y| {
        if y > 0.8 {
            let plank = (x * 9.0).fract();
            shade(
                floor,
                if plank < 0.03 {
                    0.75
                } else {
                    1.0 - (y - 0.8) * 0.8
                },
            )
        } else {
            // Brick, every other row shifted by half a brick.
            let row = (y * 13.0).floor();
            let along = x * 14.0 + if row as i32 % 2 == 0 { 0.0 } else { 0.5 };
            let mortar = along.fract() < 0.05 || (y * 13.0).fract() < 0.09;
            if mortar {
                hex(0xcfc6bb)
            } else {
                let tone = noise(row as u32 * 40 + along.floor() as u32);
                shade(mix(hex(0xa8553f), hex(0xc77b5e), tone), vignette(x, y))
            }
        }
    });
    // A tall window with a black frame, and three pendant lights.
    c.rect((0.3, 0.1, 0.7, 0.66), |_, _| hex(0x23262b));
    let (sky_top, sky) = (hex(0xcfe4f2), hex(0xa9cbe3));
    for col in 0..3 {
        let x0 = 0.31 + col as f32 * 0.128;
        c.rect((x0, 0.115, x0 + 0.118, 0.645), |_, y| {
            mix(sky_top, sky, (y - 0.115) / 0.53)
        });
    }
    for x in [0.1, 0.5, 0.9] {
        c.rect((x - 0.002, 0.0, x + 0.002, 0.18), |_, _| hex(0x23262b));
        c.disc((x, 0.2), 0.12, hex(0xffe9b0), 0.14);
        c.disc((x, 0.2), 0.035, hex(0xffe9b0), 0.95);
    }
    c
}

pub(super) fn studio(w: usize, h: usize) -> Canvas {
    let (top, mid, floor) = (hex(0xb7bcc4), hex(0x8e949e), hex(0x7d838c));
    let mut c = Canvas::new(w, h, |x, y| {
        let spot = (-(((x - 0.5) * 1.5).powi(2) + (y - 0.4).powi(2)) * 5.0).exp();
        let base = if y < 0.75 {
            mix(top, mid, y / 0.75)
        } else {
            mix(floor, shade(floor, 0.75), (y - 0.75) * 4.0)
        };
        mix(base, hex(0xeef0f3), 0.35 * spot)
    });
    // Two light stands with their panels.
    for x in [0.06, 0.94] {
        c.rect((x - 0.004, 0.45, x + 0.004, 0.95), |_, _| hex(0x2b2e34));
        c.rect((x - 0.035, 0.18, x + 0.035, 0.46), |_, y| {
            shade(hex(0xf4f1e8), 1.0 - (y - 0.18) * 0.4)
        });
        c.disc((x, 0.32), 0.2, hex(0xfff6dc), 0.1);
    }
    c
}

/// Each colour of a palette as a bokeh light list.
pub(super) const WARM: [u32; 4] = [0xffd27d, 0xffa45c, 0xff7a6b, 0xfff0c4];
pub(super) const COOL: [u32; 4] = [0x7dd3fc, 0x93c5fd, 0xa5f3fc, 0xc4b5fd];
pub(super) const CITY: [u32; 4] = [0xffc857, 0xff9f45, 0x7ab8ff, 0xe8f1ff];
