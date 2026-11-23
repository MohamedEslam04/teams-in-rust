//! The pixel work of the effects, on I420 planes: lookup tables for the filters and the low-light
//! boost, a box blur, the person mask (scaled, smoothed) and mixing two pictures through it.

use super::Filter;
use crate::video::{I420Frame, RgbFrame};

/// A new value for every 8-bit sample.
pub type Lut = [u8; 256];

pub fn apply_lut(plane: &mut [u8], lut: &Lut) {
    for p in plane {
        *p = lut[usize::from(*p)];
    }
}

fn table(f: impl Fn(i32) -> i32) -> Lut {
    let mut t = [0u8; 256];
    for (i, v) in t.iter_mut().enumerate() {
        *v = f(i as i32).clamp(0, 255) as u8;
    }
    t
}

/// The low-light boost: the luma's shadows and mid-tones lifted (limited range, 16 black, 235
/// white), black and white kept.
pub fn brighten_lut() -> Lut {
    table(|i| {
        if !(16..=235).contains(&i) {
            return i;
        }
        let t = (i - 16) as f32 / 219.0;
        (16.0 + 219.0 * t.powf(0.65)).round() as i32
    })
}

/// The low-light boost on a whole picture (`w` wide): the luma through [`brighten_lut`], and each
/// chroma sample scaled by the gain its 2x2 luma got, so the colours stay as strong as the light
/// they are lit with (a lifted luma over the same chroma looks grey and washed out). The gain is
/// held at 1.4, where noise in a dark picture would otherwise be coloured too.
pub fn brighten(y: &mut [u8], u: &mut [u8], v: &mut [u8], w: usize) {
    let lut = brighten_lut();
    let gain: Vec<i32> = (0..=255usize)
        .map(|i| {
            if (17..=235).contains(&i) {
                let g = f32::from(lut[i] - 16) / (i - 16) as f32;
                (g.clamp(1.0, 1.4) * 256.0) as i32
            } else {
                256
            }
        })
        .collect();
    let cw = w / 2;
    for (c, (pu, pv)) in u.iter_mut().zip(v.iter_mut()).enumerate() {
        let (cx, cy) = (c % cw, c / cw);
        let at = (2 * cy) * w + 2 * cx;
        let luma = (u32::from(y[at])
            + u32::from(y[at + 1])
            + u32::from(y[at + w])
            + u32::from(y[at + w + 1])
            + 2)
            / 4;
        let g = gain[luma as usize];
        for p in [pu, pv] {
            *p = (128 + (((i32::from(*p) - 128) * g + 128) >> 8)).clamp(0, 255) as u8;
        }
    }
    apply_lut(y, &lut);
}

/// The Y, U and V tables of a colour filter; `None` for no filter.
pub fn filter_luts(filter: Filter) -> Option<[Lut; 3]> {
    let same = table(|v| v);
    let shift = |d: i32| table(move |v| v + d);
    let scale = |k: f32| table(move |v| 128 + ((v - 128) as f32 * k).round() as i32);
    Some(match filter {
        Filter::None => return None,
        // Less blue, more red.
        Filter::Warm => [same, shift(-10), shift(9)],
        Filter::Cool => [same, shift(11), shift(-8)],
        Filter::Mono => [same, table(|_| 128), table(|_| 128)],
        // More contrast and colour.
        Filter::Vivid => [scale(1.1), scale(1.45), scale(1.45)],
    })
}

/// Blurs `src` (`w` x `h`) into `out`: a box of radius `r` run twice (close to a Gaussian), the
/// edges repeated. `tmp` is scratch space.
pub fn blur(src: &[u8], w: usize, h: usize, r: usize, out: &mut Vec<u8>, tmp: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(&src[..w * h]);
    if r == 0 || w == 0 || h == 0 {
        return;
    }
    tmp.clear();
    tmp.resize(w * h, 0);
    for _ in 0..2 {
        box_rows(out, tmp, w, r);
        box_cols(tmp, out, w, h, r);
    }
}

/// One horizontal box pass of `src` into `dst`.
fn box_rows(src: &[u8], dst: &mut [u8], w: usize, r: usize) {
    let n = (2 * r + 1) as u32;
    let r = r as isize;
    for (row, out) in src.chunks_exact(w).zip(dst.chunks_exact_mut(w)) {
        let at = move |i: isize| u32::from(row[i.clamp(0, w as isize - 1) as usize]);
        let mut sum: u32 = (-r..=r).map(at).sum();
        for (x, o) in out.iter_mut().enumerate() {
            *o = ((sum + n / 2) / n) as u8;
            sum = sum + at(x as isize + r + 1) - at(x as isize - r);
        }
    }
}

/// One vertical box pass of `src` into `dst`, row by row (a running sum per column).
fn box_cols(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let n = (2 * r + 1) as u32;
    let r = r as isize;
    let row = move |y: isize| {
        let y = y.clamp(0, h as isize - 1) as usize;
        &src[y * w..(y + 1) * w]
    };
    let mut sums = vec![0u32; w];
    for y in -r..=r {
        for (s, v) in sums.iter_mut().zip(row(y)) {
            *s += u32::from(*v);
        }
    }
    for (y, out) in dst.chunks_exact_mut(w).enumerate().take(h) {
        for (o, s) in out.iter_mut().zip(&sums) {
            *o = ((s + n / 2) / n) as u8;
        }
        let (add, sub) = (row(y as isize + r + 1), row(y as isize - r));
        for ((s, a), b) in sums.iter_mut().zip(add).zip(sub) {
            *s = *s + u32::from(*a) - u32::from(*b);
        }
    }
}

/// How much of each pixel is the person: 255 the person, 0 the background.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Mask {
    /// The model's person probabilities as a mask with a short soft edge: below 0.25 is
    /// background, above 0.75 the person (a stray low value does not show the room through).
    pub fn from_probabilities(p: &[f32], width: usize, height: usize) -> Mask {
        let data = p
            .iter()
            .take(width * height)
            .map(|&v| {
                let t = ((v - 0.25) / 0.5).clamp(0.0, 1.0);
                (t * t * (3.0 - 2.0 * t) * 255.0).round() as u8
            })
            .collect();
        Mask {
            width,
            height,
            data,
        }
    }
}

/// Mixes `next` into `prev` (mostly `next`): the edge flickers less from frame to frame.
pub fn smooth(prev: &mut Mask, next: &Mask) {
    if (prev.width, prev.height) != (next.width, next.height) {
        *prev = next.clone();
        return;
    }
    for (p, n) in prev.data.iter_mut().zip(&next.data) {
        *p = ((u16::from(*p) + 3 * u16::from(*n) + 2) / 4) as u8;
    }
}

/// `mask` scaled to `w` x `h` (bilinear) into `out`.
pub fn upscale(mask: &Mask, w: usize, h: usize, out: &mut Vec<u8>) {
    scale_plane(&mask.data, mask.width, mask.height, w, h, out);
}

/// The plane `src` (`sw` x `sh`) scaled to `w` x `h` (bilinear) into `out`.
pub fn scale_plane(src: &[u8], sw: usize, sh: usize, w: usize, h: usize, out: &mut Vec<u8>) {
    out.clear();
    out.resize(w * h, 0);
    let (mw, mh) = (sw.max(1), sh.max(1));
    if src.len() < mw * mh {
        return;
    }
    // The two source samples around a destination pixel's centre, and the second one's weight
    // in 256ths.
    let around = |i: usize, src: usize, dst: usize| {
        let f = ((i as f32 + 0.5) * src as f32 / dst as f32 - 0.5).max(0.0);
        let i0 = (f as usize).min(src - 1);
        let i1 = (i0 + 1).min(src - 1);
        (i0, i1, ((f - i0 as f32) * 256.0) as u32)
    };
    let xs: Vec<_> = (0..w).map(|x| around(x, mw, w)).collect();
    for (y, line) in out.chunks_exact_mut(w).enumerate() {
        let (y0, y1, fy) = around(y, mh, h);
        let (r0, r1) = (&src[y0 * mw..(y0 + 1) * mw], &src[y1 * mw..(y1 + 1) * mw]);
        for (o, &(x0, x1, fx)) in line.iter_mut().zip(&xs) {
            let top = u32::from(r0[x0]) * (256 - fx) + u32::from(r0[x1]) * fx;
            let bottom = u32::from(r1[x0]) * (256 - fx) + u32::from(r1[x1]) * fx;
            *o = ((top * (256 - fy) + bottom * fy + 32768) >> 16) as u8;
        }
    }
}

/// `src` (`w` x `h`, even) at half the size, every 2x2 block averaged, into `out`.
pub fn halve_plane(src: &[u8], w: usize, h: usize, out: &mut Vec<u8>) {
    out.clear();
    for rows in src.chunks_exact(2 * w).take(h / 2) {
        let (a, b) = rows.split_at(w);
        out.extend(a.chunks_exact(2).zip(b.chunks_exact(2)).map(|(p, q)| {
            ((u16::from(p[0]) + u16::from(p[1]) + u16::from(q[0]) + u16::from(q[1]) + 2) / 4) as u8
        }));
    }
}

/// Where the person is, made to follow the edges of the picture: the guided filter (He et al.)
/// with the picture's luma as the guide. The model's mask is small and blurry once scaled up; the
/// filter makes its edge sit on the edge it finds in the picture (a head against a wall) and
/// leaves it soft where the picture has none (hair, a shoulder in the shade). Holds scratch
/// buffers, so keep one for every frame.
#[derive(Default)]
pub struct Refiner {
    guide: Vec<f32>,
    mask: Vec<f32>,
    mean_guide: Vec<f32>,
    mean_mask: Vec<f32>,
    mean_gg: Vec<f32>,
    mean_gm: Vec<f32>,
    tmp: Vec<f32>,
    scratch: Vec<f32>,
}

/// How much variation in the guide counts as an edge (squared, guide in 0..1): smaller follows
/// the picture closer.
const REFINE_EPS: f32 = 0.002;
/// After refining, the mask's contrast around 0.5: a little steeper, so no halo is left.
const REFINE_CONTRAST: f32 = 1.3;

impl Refiner {
    /// `mask` made to follow `guide` (both `w` x `h`), within a window of radius `r`, into `out`.
    pub fn refine(
        &mut self,
        mask: &[u8],
        guide: &[u8],
        w: usize,
        h: usize,
        r: usize,
        out: &mut Vec<u8>,
    ) {
        let n = w * h;
        out.clear();
        if mask.len() < n || guide.len() < n || n == 0 {
            out.resize(n, 0);
            return;
        }
        let scale = 1.0 / 255.0;
        self.guide.clear();
        self.guide.extend(guide[..n].iter().map(|&v| f32::from(v) * scale));
        self.mask.clear();
        self.mask.extend(mask[..n].iter().map(|&v| f32::from(v) * scale));
        let (g, m) = (&self.guide, &self.mask);
        // `scratch` holds the products to be averaged, in turn.
        self.scratch.clear();
        self.scratch.extend(g.iter().zip(m).map(|(g, m)| g * m));
        box_f32(g, &mut self.mean_guide, &mut self.tmp, w, h, r);
        box_f32(m, &mut self.mean_mask, &mut self.tmp, w, h, r);
        box_f32(&self.scratch, &mut self.mean_gm, &mut self.tmp, w, h, r);
        self.scratch.clear();
        self.scratch.extend(g.iter().map(|g| g * g));
        box_f32(&self.scratch, &mut self.mean_gg, &mut self.tmp, w, h, r);
        // a and b of q = a * guide + b, reusing the mean buffers; a in `mean_gm`, b in `mean_gg`.
        for i in 0..n {
            let (mg, mm) = (self.mean_guide[i], self.mean_mask[i]);
            let var = self.mean_gg[i] - mg * mg;
            let cov = self.mean_gm[i] - mg * mm;
            let a = cov / (var + REFINE_EPS);
            self.mean_gm[i] = a;
            self.mean_gg[i] = mm - a * mg;
        }
        box_f32(&self.mean_gm, &mut self.scratch, &mut self.tmp, w, h, r);
        box_f32(&self.mean_gg, &mut self.mean_mask, &mut self.tmp, w, h, r);
        out.extend(
            self.scratch
                .iter()
                .zip(&self.mean_mask)
                .zip(&self.guide)
                .map(|((a, b), g)| {
                    let q = a * g + b;
                    (((q - 0.5) * REFINE_CONTRAST + 0.5).clamp(0.0, 1.0) * 255.0 + 0.5) as u8
                }),
        );
    }
}

/// A box mean of radius `r` of `src` (`w` x `h`) into `dst`, the edges repeated.
fn box_f32(src: &[f32], dst: &mut Vec<f32>, tmp: &mut Vec<f32>, w: usize, h: usize, r: usize) {
    dst.clear();
    dst.resize(w * h, 0.0);
    tmp.clear();
    tmp.resize(w * h, 0.0);
    let n = (2 * r + 1) as f32;
    let ri = r as isize;
    for (row, out) in src.chunks_exact(w).zip(tmp.chunks_exact_mut(w)) {
        let at = |i: isize| row[i.clamp(0, w as isize - 1) as usize];
        let mut sum: f32 = (-ri..=ri).map(at).sum();
        for (x, o) in out.iter_mut().enumerate() {
            *o = sum / n;
            sum += at(x as isize + ri + 1) - at(x as isize - ri);
        }
    }
    let row = |y: isize| {
        let y = y.clamp(0, h as isize - 1) as usize;
        &tmp[y * w..(y + 1) * w]
    };
    let mut sums = vec![0f32; w];
    for y in -ri..=ri {
        for (s, v) in sums.iter_mut().zip(row(y)) {
            *s += v;
        }
    }
    for (y, out) in dst.chunks_exact_mut(w).enumerate().take(h) {
        for (o, s) in out.iter_mut().zip(&sums) {
            *o = s / n;
        }
        let (add, sub) = (row(y as isize + ri + 1), row(y as isize - ri));
        for ((s, a), b) in sums.iter_mut().zip(add).zip(sub) {
            *s += a - b;
        }
    }
}

/// The chroma planes' mask: every other sample of every other row of the luma's.
pub fn half(alpha: &[u8], w: usize, h: usize, out: &mut Vec<u8>) {
    out.clear();
    for row in alpha.chunks_exact(w).take(h).step_by(2) {
        out.extend(row.iter().step_by(2));
    }
}

/// `fg` where `alpha` is 255, `bg` where it is 0, mixed in between.
pub fn composite(fg: &mut [u8], bg: &[u8], alpha: &[u8]) {
    for ((f, &b), &a) in fg.iter_mut().zip(bg).zip(alpha) {
        let a = u32::from(a);
        *f = ((u32::from(*f) * a + u32::from(b) * (255 - a) + 127) / 255) as u8;
    }
}

/// BT.601 limited range, as cameras write it.
pub fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let c = (i32::from(y) - 16).max(0) * 298;
    let (d, e) = (i32::from(u) - 128, i32::from(v) - 128);
    let clamp = |x: i32| (x >> 8).clamp(0, 255) as u8;
    [
        clamp(c + 409 * e + 128),
        clamp(c - 100 * d - 208 * e + 128),
        clamp(c + 516 * d + 128),
    ]
}

/// The other way: luma, then the two chroma values.
pub fn rgb_to_yuv([r, g, b]: [u8; 3]) -> [u8; 3] {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    [y, u, v].map(|c| c.clamp(0, 255) as u8)
}

/// `img` scaled to cover `w` x `h` (what sticks out is cut off, as a wallpaper is), as I420. A
/// picture shrunk to fit is averaged over what each pixel covers (not picked: that is jagged), one
/// that is enlarged is interpolated, and the chroma is the average of each 2x2 block.
pub fn cover_i420(img: &RgbFrame, w: usize, h: usize) -> I420Frame {
    let (iw, ih) = (img.width.max(1), img.height.max(1));
    let scale = f32::max(w as f32 / iw as f32, h as f32 / ih as f32);
    let (ox, oy) = (
        (iw as f32 * scale - w as f32) / 2.0,
        (ih as f32 * scale - h as f32) / 2.0,
    );
    let px = |x: usize, y: usize| {
        let i = (y.min(ih - 1) * iw + x.min(iw - 1)) * 3;
        match img.data.get(i..i + 3) {
            Some(p) => [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])],
            None => [0.0; 3],
        }
    };
    // The colour of destination pixel (x, y).
    let sample = |x: usize, y: usize| -> [u8; 3] {
        let (sx0, sx1) = ((x as f32 + ox) / scale, (x as f32 + 1.0 + ox) / scale);
        let (sy0, sy1) = ((y as f32 + oy) / scale, (y as f32 + 1.0 + oy) / scale);
        let mut sum = [0f32; 3];
        let rgb = if sx1 - sx0 > 1.0 || sy1 - sy0 > 1.0 {
            let (x0, x1) = (sx0 as usize, (sx1.ceil() as usize).max(sx0 as usize + 1));
            let (y0, y1) = (sy0 as usize, (sy1.ceil() as usize).max(sy0 as usize + 1));
            for sy in y0..y1 {
                for sx in x0..x1 {
                    for (s, v) in sum.iter_mut().zip(px(sx, sy)) {
                        *s += v;
                    }
                }
            }
            let n = ((x1 - x0) * (y1 - y0)) as f32;
            sum.map(|s| s / n)
        } else {
            let (cx, cy) = ((sx0 + sx1) / 2.0 - 0.5, (sy0 + sy1) / 2.0 - 0.5);
            let (fx0, fy0) = (cx.max(0.0).floor(), cy.max(0.0).floor());
            let (tx, ty) = ((cx.max(0.0) - fx0).min(1.0), (cy.max(0.0) - fy0).min(1.0));
            let (x0, y0) = (fx0 as usize, fy0 as usize);
            let (a, b, c, d) = (px(x0, y0), px(x0 + 1, y0), px(x0, y0 + 1), px(x0 + 1, y0 + 1));
            for k in 0..3 {
                let top = a[k] * (1.0 - tx) + b[k] * tx;
                let bottom = c[k] * (1.0 - tx) + d[k] * tx;
                sum[k] = top * (1.0 - ty) + bottom * ty;
            }
            sum
        };
        rgb.map(|v| (v + 0.5) as u8)
    };
    let mut data = vec![128u8; I420Frame::len_for(w, h)];
    let (luma, chroma) = data.split_at_mut(w * h);
    let (u, v) = chroma.split_at_mut(w * h / 4);
    let mut sums = vec![[0u32; 2]; u.len()];
    for (y, line) in luma.chunks_exact_mut(w).enumerate() {
        for (x, l) in line.iter_mut().enumerate() {
            let [py, pu, pv] = rgb_to_yuv(sample(x, y));
            *l = py;
            if let Some(s) = sums.get_mut((y / 2) * (w / 2) + x / 2) {
                s[0] += u32::from(pu);
                s[1] += u32::from(pv);
            }
        }
    }
    for ((u, v), s) in u.iter_mut().zip(v.iter_mut()).zip(&sums) {
        *u = ((s[0] + 2) / 4) as u8;
        *v = ((s[1] + 2) / 4) as u8;
    }
    I420Frame {
        width: w,
        height: h,
        data,
    }
}

/// `frame` as the model's input: planar RGB in 0..1, `w` x `h`, each pixel the average of the
/// part of the frame it covers (the frame is several times larger than the model's input, and
/// picking one pixel of every few would alias fine detail into the mask).
pub fn model_input(frame: &I420Frame, w: usize, h: usize) -> Vec<f32> {
    let (fy, fu, fv) = frame.planes();
    let (fw, fh) = (frame.width, frame.height);
    let chroma_w = fw / 2;
    // The source span of destination index `i` of `dst` over `src`: at least one sample.
    let span = |i: usize, dst: usize, src: usize| {
        let a = (i * src / dst).min(src - 1);
        (a, ((i + 1) * src / dst).clamp(a + 1, src))
    };
    let xs: Vec<_> = (0..w).map(|x| span(x, w, fw)).collect();
    let plane = w * h;
    let mut out = vec![0f32; 3 * plane];
    for y in 0..h {
        let (y0, y1) = span(y, h, fh);
        let (cy0, cy1) = (y0 / 2, y1.div_ceil(2).min(fh / 2));
        for (x, &(x0, x1)) in xs.iter().enumerate() {
            let (cx0, cx1) = (x0 / 2, x1.div_ceil(2).min(chroma_w));
            let mut luma = 0u32;
            for row in y0..y1 {
                luma += fy[row * fw + x0..row * fw + x1]
                    .iter()
                    .map(|&v| u32::from(v))
                    .sum::<u32>();
            }
            let (mut u, mut v) = (0u32, 0u32);
            for row in cy0..cy1 {
                for c in cx0..cx1 {
                    u += u32::from(fu[row * chroma_w + c]);
                    v += u32::from(fv[row * chroma_w + c]);
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let nc = (((cy1 - cy0) * (cx1 - cx0)) as u32).max(1);
            let rgb = yuv_to_rgb(
                ((luma + n / 2) / n) as u8,
                ((u + nc / 2) / nc) as u8,
                ((v + nc / 2) / nc) as u8,
            );
            let i = y * w + x;
            for (c, value) in rgb.into_iter().enumerate() {
                out[c * plane + i] = f32::from(value) / 255.0;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_warm_and_cool_filters_move_the_colour_and_keep_the_light() {
        let [y, u, v] = filter_luts(Filter::Warm).unwrap();
        assert_eq!(y[100], 100);
        assert!(u[128] < 128 && v[128] > 128, "less blue, more red");
        let [y, u, v] = filter_luts(Filter::Cool).unwrap();
        assert_eq!(y[100], 100);
        assert!(u[128] > 128 && v[128] < 128);
        assert_eq!(u[255], 255, "clamped");
        assert!(filter_luts(Filter::None).is_none());
    }

    #[test]
    fn black_and_white_drops_the_colour_and_vivid_adds_some() {
        let [_, u, v] = filter_luts(Filter::Mono).unwrap();
        assert!(u.iter().chain(&v).all(|&c| c == 128));
        let [y, u, _] = filter_luts(Filter::Vivid).unwrap();
        assert!(u[160] > 160 && u[96] < 96, "further from grey");
        assert_eq!(u[128], 128, "grey stays grey");
        assert!(y[200] > 200 && y[60] < 60, "more contrast");
    }

    #[test]
    fn the_low_light_boost_keeps_colours_as_strong_as_the_light_it_adds() {
        // 2x2, a dim reddish picture, a neutral one and a bright reddish one.
        let dim = |chroma: (u8, u8), luma: u8| {
            let mut y = [luma; 4];
            let (mut u, mut v) = ([chroma.0], [chroma.1]);
            brighten(&mut y, &mut u, &mut v, 2);
            (y[0], u[0], v[0])
        };
        let (y, u, v) = dim((108, 148), 60);
        assert!(y > 60, "lifted: {y}");
        assert!(u < 108 && v > 148, "more colour, not less: {u} {v}");
        assert_eq!(dim((128, 128), 60).1, 128, "grey stays grey");
        assert_eq!(dim((108, 148), 235), (235, 108, 148), "white is left alone");
        assert!(i32::from(v) <= 128 + 20 * 14 / 10 + 1, "the gain is held: {v}");
    }

    #[test]
    fn the_low_light_boost_is_monotonic_and_keeps_black_and_white() {
        let lut = brighten_lut();
        assert_eq!(lut[16], 16);
        assert_eq!(lut[235], 235);
        assert_eq!(lut[5], 5);
        assert!(lut[60] > 60);
        assert!(lut.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn a_blur_keeps_a_flat_picture_and_spreads_a_dot() {
        let (w, h) = (9, 7);
        let flat = vec![90u8; w * h];
        let (mut out, mut tmp) = (Vec::new(), Vec::new());
        blur(&flat, w, h, 2, &mut out, &mut tmp);
        assert_eq!(out, flat);

        let mut dot = vec![0u8; w * h];
        dot[3 * w + 4] = 255;
        blur(&dot, w, h, 1, &mut out, &mut tmp);
        assert!(out[3 * w + 4] < 255 && out[3 * w + 4] > 0);
        assert!(out[3 * w + 5] > 0 && out[2 * w + 4] > 0, "spread around");
        assert_eq!(out[0], 0, "not far");
        blur(&dot, w, h, 0, &mut out, &mut tmp);
        assert_eq!(out, dot, "radius 0 copies");
    }

    #[test]
    fn compositing_takes_the_person_from_the_frame_and_the_rest_from_the_background() {
        let mut fg = vec![200u8, 200, 200];
        composite(&mut fg, &[10, 10, 10], &[255, 0, 128]);
        assert_eq!(fg[0], 200, "the person");
        assert_eq!(fg[1], 10, "the background");
        assert!((100..=110).contains(&fg[2]), "the edge mixes: {}", fg[2]);
    }

    #[test]
    fn blur_compositing_on_a_tiny_frame_blurs_only_the_background() {
        // 8x2: a sharp checker; the person is the left half.
        let (w, h) = (8, 2);
        let frame: Vec<u8> = (0..w * h)
            .map(|i| if i % 2 == 0 { 220 } else { 30 })
            .collect();
        let alpha: Vec<u8> = (0..w * h)
            .map(|i| if i % w < 4 { 255 } else { 0 })
            .collect();
        let (mut blurred, mut tmp) = (Vec::new(), Vec::new());
        blur(&frame, w, h, 1, &mut blurred, &mut tmp);
        let mut out = frame.clone();
        composite(&mut out, &blurred, &alpha);
        assert_eq!(out[..4], frame[..4], "the person stays sharp");
        let spread = |v: &[u8]| v.iter().max().unwrap() - v.iter().min().unwrap();
        assert!(spread(&out[4..8]) < spread(&frame[4..8]) / 2);
    }

    #[test]
    fn a_replacement_covers_the_frame_and_keeps_its_colour() {
        let red = RgbFrame {
            width: 4,
            height: 2,
            data: [220, 30, 30].repeat(8),
        };
        let bg = cover_i420(&red, 8, 4);
        assert_eq!((bg.width, bg.height), (8, 4));
        let (y, u, v) = bg.planes();
        let [ey, eu, ev] = rgb_to_yuv([220, 30, 30]);
        assert!(y.iter().all(|&s| s == ey));
        assert!(u.iter().all(|&s| s == eu) && v.iter().all(|&s| s == ev));

        // Replacing: where the mask says background the frame becomes the picture.
        let mut frame = I420Frame::new(8, 4, vec![128; I420Frame::len_for(8, 4)]).unwrap();
        let alpha = vec![0u8; 32];
        let mut alpha_c = Vec::new();
        half(&alpha, 8, 4, &mut alpha_c);
        assert_eq!(alpha_c.len(), 8);
        let (fy, fu, fv) = frame.planes_mut();
        composite(fy, y, &alpha);
        composite(fu, u, &alpha_c);
        composite(fv, v, &alpha_c);
        assert_eq!(frame, bg);
    }

    #[test]
    fn a_picture_scaled_down_is_averaged_not_picked_and_keeps_its_colour_detail() {
        // A 1-pixel black and white checker covering 4x4 from 8x8: every pixel is the mid grey.
        let checker = RgbFrame {
            width: 8,
            height: 8,
            data: (0..64)
                .flat_map(|i| {
                    let v = if (i % 8 + i / 8) % 2 == 0 { 0 } else { 255 };
                    [v, v, v]
                })
                .collect(),
        };
        let bg = cover_i420(&checker, 4, 4);
        assert!(
            bg.planes().0.iter().all(|&y| (115..140).contains(&y)),
            "{:?}",
            bg.planes().0
        );
        // Red and blue columns, as wide as the frame: the chroma is the average of both.
        let columns = RgbFrame {
            width: 4,
            height: 2,
            data: (0..8)
                .flat_map(|i| {
                    if i % 2 == 0 {
                        [220, 30, 30]
                    } else {
                        [30, 30, 220]
                    }
                })
                .collect(),
        };
        let bg = cover_i420(&columns, 4, 2);
        let (u, v) = (bg.planes().1, bg.planes().2);
        let (red, blue) = (rgb_to_yuv([220, 30, 30]), rgb_to_yuv([30, 30, 220]));
        assert!(u[0].abs_diff(((u16::from(red[1]) + u16::from(blue[1])) / 2) as u8) <= 1);
        assert!(v[0].abs_diff(((u16::from(red[2]) + u16::from(blue[2])) / 2) as u8) <= 1);
    }

    #[test]
    fn a_mask_scales_up_smoothly_and_keeps_solid_areas() {
        let m = Mask {
            width: 2,
            height: 1,
            data: vec![0, 255],
        };
        let mut out = Vec::new();
        upscale(&m, 8, 2, &mut out);
        assert_eq!(out.len(), 16);
        assert_eq!(out[0], 0);
        assert_eq!(out[7], 255);
        assert!(out[..8].windows(2).all(|w| w[0] <= w[1]), "{out:?}");
        assert!(out[3] > 0 && out[4] < 255, "a soft edge in the middle");
        assert_eq!(out[..8], out[8..]);
    }

    #[test]
    fn probabilities_become_a_mask_with_a_short_edge_and_smooth_over_time() {
        let m = Mask::from_probabilities(&[0.1, 0.5, 0.9, 1.0], 2, 2);
        assert_eq!(m.data, [0, 128, 255, 255]);
        let mut prev = Mask {
            width: 2,
            height: 2,
            data: vec![0, 0, 255, 255],
        };
        smooth(&mut prev, &m);
        assert_eq!(prev.data, [0, 96, 255, 255]);
    }

    #[test]
    fn the_model_sees_the_average_of_a_fine_pattern_not_one_pixel_of_it() {
        // A 1-pixel checker of dark and bright luma, grey chroma: seen at a fifth of the size the
        // model gets the mid grey, not whichever pixel a nearest pick lands on.
        let (w, h) = (20, 20);
        let mut data: Vec<u8> = (0..w * h)
            .map(|i| if (i % w + i / w) % 2 == 0 { 16 } else { 235 })
            .collect();
        data.extend(vec![128u8; w * h / 2]);
        let f = I420Frame::new(w, h, data).unwrap();
        let input = model_input(&f, 4, 4);
        assert!(input.iter().all(|&v| (0.4..0.6).contains(&v)), "{input:?}");
    }

    /// A mask ramping from 0 to 255 over 8 pixels around x = 32.
    fn ramp(w: usize, h: usize) -> Vec<u8> {
        (0..w * h)
            .map(|i| (((i % w) as f32 - 28.0) / 8.0 * 255.0).clamp(0.0, 255.0) as u8)
            .collect()
    }

    fn between(row: &[u8]) -> usize {
        row.iter().filter(|&&a| (32..224).contains(&a)).count()
    }

    #[test]
    fn a_blurry_mask_snaps_to_the_edge_in_the_picture() {
        // The person is the bright right half; the coarse mask ramps over 8 pixels around it.
        let (w, h) = (64, 8);
        let guide: Vec<u8> = (0..w * h)
            .map(|i| if i % w < 32 { 40 } else { 200 })
            .collect();
        let coarse = ramp(w, h);
        assert_eq!(between(&coarse[3 * w..4 * w]), 6, "the ramp it starts as");
        let mut refiner = Refiner::default();
        let mut out = Vec::new();
        refiner.refine(&coarse, &guide, w, h, 8, &mut out);
        let row = &out[3 * w..4 * w];
        assert!(row[24] < 32 && row[28] < 32, "background: {row:?}");
        assert!(row[36] > 224 && row[40] > 224, "person: {row:?}");
        assert!(between(row) <= 3, "a sharp edge: {row:?}");
    }

    #[test]
    fn a_mask_stays_as_soft_as_it_was_where_the_picture_has_no_edge() {
        let (w, h) = (64, 8);
        let guide = vec![120u8; w * h];
        let mut refiner = Refiner::default();
        let mut out = Vec::new();
        refiner.refine(&ramp(w, h), &guide, w, h, 8, &mut out);
        assert!(between(&out[3 * w..4 * w]) >= 6, "still a ramp: {out:?}");
    }

    #[test]
    fn rgb_and_yuv_round_trip_closely() {
        for rgb in [[0, 0, 0], [255, 255, 255], [200, 40, 90], [30, 140, 220]] {
            let [y, u, v] = rgb_to_yuv(rgb);
            let back = yuv_to_rgb(y, u, v);
            for (a, b) in rgb.iter().zip(back) {
                assert!(a.abs_diff(b) <= 4, "{rgb:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn the_model_sees_the_frame_as_rgb_in_0_to_1() {
        let [y, u, v] = rgb_to_yuv([255, 0, 0]);
        let mut data = vec![y; 16];
        data.extend([u; 4]);
        data.extend([v; 4]);
        let f = I420Frame::new(4, 4, data).unwrap();
        let input = model_input(&f, 2, 2);
        assert_eq!(input.len(), 12);
        assert!(input[..4].iter().all(|&r| r > 0.95), "red");
        assert!(input[4..].iter().all(|&c| c < 0.08), "no green or blue");
    }
}
