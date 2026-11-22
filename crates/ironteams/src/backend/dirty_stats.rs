//! Counts the dirty marks by the line that made them, for the debug log (`docs/perf.md`): how
//! often the chat list and the messages are rebuilt, and why. Logged once a minute at debug level
//! (`IRONTEAMS_LOG=ironteams=debug`).

use std::collections::HashMap;
use std::panic::Location;
use std::time::{Duration, Instant};

/// How often the counts are logged.
pub(super) const EVERY: Duration = Duration::from_secs(60);

/// The parts of the view a mark can set, in the order of their bits.
pub(super) const PARTS: [&str; 6] = ["chats", "messages", "teams", "header", "scroll", "activity"];

/// The marks since the last summary: by the line that made them and the parts they set.
pub(super) struct MarkStats {
    since: Instant,
    by_site: HashMap<(&'static Location<'static>, u8), u32>,
}

impl MarkStats {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            since: now,
            by_site: HashMap::new(),
        }
    }

    /// One mark made at `site`, setting the parts in `mask` (bits in the order of `PARTS`).
    pub(super) fn record(&mut self, site: &'static Location<'static>, mask: u8) {
        *self.by_site.entry((site, mask)).or_default() += 1;
    }

    /// The summary of the marks once `EVERY` has passed since the last one (the counts then start
    /// over), else `None`. A minute without marks gives no summary.
    pub(super) fn take_due(&mut self, now: Instant) -> Option<String> {
        if now.duration_since(self.since) < EVERY {
            return None;
        }
        self.since = now;
        let counts = std::mem::take(&mut self.by_site);
        (!counts.is_empty()).then(|| summary(&counts))
    }
}

/// The parts named by `mask`, joined with `+` ("chats+messages").
fn parts(mask: u8) -> String {
    let names: Vec<&str> = PARTS
        .iter()
        .enumerate()
        .filter(|(i, _)| mask & (1 << i) != 0)
        .map(|(_, n)| *n)
        .collect();
    if names.is_empty() {
        "none".into()
    } else {
        names.join("+")
    }
}

/// "chats=3 messages=12 | backend/media.rs:870 messages x12, ...": the marks per part, then per
/// line, the busiest first.
fn summary(counts: &HashMap<(&'static Location<'static>, u8), u32>) -> String {
    let mut totals = [0u32; PARTS.len()];
    for ((_, mask), n) in counts {
        for (i, t) in totals.iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                *t += n;
            }
        }
    }
    let head: Vec<String> = PARTS
        .iter()
        .zip(totals)
        .filter(|(_, t)| *t > 0)
        .map(|(p, t)| format!("{p}={t}"))
        .collect();
    let mut sites: Vec<_> = counts.iter().collect();
    sites.sort_by(|a, b| {
        b.1.cmp(a.1)
            .then_with(|| a.0 .0.file().cmp(b.0 .0.file()))
            .then_with(|| a.0 .0.line().cmp(&b.0 .0.line()))
            .then_with(|| a.0 .1.cmp(&b.0 .1))
    });
    let lines: Vec<String> = sites
        .iter()
        .map(|((site, mask), n)| format!("{}:{} {} x{n}", site.file(), site.line(), parts(*mask)))
        .collect();
    format!("{} | {}", head.join(" "), lines.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHATS: u8 = 1;
    const MESSAGES: u8 = 2;

    #[test]
    fn nothing_is_logged_before_a_minute() {
        let t0 = Instant::now();
        let mut s = MarkStats::new(t0);
        s.record(Location::caller(), MESSAGES);
        assert_eq!(s.take_due(t0 + Duration::from_secs(59)), None);
    }

    #[test]
    fn a_minute_of_marks_is_summed_per_part_and_per_line_then_starts_over() {
        let t0 = Instant::now();
        let mut s = MarkStats::new(t0);
        let a = Location::caller();
        let b = Location::caller();
        for _ in 0..3 {
            s.record(a, MESSAGES);
        }
        s.record(b, CHATS | MESSAGES);
        let line = s.take_due(t0 + EVERY).unwrap();
        let (head, sites) = line.split_once(" | ").unwrap();
        assert_eq!(head, "chats=1 messages=4");
        // The busiest line comes first.
        let first = format!("{}:{} messages x3", a.file(), a.line());
        let second = format!("{}:{} chats+messages x1", b.file(), b.line());
        assert_eq!(sites, format!("{first}, {second}"));
        // The counts started over: a quiet minute logs nothing.
        assert_eq!(s.take_due(t0 + EVERY * 2), None);
    }

    #[test]
    fn a_mark_sets_its_parts_over_the_pending_ones_and_names_them_as_bits() {
        let mut pending = super::super::Dirty {
            teams: true,
            ..Default::default()
        };
        let set = super::super::Dirty {
            chats: true,
            messages: true,
            ..Default::default()
        };
        assert_eq!(set.mask(), CHATS | MESSAGES);
        assert_eq!(parts(set.mask()), "chats+messages");
        pending.merge(&set);
        assert!(pending.chats && pending.messages && pending.teams);
        assert!(!pending.scroll && !pending.header && !pending.activity);
        // A mark that sets nothing leaves the pending parts as they were.
        pending.merge(&Default::default());
        assert_eq!(pending.mask(), set.mask() | 1 << 2);
    }
}
