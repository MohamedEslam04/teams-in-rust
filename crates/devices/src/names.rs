//! Turning what the Linux sound stack reports into the list a user should see: PulseAudio or
//! PipeWire sinks and sources by their descriptions, or, without a sound server, the ALSA
//! entries that are a real choice. Pure functions, so they run (and are tested) on any OS.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::AudioDevice;
use serde::Deserialize;

/// Prefix of the id of a sink or source of the sound server: `pulse:<node name>`.
pub(crate) const PULSE_PREFIX: &str = "pulse:";

/// One sink or source as `pactl -f json list sinks|sources` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Node {
    /// The server's name for it (what `PULSE_SINK` / `PULSE_SOURCE` take).
    pub name: String,
    /// What the desktop shows, for example `Built-in Audio Analog Stereo`.
    pub description: String,
    pub channels: u16,
}

#[derive(Deserialize)]
struct RawNode {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    sample_specification: String,
    /// Set on a source that records what a sink plays.
    #[serde(default)]
    monitor_of_sink: Option<serde_json::Value>,
}

/// Channels of a `s32le 2ch 48000Hz` sample specification; stereo when it says nothing.
fn channels_of(spec: &str) -> u16 {
    spec.split_whitespace()
        .find_map(|w| w.strip_suffix("ch")?.parse().ok())
        .unwrap_or(2)
}

/// The nodes of `pactl -f json list sinks|sources`. Monitors (a sink's "what you hear" source) are
/// left out: they are not microphones. `None` when the text is not that JSON.
pub(crate) fn parse_nodes(json: &str) -> Option<Vec<Node>> {
    let raw: Vec<RawNode> = serde_json::from_str(json).ok()?;
    let mut nodes: Vec<Node> = raw
        .into_iter()
        .filter(|n| {
            let monitor = n.name.ends_with(".monitor")
                || n.monitor_of_sink.as_ref().is_some_and(|v| !v.is_null());
            !monitor && !n.name.is_empty()
        })
        .map(|n| Node {
            description: if n.description.is_empty() {
                n.name.clone()
            } else {
                n.description
            },
            channels: channels_of(&n.sample_specification),
            name: n.name,
        })
        .collect();
    label_duplicates(&mut nodes);
    Some(nodes)
}

/// Two devices that the server describes alike (a pair of the same USB microphone) get
/// ` (2)`, ` (3)`… so the list never shows the same label twice.
pub(crate) fn label_duplicates(nodes: &mut [Node]) {
    let mut seen: Vec<(String, usize)> = Vec::new();
    for n in nodes {
        let base = n.description.clone();
        match seen.iter_mut().find(|(d, _)| *d == base) {
            Some((_, count)) => {
                *count += 1;
                n.description = format!("{base} ({count})");
            }
            None => seen.push((base, 1)),
        }
    }
}

/// The list entry of a node; `default` is the server's current default sink or source name.
pub(crate) fn node_device(node: &Node, default: Option<&str>) -> AudioDevice {
    AudioDevice {
        id: format!("{PULSE_PREFIX}{}", node.name),
        name: node.description.clone(),
        is_default: default == Some(node.name.as_str()),
        channels: node.channels,
        // The server converts whatever the stream asks for.
        sample_rates: vec![44_100, 48_000],
    }
}

/// The node name a saved or listed id stands for; `None` for the default and for any id that is
/// not the sound server's (an old `alsa:…` one), which then means the default as well.
pub(crate) fn pulse_node(id: Option<&str>) -> Option<&str> {
    id?.strip_prefix(PULSE_PREFIX).filter(|n| !n.is_empty())
}

/// The ALSA PCM name of a cpal id (`alsa:sysdefault:CARD=PCH` is `sysdefault`).
fn alsa_pcm(id: &str) -> Option<&str> {
    let rest = id.strip_prefix("alsa:")?;
    Some(rest.split(':').next().unwrap_or(rest))
}

/// Without a sound server, the ALSA entries worth choosing: the default and one per card and
/// output (`sysdefault`, `hdmi`). The rest is plumbing (`pulse`, `null`, `front`, `surround51`,
/// `dmix`, `hw:`…): it repeats a card many times over or is not a device at all. Ids that are
/// not ALSA's (other hosts) are kept.
pub(crate) fn is_real_alsa_choice(id: &str) -> bool {
    match alsa_pcm(id) {
        None => true,
        Some(pcm) => matches!(pcm, "default" | "sysdefault" | "hdmi"),
    }
}

/// ALSA hints describe the default as `Default ALSA Output (currently PulseAudio Sound Server)`
/// and list the rest as `Card, Device` over a second line of explanation.
pub(crate) fn friendly_alsa_name(id: &str, name: &str) -> String {
    if id == "alsa:default" {
        return "Default".into();
    }
    name.lines().next().unwrap_or(name).trim().to_string()
}

/// Keeps the first of the entries that carry the same name.
pub(crate) fn dedup_by_name(list: Vec<AudioDevice>) -> Vec<AudioDevice> {
    let mut seen: Vec<String> = Vec::new();
    list.into_iter()
        .filter(|d| {
            if seen.contains(&d.name) {
                false
            } else {
                seen.push(d.name.clone());
                true
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from this machine's `pactl -f json list sinks`.
    const SINKS: &str = r#"[
      {"index":55,"state":"SUSPENDED","name":"alsa_output.pci-0000_00_1f.3.analog-stereo",
       "description":"Built-in Audio Analog Stereo","sample_specification":"s32le 2ch 48000Hz",
       "monitor_source":"alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"},
      {"index":56,"name":"alsa_output.pci-0000_01_00.1.hdmi-stereo",
       "description":"GA107 High Definition Audio Controller Digital Stereo (HDMI)",
       "sample_specification":"s32le 2ch 48000Hz"},
      {"index":57,"name":"bluez_output.AA_BB.1","description":"soundcore R50i",
       "sample_specification":"s16le 2ch 48000Hz"}
    ]"#;

    const SOURCES: &str = r#"[
      {"index":1,"name":"alsa_output.pci-0000_00_1f.3.analog-stereo.monitor",
       "description":"Monitor of Built-in Audio Analog Stereo","monitor_of_sink":"alsa_output.pci-0000_00_1f.3.analog-stereo",
       "sample_specification":"s32le 2ch 48000Hz"},
      {"index":2,"name":"alsa_input.pci-0000_00_1f.3.analog-stereo",
       "description":"Built-in Audio Analog Stereo","monitor_of_sink":null,
       "sample_specification":"s32le 2ch 48000Hz"},
      {"index":3,"name":"weird.monitor","description":"Monitor without the field"}
    ]"#;

    #[test]
    fn sinks_are_listed_by_their_descriptions() {
        let nodes = parse_nodes(SINKS).unwrap();
        let names: Vec<_> = nodes.iter().map(|n| n.description.as_str()).collect();
        assert_eq!(
            names,
            [
                "Built-in Audio Analog Stereo",
                "GA107 High Definition Audio Controller Digital Stereo (HDMI)",
                "soundcore R50i"
            ]
        );
        assert_eq!(nodes[2].name, "bluez_output.AA_BB.1");
        assert_eq!(nodes[0].channels, 2);
    }

    #[test]
    fn a_microphone_list_holds_no_monitors_and_so_no_output() {
        let nodes = parse_nodes(SOURCES).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "alsa_input.pci-0000_00_1f.3.analog-stereo");
    }

    #[test]
    fn text_that_is_not_the_listing_is_none() {
        assert!(parse_nodes("Connection failure: Connection refused").is_none());
        assert_eq!(parse_nodes("[]"), Some(vec![]));
    }

    #[test]
    fn the_same_description_twice_is_told_apart() {
        let json = r#"[{"name":"a","description":"USB Mic"},{"name":"b","description":"USB Mic"},
                       {"name":"c","description":"USB Mic"},{"name":"d","description":"Other"}]"#;
        let labels: Vec<_> = parse_nodes(json)
            .unwrap()
            .into_iter()
            .map(|n| n.description)
            .collect();
        assert_eq!(labels, ["USB Mic", "USB Mic (2)", "USB Mic (3)", "Other"]);
    }

    #[test]
    fn a_node_without_a_description_is_called_by_its_name() {
        let nodes = parse_nodes(r#"[{"name":"alsa_input.x"}]"#).unwrap();
        assert_eq!(nodes[0].description, "alsa_input.x");
    }

    #[test]
    fn the_server_default_is_flagged_and_ids_carry_the_node_name() {
        let nodes = parse_nodes(SINKS).unwrap();
        let list: Vec<_> = nodes
            .iter()
            .map(|n| node_device(n, Some("bluez_output.AA_BB.1")))
            .collect();
        assert_eq!(
            list.iter().map(|d| d.is_default).collect::<Vec<_>>(),
            [false, false, true]
        );
        assert_eq!(list[2].id, "pulse:bluez_output.AA_BB.1");
    }

    #[test]
    fn only_a_pulse_id_names_a_node() {
        assert_eq!(pulse_node(Some("pulse:abc")), Some("abc"));
        assert_eq!(pulse_node(None), None);
        // Saved before the sound server list existed: the default.
        assert_eq!(pulse_node(Some("alsa:front:CARD=PCH,DEV=0")), None);
        assert_eq!(pulse_node(Some("alsa:default")), None);
        assert_eq!(pulse_node(Some("pulse:")), None);
    }

    #[test]
    fn alsa_keeps_the_default_and_one_entry_per_output_and_drops_plumbing() {
        for keep in [
            "alsa:default",
            "alsa:sysdefault:CARD=PCH",
            "alsa:hdmi:CARD=PCH,DEV=0",
            "wasapi:{0.0.0.00000000}.abc",
        ] {
            assert!(is_real_alsa_choice(keep), "{keep}");
        }
        for gone in [
            "alsa:null",
            "alsa:pulse",
            "alsa:pipewire",
            "alsa:jack",
            "alsa:oss",
            "alsa:front:CARD=PCH,DEV=0",
            "alsa:surround51:CARD=PCH,DEV=0",
            "alsa:iec958:CARD=PCH,DEV=0",
            "alsa:dmix:CARD=PCH,DEV=0",
            "alsa:hw:CARD=0,DEV=0",
            "alsa:plughw:CARD=0,DEV=0",
        ] {
            assert!(!is_real_alsa_choice(gone), "{gone}");
        }
    }

    #[test]
    fn alsa_names_lose_their_second_line_and_the_default_its_plumbing_talk() {
        assert_eq!(
            friendly_alsa_name(
                "alsa:default",
                "Default ALSA Output (currently PulseAudio Sound Server)"
            ),
            "Default"
        );
        assert_eq!(
            friendly_alsa_name(
                "alsa:sysdefault:CARD=PCH",
                "HDA Intel PCH, ALC257 Analog\nDefault Audio Device"
            ),
            "HDA Intel PCH, ALC257 Analog"
        );
    }

    #[test]
    fn identical_alsa_names_collapse_to_the_first() {
        let d = |id: &str, name: &str| AudioDevice {
            id: id.into(),
            name: name.into(),
            is_default: false,
            channels: 2,
            sample_rates: vec![],
        };
        let out = dedup_by_name(vec![
            d("alsa:sysdefault:CARD=PCH", "HDA Intel PCH, ALC257 Analog"),
            d("alsa:front:CARD=PCH", "HDA Intel PCH, ALC257 Analog"),
            d("alsa:hdmi:CARD=PCH,DEV=0", "HDA Intel PCH, HDMI 0"),
        ]);
        assert_eq!(
            out.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            ["alsa:sysdefault:CARD=PCH", "alsa:hdmi:CARD=PCH,DEV=0"]
        );
    }

    #[test]
    fn channels_come_from_the_sample_specification() {
        assert_eq!(channels_of("s16le 1ch 16000Hz"), 1);
        assert_eq!(channels_of("float32le 6ch 48000Hz"), 6);
        assert_eq!(channels_of(""), 2);
    }
}
