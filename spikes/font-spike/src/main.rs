//! Font spike for the theme engine (docs/designs/theme-engine.md, Open Question 1).
//!
//! Draws samples (Latin regular, semibold and bold, colour emoji, an Arabic sentence) with Slint's
//! software renderer, with `Window.default-font-family` set to installed families: before the
//! window is first drawn, and changed at run time on a window already drawn. Lists the families of
//! `slint::fontique_012::shared_collection()` and which cover basic Latin. On Linux the font chain
//! is built the way `main.rs::configure_fonts` builds it (fc-matched sans-serif in three weights,
//! then the bundled FluentEmoji3D and Twemoji). Writes PNGs and `findings.txt` to the directory in
//! `FONT_SPIKE_OUT` (default `font-spike-out`).

use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::fontique_012::fontique;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};

slint::slint! {
    export component Sample inherits Window {
        in property <string> family;
        in property <string> sample;
        in property <int> weight: 400;
        default-font-family: root.family;
        background: white;
        Text {
            x: 8px;
            y: 4px;
            text: root.sample;
            font-size: 28px;
            font-weight: root.weight;
            color: black;
        }
    }
}

const W: u32 = 900;
const H: u32 = 48;

thread_local!(static LAST: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) });

struct SpikePlatform;

impl Platform for SpikePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        w.set_size(PhysicalSize::new(W, H));
        LAST.with(|l| *l.borrow_mut() = Some(w.clone()));
        Ok(w)
    }
}

const SAMPLES: [(&str, &str, i32); 5] = [
    ("regular", "The quick brown fox 0123", 400),
    ("semibold", "The quick brown fox 0123", 600),
    ("bold", "The quick brown fox 0123", 700),
    ("emoji", "😀 🎉 👍 ❤️ 🚀 🇪🇬", 400),
    ("arabic", "مرحبا بكم في المحادثة", 400),
];

type Image = Vec<Rgb8Pixel>;

fn draw(w: &MinimalSoftwareWindow) -> Image {
    let mut buf = vec![Rgb8Pixel::default(); (W * H) as usize];
    w.request_redraw();
    w.draw_if_needed(|r| {
        r.render(&mut buf, W as usize);
    });
    buf
}

fn make(family: &str, text: &str, weight: i32) -> (Sample, Rc<MinimalSoftwareWindow>) {
    let ui = Sample::new().unwrap();
    ui.set_family(family.into());
    ui.set_sample(text.into());
    ui.set_weight(weight);
    ui.show().unwrap();
    let w = LAST.with(|l| l.borrow().clone()).unwrap();
    (ui, w)
}

/// Dark pixels (the text's ink) and saturated pixels (colour, only emoji have it).
fn stats(img: &Image) -> (usize, usize) {
    let ink = img
        .iter()
        .filter(|p| (p.r as u32 + p.g as u32 + p.b as u32) < 3 * 128)
        .count();
    let colour = img
        .iter()
        .filter(|p| {
            let mx = p.r.max(p.g).max(p.b);
            let mn = p.r.min(p.g).min(p.b);
            mx - mn > 60
        })
        .count();
    (ink, colour)
}

fn diff(a: &Image, b: &Image) -> usize {
    a.iter()
        .zip(b)
        .filter(|(x, y)| x.r != y.r || x.g != y.g || x.b != y.b)
        .count()
}

fn save(path: &Path, h: u32, px: &[Rgb8Pixel]) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), W, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().unwrap();
    let raw: Vec<u8> = px.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    wr.write_image_data(&raw).unwrap();
}

fn fc_match(pattern: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", pattern])
        .output()
        .ok()?;
    let path = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    path.is_file().then_some(path)
}

/// The chain `main.rs::configure_fonts` sets on Linux.
fn linux_chain(out: &mut String) {
    if !cfg!(target_os = "linux") {
        return;
    }
    let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/ironteams/ui/fonts");
    let mut chain: Vec<PathBuf> = ["sans-serif", "sans-serif:semibold", "sans-serif:bold"]
        .into_iter()
        .filter_map(fc_match)
        .collect();
    chain.dedup();
    chain.push(fonts.join("FluentEmoji3D.ttf"));
    chain.push(fonts.join("TwemojiMozilla.ttf"));
    writeln!(out, "SLINT_FONT_PATH chain: {chain:?}").unwrap();
    let joined = std::env::join_paths(chain).unwrap();
    // SAFETY: at the start of main, before any other thread exists.
    unsafe { std::env::set_var("SLINT_FONT_PATH", joined) };
}

/// Whether the family's default face maps every char A-Z and a-z.
fn covers_latin(collection: &mut fontique::Collection, id: fontique::FamilyId) -> bool {
    let Some(family) = collection.family(id) else {
        return false;
    };
    let Some(font) = family.default_font() else {
        return false;
    };
    let Some(blob) = font.load(None) else {
        return false;
    };
    let Some(map) = font.charmap_index().charmap(blob.data()) else {
        return false;
    };
    ('A'..='Z').chain('a'..='z').all(|c| map.map(c).is_some())
}

fn main() {
    let dir = PathBuf::from(std::env::var("FONT_SPIKE_OUT").unwrap_or("font-spike-out".into()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut out = String::new();
    writeln!(out, "os: {}", std::env::consts::OS).unwrap();
    linux_chain(&mut out);
    slint::platform::set_platform(Box::new(SpikePlatform)).unwrap();

    // Families of the shared collection.
    let start = std::time::Instant::now();
    let mut collection = slint::fontique_012::shared_collection();
    let ids: Vec<_> = collection.family_ids().collect();
    let mut latin = Vec::new();
    let mut not_latin = Vec::new();
    for id in ids {
        let name = collection.family_name(id).unwrap_or("").to_string();
        if covers_latin(&mut collection, id) {
            latin.push(name);
        } else {
            not_latin.push(name);
        }
    }
    latin.sort();
    latin.dedup();
    not_latin.sort();
    writeln!(
        out,
        "families: {} cover basic Latin, {} do not; listing took {:?}",
        latin.len(),
        not_latin.len(),
        start.elapsed()
    )
    .unwrap();
    writeln!(out, "Latin (first 60): {:?}", &latin[..latin.len().min(60)]).unwrap();
    writeln!(out, "not Latin: {not_latin:?}").unwrap();
    for g in [
        fontique::GenericFamily::SansSerif,
        fontique::GenericFamily::SystemUi,
        fontique::GenericFamily::Monospace,
        fontique::GenericFamily::Emoji,
    ] {
        let names: Vec<String> = collection
            .generic_families(g)
            .collect::<Vec<_>>()
            .into_iter()
            .map(|id| collection.family_name(id).unwrap_or("?").to_string())
            .collect();
        writeln!(out, "generic {g:?}: {names:?}").unwrap();
    }

    let candidates = [
        "DejaVu Serif",
        "Liberation Serif",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Noto Serif",
        "Ubuntu",
        "Georgia",
        "Times New Roman",
        "Consolas",
        "Courier New",
        "Menlo",
        "Helvetica Neue",
        "Segoe UI",
    ];
    let mut families: Vec<String> = vec![String::new()];
    families.extend(
        candidates
            .iter()
            .filter(|c| latin.iter().any(|l| l == *c))
            .take(4)
            .map(|c| c.to_string()),
    );
    families.push("NoSuchFamily".into());

    // Default family first: the reference the others are compared with.
    let mut reference: Vec<Image> = Vec::new();
    for family in &families {
        let label = if family.is_empty() { "default" } else { family };
        writeln!(out, "\n== family {label:?}").unwrap();
        let mut sheet: Vec<Rgb8Pixel> = Vec::new();
        let mut images = Vec::new();
        for (i, (name, text, weight)) in SAMPLES.iter().enumerate() {
            // Set before the first draw.
            let (ui, w) = make(family, text, *weight);
            let img = draw(&w);
            drop(ui);
            // Changed at run time on a window already drawn with the default family.
            let (ui, w) = make("", text, *weight);
            let _ = draw(&w);
            ui.set_family(family.as_str().into());
            let live = draw(&w);
            drop(ui);
            let (ink, colour) = stats(&img);
            let vs_default = reference.get(i).map(|r| diff(r, &img));
            writeln!(
                out,
                "{name:9} ink {ink:6} colour {colour:6} differs-from-default {vs_default:?} runtime-change-differs-from-first-draw {}",
                diff(&img, &live)
            )
            .unwrap();
            sheet.extend_from_slice(&img);
            images.push(img);
        }
        let (r, _) = stats(&images[0]);
        let (s, _) = stats(&images[1]);
        let (b, _) = stats(&images[2]);
        writeln!(
            out,
            "weights: semibold/regular ink {:.2}, bold/regular ink {:.2}",
            s as f64 / r.max(1) as f64,
            b as f64 / r.max(1) as f64
        )
        .unwrap();
        let file = format!("{}-{}.png", std::env::consts::OS, label.replace(' ', "_"));
        save(&dir.join(file), H * SAMPLES.len() as u32, &sheet);
        if reference.is_empty() {
            reference = images;
        }
    }

    println!("{out}");
    std::fs::write(
        dir.join(format!("findings-{}.txt", std::env::consts::OS)),
        out,
    )
    .unwrap();
}
