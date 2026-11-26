//! The theme share line: `ironteams-theme:1:<name>:<8 colours>`. Lines come from other people
//! (the clipboard, a chat message), so [`parse`] is a pure function with hard limits and uses
//! nothing from the line as a path, a font name or markup.

use crate::prefs::Rgb;
use crate::themes::Base;

pub const PREFIX: &str = "ironteams-theme:";
/// The longest line [`parse`] reads, in Unicode scalar values.
pub const MAX_LEN: usize = 200;
/// The longest line [`parse`] reads, in bytes. Checked first, so a huge line is never walked.
const MAX_BYTES: usize = 800;
/// The longest theme name, in Unicode scalar values.
pub const NAME_MAX: usize = 40;

/// Why a name is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    Colon,
    /// A control character, or a format character that can hide or reorder text.
    Hidden,
}

/// Why a share line is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareError {
    TooLong,
    NotATheme,
    Version,
    Name(NameError),
    Colours,
}

impl NameError {
    /// A stable code for the UI, which has the translated sentence for each.
    pub fn code(&self) -> &'static str {
        match self {
            NameError::Empty => "name-empty",
            NameError::TooLong => "name-too-long",
            NameError::Colon => "name-colon",
            NameError::Hidden => "name-hidden",
        }
    }
}

impl ShareError {
    /// A stable code for the UI, which has the translated sentence for each.
    pub fn code(&self) -> &'static str {
        match self {
            ShareError::TooLong => "too-long",
            ShareError::NotATheme => "not-a-theme",
            ShareError::Version => "version",
            ShareError::Name(e) => e.code(),
            ShareError::Colours => "colours",
        }
    }
}

/// The share line for a theme: lower-case `#rrggbb` colours in base-colour order.
pub fn export(name: &str, base: &Base) -> String {
    let colours: Vec<String> = base.0.iter().map(|c| c.to_hex()).collect();
    format!("{PREFIX}1:{name}:{}", colours.join(","))
}

/// Reads a share line (surrounding white space is ignored). The length limits come first, before
/// anything else is looked at.
pub fn parse(line: &str) -> Result<(String, Base), ShareError> {
    let line = line.trim();
    if line.len() > MAX_BYTES || line.chars().count() > MAX_LEN {
        return Err(ShareError::TooLong);
    }
    let rest = line.strip_prefix(PREFIX).ok_or(ShareError::NotATheme)?;
    let (version, rest) = rest.split_once(':').unwrap_or((rest, ""));
    if version != "1" {
        return Err(ShareError::Version);
    }
    let (name, colours) = rest.rsplit_once(':').ok_or(ShareError::Colours)?;
    let name = check_name(name).map_err(ShareError::Name)?;
    let base = parse_colours(colours).ok_or(ShareError::Colours)?;
    Ok((name.to_string(), base))
}

/// Exactly 8 colours, each `#` and 6 hex digits (any case).
fn parse_colours(s: &str) -> Option<Base> {
    let mut out = [Rgb::new(0, 0, 0); 8];
    let mut parts = s.split(',');
    for slot in &mut out {
        let p = parts.next()?;
        let digits = p.strip_prefix('#')?;
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        *slot = Rgb::from_hex(digits)?;
    }
    parts.next().is_none().then_some(Base(out))
}

/// The name, trimmed, when it is allowed: 1 to 40 Unicode scalar values, no `:`, no control
/// character, and no format (Cf) character except the zero-width joiners U+200C and U+200D.
pub fn check_name(s: &str) -> Result<&str, NameError> {
    let s = s.trim();
    let len = s.chars().count();
    if len == 0 {
        return Err(NameError::Empty);
    }
    if len > NAME_MAX {
        return Err(NameError::TooLong);
    }
    if s.contains(':') {
        return Err(NameError::Colon);
    }
    if s.chars().any(|c| c.is_control() || is_hidden_format(c)) {
        return Err(NameError::Hidden);
    }
    Ok(s)
}

/// Unicode category Cf (Unicode 15.1), as ranges. A table here avoids a crate for one check.
const CF: &[(u32, u32)] = &[
    (0x00AD, 0x00AD),
    (0x0600, 0x0605),
    (0x061C, 0x061C),
    (0x06DD, 0x06DD),
    (0x070F, 0x070F),
    (0x0890, 0x0891),
    (0x08E2, 0x08E2),
    (0x180E, 0x180E),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x2064),
    (0x2066, 0x206F),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
    (0x110BD, 0x110BD),
    (0x110CD, 0x110CD),
    (0x13430, 0x1343F),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0001, 0xE0001),
    (0xE0020, 0xE007F),
];

/// A Cf character other than U+200C and U+200D, which Arabic and Persian words need.
fn is_hidden_format(c: char) -> bool {
    let v = u32::from(c);
    if v == 0x200C || v == 0x200D {
        return false;
    }
    CF.iter().any(|&(lo, hi)| (lo..=hi).contains(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "ironteams-theme:1:Night shift:#1d2b45,#e6ecf5,#2f4a75,#ffffff,#1c2430,#2f6fd6,#1f57b8,#d93f3f";

    fn sample() -> Base {
        Base([
            Rgb::new(0x1d, 0x2b, 0x45),
            Rgb::new(0xe6, 0xec, 0xf5),
            Rgb::new(0x2f, 0x4a, 0x75),
            Rgb::new(0xff, 0xff, 0xff),
            Rgb::new(0x1c, 0x24, 0x30),
            Rgb::new(0x2f, 0x6f, 0xd6),
            Rgb::new(0x1f, 0x57, 0xb8),
            Rgb::new(0xd9, 0x3f, 0x3f),
        ])
    }

    /// A line with `name` and the sample colours.
    fn with_name(name: &str) -> String {
        format!("{PREFIX}1:{name}:#1d2b45,#e6ecf5,#2f4a75,#ffffff,#1c2430,#2f6fd6,#1f57b8,#d93f3f")
    }

    #[test]
    fn export_is_the_documented_line() {
        assert_eq!(export("Night shift", &sample()), LINE);
    }

    #[test]
    fn export_then_parse_round_trips() {
        for name in ["Night shift", "ليل", "Dusk (2)", "a"] {
            let line = export(name, &sample());
            assert_eq!(parse(&line), Ok((name.to_string(), sample())), "{name}");
        }
    }

    #[test]
    fn export_is_lower_case() {
        let line = export("x", &Base([Rgb::new(0xAB, 0xCD, 0xEF); 8]));
        assert!(!line.contains(|c: char| c.is_ascii_uppercase()), "{line}");
        assert!(line.contains("#abcdef,#abcdef"));
    }

    #[test]
    fn parse_accepts_upper_case_hex_and_white_space() {
        let line = format!(
            "  \n{PREFIX}1:Night shift:#1D2B45,#E6ECF5,#2F4A75,#FFFFFF,#1C2430,#2F6FD6,#1F57B8,#D93F3F\t "
        );
        assert_eq!(parse(&line), Ok(("Night shift".to_string(), sample())));
    }

    #[test]
    fn a_line_over_200_scalar_values_is_refused_first() {
        // 200 scalar values pass the length check (this one fails on its version instead).
        let at_limit = format!("{PREFIX}2:{}", "é".repeat(MAX_LEN - PREFIX.len() - 2));
        assert_eq!(at_limit.chars().count(), MAX_LEN);
        assert_eq!(parse(&at_limit), Err(ShareError::Version));
        // 201 do not, whatever else is wrong with the line.
        let over = format!("{at_limit}é");
        assert_eq!(over.chars().count(), MAX_LEN + 1);
        assert_eq!(parse(&over), Err(ShareError::TooLong));
        // A real line with a long tail.
        let tail = format!("{LINE}{}", " x".repeat(100));
        assert_eq!(parse(&tail), Err(ShareError::TooLong));
    }

    #[test]
    fn a_huge_line_is_refused_before_anything_else() {
        assert_eq!(parse(&"#".repeat(10_000)), Err(ShareError::TooLong));
        assert_eq!(parse(&"😀".repeat(5_000)), Err(ShareError::TooLong));
    }

    #[test]
    fn a_wrong_prefix_or_version_is_refused() {
        assert_eq!(
            parse(&LINE.replace("ironteams-theme", "foo")),
            Err(ShareError::NotATheme)
        );
        assert_eq!(parse(""), Err(ShareError::NotATheme));
        assert_eq!(
            parse(&LINE.replace("theme:1:", "theme:2:")),
            Err(ShareError::Version)
        );
        assert_eq!(
            parse(&LINE.replace("theme:1:", "theme:11:")),
            Err(ShareError::Version)
        );
    }

    #[test]
    fn names_are_checked() {
        use NameError::*;
        let long = "a".repeat(41);
        let cases = [
            ("", Empty),
            ("   ", Empty),
            (long.as_str(), TooLong),
            ("a:b", Colon),
            ("a\u{7}b", Hidden),
            ("a\u{202e}b", Hidden),
        ];
        for (name, want) in cases {
            assert_eq!(
                parse(&with_name(name)),
                Err(ShareError::Name(want)),
                "{name:?}"
            );
        }
        assert_eq!(
            parse(&with_name("a\u{200b}b")),
            Err(ShareError::Name(Hidden))
        );
        // 40 is allowed; surrounding white space is not part of the name.
        let forty = "b".repeat(40);
        assert_eq!(parse(&with_name(&forty)).unwrap().0, forty);
        assert_eq!(parse(&with_name("  Night ")).unwrap().0, "Night");
    }

    #[test]
    fn zero_width_joiners_are_allowed() {
        let name = "می\u{200c}خواهم \u{200d}x";
        assert_eq!(parse(&with_name(name)).unwrap().0, name);
        assert_eq!(check_name("a\u{200c}b"), Ok("a\u{200c}b"));
        assert_eq!(check_name("a\u{200d}b"), Ok("a\u{200d}b"));
    }

    #[test]
    fn every_listed_format_character_is_hidden() {
        for &(lo, hi) in CF {
            for v in [lo, hi] {
                let c = char::from_u32(v).unwrap();
                if v == 0x200c || v == 0x200d {
                    continue;
                }
                assert_eq!(
                    check_name(&format!("a{c}b")),
                    Err(NameError::Hidden),
                    "U+{v:04X}"
                );
            }
        }
    }

    #[test]
    fn colours_must_be_exactly_eight_hash_hex() {
        let seven = "ironteams-theme:1:n:#1d2b45,#e6ecf5,#2f4a75,#ffffff,#1c2430,#2f6fd6,#1f57b8";
        let nine = format!("{LINE},#000000");
        let bad_digit = LINE.replace("#d93f3f", "#12345g");
        let no_hash = LINE.replace("#d93f3f", "d93f3f");
        let short = LINE.replace("#d93f3f", "#d93f3");
        for line in [seven.to_string(), nine, bad_digit, no_hash, short] {
            assert_eq!(parse(&line), Err(ShareError::Colours), "{line}");
        }
        assert_eq!(
            parse("ironteams-theme:1:onlyname"),
            Err(ShareError::Colours)
        );
        assert_eq!(parse("ironteams-theme:1"), Err(ShareError::Colours));
    }

    #[test]
    fn every_code_is_distinct() {
        let all = [
            ShareError::TooLong,
            ShareError::NotATheme,
            ShareError::Version,
            ShareError::Name(NameError::Empty),
            ShareError::Name(NameError::TooLong),
            ShareError::Name(NameError::Colon),
            ShareError::Name(NameError::Hidden),
            ShareError::Colours,
        ];
        let codes: std::collections::BTreeSet<&str> = all.iter().map(|e| e.code()).collect();
        assert_eq!(codes.len(), all.len());
        assert_eq!(ShareError::TooLong.code(), "too-long");
        assert_eq!(ShareError::Name(NameError::Hidden).code(), "name-hidden");
    }
}
