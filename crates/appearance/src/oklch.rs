//! sRGB to OKLCH and back, the WCAG 2.x contrast ratio, and the colour steps the theme
//! derivation uses. Pure functions in `f64`; there is no colour crate behind them.
//!
//! OKLCH is Björn Ottosson's Oklab in polar form: `l` 0 to 1, `c` 0 and up, `h` in degrees.

use crate::prefs::Rgb;
use crate::resolve::ResolvedTheme;

/// A colour in OKLCH.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

const WHITE: Rgb = Rgb::new(255, 255, 255);
const BLACK: Rgb = Rgb::new(0, 0, 0);

/// A colour whose WCAG relative luminance is below this is dark.
const DARK_BELOW: f64 = 0.18;
/// An accent chroma below this counts as grey in [`tint`].
const GREY_CHROMA: f64 = 0.02;
/// Slack for float noise when testing whether a colour fits in sRGB.
const GAMUT_EPS: f64 = 1e-9;

fn to_linear(v: u8) -> f64 {
    let c = f64::from(v) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(v: f64) -> f64 {
    let c = v.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// The colour in OKLCH. The hue of a grey is meaningless (any value, chroma near 0).
pub fn to_oklch(c: Rgb) -> Oklch {
    let (r, g, b) = (to_linear(c.r), to_linear(c.g), to_linear(c.b));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    let lab_l = 0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s;
    let a = 1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s;
    let b = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s;
    Oklch {
        l: lab_l,
        c: a.hypot(b),
        h: b.atan2(a).to_degrees().rem_euclid(360.0),
    }
}

/// Linear sRGB of an OKLCH colour (may lie outside 0..=1).
fn to_linear_rgb(o: Oklch) -> [f64; 3] {
    let (sin, cos) = o.h.to_radians().sin_cos();
    let (a, b) = (o.c * cos, o.c * sin);
    let l = (o.l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m = (o.l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s = (o.l - 0.089_484_177_5 * a - 1.291_485_548_0 * b).powi(3);
    [
        4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
        -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
        -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701_0 * s,
    ]
}

fn in_gamut(o: Oklch) -> bool {
    to_linear_rgb(o)
        .iter()
        .all(|v| (-GAMUT_EPS..=1.0 + GAMUT_EPS).contains(v))
}

/// The sRGB colour for an OKLCH one: `l` is clamped to 0..=1; a colour outside sRGB loses chroma
/// (30 bisection steps, keeping the highest chroma that fits); channels round to 8 bits.
pub fn from_oklch(o: Oklch) -> Rgb {
    let mut o = Oklch {
        l: o.l.clamp(0.0, 1.0),
        c: o.c.max(0.0),
        h: o.h,
    };
    if !in_gamut(o) {
        let (mut lo, mut hi) = (0.0, o.c);
        for _ in 0..30 {
            let mid = (lo + hi) / 2.0;
            if in_gamut(Oklch { c: mid, ..o }) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        o.c = lo;
    }
    let [r, g, b] = to_linear_rgb(o).map(|v| (from_linear(v) * 255.0).round() as u8);
    Rgb::new(r, g, b)
}

/// WCAG 2.x relative luminance, 0 (black) to 1 (white).
pub fn relative_luminance(c: Rgb) -> f64 {
    0.2126 * to_linear(c.r) + 0.7152 * to_linear(c.g) + 0.0722 * to_linear(c.b)
}

/// WCAG 2.x contrast ratio, 1 to 21, whichever colour is lighter.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if x >= y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

/// `x` moved the fraction `t` of the way to `y` in lightness, keeping its chroma and hue. It never
/// passes `y`.
pub fn toward(x: Rgb, y: Rgb, t: f64) -> Rgb {
    let (ox, oy) = (to_oklch(x), to_oklch(y));
    from_oklch(Oklch {
        l: ox.l + t * (oy.l - ox.l),
        ..ox
    })
}

/// A background tint of the accent: the lightness `t` of the way from `bg` to `fg`, with `k` times
/// the accent's chroma and its hue. A grey accent (chroma under 0.02) gives a grey.
pub fn tint(bg: Rgb, fg: Rgb, accent: Rgb, t: f64, k: f64) -> Rgb {
    let (b, f, a) = (to_oklch(bg), to_oklch(fg), to_oklch(accent));
    let (c, h) = if a.c < GREY_CHROMA {
        (0.0, 0.0)
    } else {
        (k * a.c, a.h)
    };
    from_oklch(Oklch {
        l: b.l + t * (f.l - b.l),
        c,
        h,
    })
}

/// Dark when the relative luminance is below 0.18.
pub fn kind_of(c: Rgb) -> ResolvedTheme {
    if relative_luminance(c) < DARK_BELOW {
        ResolvedTheme::Dark
    } else {
        ResolvedTheme::Light
    }
}

/// White or black, whichever has the higher lowest contrast over `bgs`; white on a tie.
pub fn best_on(bgs: &[Rgb]) -> Rgb {
    let worst = |fg: Rgb| {
        bgs.iter()
            .map(|&b| contrast(fg, b))
            .fold(f64::INFINITY, f64::min)
    };
    if worst(WHITE) >= worst(BLACK) {
        WHITE
    } else {
        BLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    fn within_one(a: Rgb, b: Rgb) -> bool {
        a.r.abs_diff(b.r) <= 1 && a.g.abs_diff(b.g) <= 1 && a.b.abs_diff(b.b) <= 1
    }

    #[test]
    fn round_trip_stays_within_one_per_channel() {
        for r in (0..=255u8).step_by(17) {
            for g in (0..=255u8).step_by(17) {
                for b in (0..=255u8).step_by(17) {
                    let c = Rgb::new(r, g, b);
                    let back = from_oklch(to_oklch(c));
                    assert!(within_one(c, back), "{c:?} came back as {back:?}");
                }
            }
        }
    }

    #[test]
    fn a_known_colour_in_oklch() {
        let o = to_oklch(Rgb::from_hex("#7f85f5").unwrap());
        assert!(near(o.l, 0.664, 0.001), "l {}", o.l);
        assert!(near(o.c, 0.164, 0.001), "c {}", o.c);
        assert!(near(o.h, 278.6, 0.1), "h {}", o.h);
    }

    #[test]
    fn contrast_of_the_extremes() {
        assert!(near(contrast(BLACK, WHITE), 21.0, 1e-9));
        assert!(near(contrast(WHITE, BLACK), 21.0, 1e-9));
        let grey = Rgb::new(120, 120, 120);
        assert!(near(contrast(grey, grey), 1.0, 1e-12));
    }

    #[test]
    fn an_out_of_gamut_colour_loses_chroma() {
        let c = from_oklch(Oklch {
            l: 0.7,
            c: 0.4,
            h: 140.0,
        });
        let o = to_oklch(c);
        assert!(o.c < 0.4, "chroma {}", o.c);
        assert!(near(o.l, 0.7, 0.01), "lightness {}", o.l);
    }

    #[test]
    fn toward_never_passes_its_target() {
        let colours = [
            Rgb::new(0x17, 0x14, 0x1c),
            Rgb::new(0xf1, 0xee, 0xf5),
            Rgb::new(0x80, 0x80, 0x80),
            Rgb::new(0xb4, 0x8c, 0xf2),
        ];
        for x in colours {
            for y in colours {
                for t in [0.0, 0.05, 0.25, 0.5, 1.0] {
                    let (lx, ly) = (to_oklch(x).l, to_oklch(y).l);
                    let lt = to_oklch(toward(x, y, t)).l;
                    let (lo, hi) = (lx.min(ly) - 0.005, lx.max(ly) + 0.005);
                    assert!((lo..=hi).contains(&lt), "{x:?} to {y:?} at {t}: {lt}");
                }
            }
        }
        // t = 1 reaches the target's lightness.
        let t = toward(Rgb::new(0x20, 0x20, 0x20), WHITE, 1.0);
        assert!(within_one(t, WHITE), "{t:?}");
    }

    #[test]
    fn tint_of_a_grey_accent_is_grey() {
        let c = tint(
            Rgb::new(0x20, 0x20, 0x20),
            WHITE,
            Rgb::new(0x80, 0x80, 0x80),
            0.2,
            0.5,
        );
        assert!(to_oklch(c).c < 0.005, "{c:?}");
        let coloured = tint(
            Rgb::new(0x20, 0x20, 0x20),
            WHITE,
            Rgb::new(0x5b, 0x5f, 0xc7),
            0.2,
            0.5,
        );
        assert!(to_oklch(coloured).c > 0.02, "{coloured:?}");
    }

    #[test]
    fn kinds_and_best_on() {
        assert_eq!(kind_of(Rgb::new(0x14, 0x14, 0x14)), ResolvedTheme::Dark);
        assert_eq!(kind_of(WHITE), ResolvedTheme::Light);
        assert_eq!(best_on(&[Rgb::new(0x10, 0x10, 0x10)]), WHITE);
        assert_eq!(best_on(&[Rgb::new(0xf0, 0xf0, 0xf0)]), BLACK);
        // Mixed backgrounds: the lowest contrast decides.
        assert_eq!(
            best_on(&[Rgb::new(0x10, 0x10, 0x10), Rgb::new(0x20, 0x20, 0x20)]),
            WHITE
        );
        // White on a tie.
        assert_eq!(best_on(&[]), WHITE);
    }
}
