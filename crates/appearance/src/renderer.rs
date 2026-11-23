//! Which renderer draws the window: the CPU (Slint's software renderer) or the GPU (femtovg).
//!
//! The choice is read before the Slint backend is selected, so before any window or runtime
//! exists: it lives in `general.json` ([`crate::GeneralPrefs::renderer`]) and
//! `IRONTEAMS_RENDERER` overrides it. The startup itself (and the fall-back when the GPU renderer
//! cannot start) is `ironteams/src/renderer.rs`.

use serde::{Deserialize, Serialize};

/// The renderer to draw with. The CPU one is the default: it keeps memory low
/// (`docs/adr/0001-ui-toolkit.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Renderer {
    /// Slint's software renderer.
    #[default]
    Cpu,
    /// femtovg on OpenGL.
    Gpu,
}

impl Renderer {
    /// Reads a saved value (`cpu`, `gpu`) or an `IRONTEAMS_RENDERER` value, which also takes
    /// Slint's own names (`software`, `femtovg`). Case and surrounding spaces do not matter.
    pub fn parse(s: &str) -> Option<Renderer> {
        match s.trim().to_ascii_lowercase().as_str() {
            "cpu" | "software" => Some(Renderer::Cpu),
            "gpu" | "femtovg" => Some(Renderer::Gpu),
            _ => None,
        }
    }

    /// The value that is saved and shown in Settings.
    pub fn slug(self) -> &'static str {
        match self {
            Renderer::Cpu => "cpu",
            Renderer::Gpu => "gpu",
        }
    }

    /// The name Slint's `BackendSelector::renderer_name` takes.
    pub fn slint_name(self) -> &'static str {
        match self {
            Renderer::Cpu => "software",
            Renderer::Gpu => "femtovg",
        }
    }
}

/// The renderer to start with: `IRONTEAMS_RENDERER` when it is set (an empty value counts as not
/// set; one that is not a renderer gives the CPU one), else the saved choice.
pub fn resolve(env: Option<&str>, saved: Renderer) -> Renderer {
    match env.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => Renderer::parse(v).unwrap_or_default(),
        None => saved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cpu_renderer_is_the_default() {
        assert_eq!(Renderer::default(), Renderer::Cpu);
        assert_eq!(resolve(None, Renderer::default()), Renderer::Cpu);
    }

    #[test]
    fn the_environment_beats_the_saved_choice() {
        assert_eq!(resolve(Some("femtovg"), Renderer::Cpu), Renderer::Gpu);
        assert_eq!(resolve(Some("software"), Renderer::Gpu), Renderer::Cpu);
        assert_eq!(resolve(Some("GPU"), Renderer::Cpu), Renderer::Gpu);
        assert_eq!(resolve(Some(" cpu "), Renderer::Gpu), Renderer::Cpu);
    }

    #[test]
    fn without_the_environment_the_saved_choice_decides() {
        assert_eq!(resolve(None, Renderer::Gpu), Renderer::Gpu);
        assert_eq!(resolve(None, Renderer::Cpu), Renderer::Cpu);
        assert_eq!(resolve(Some(""), Renderer::Gpu), Renderer::Gpu);
        assert_eq!(resolve(Some("  "), Renderer::Gpu), Renderer::Gpu);
    }

    #[test]
    fn an_unknown_value_falls_back_to_the_cpu_renderer() {
        assert_eq!(resolve(Some("vulkan"), Renderer::Gpu), Renderer::Cpu);
        assert_eq!(resolve(Some("skia"), Renderer::Gpu), Renderer::Cpu);
        assert_eq!(Renderer::parse("vulkan"), None);
    }

    #[test]
    fn parse_and_slug_round_trip() {
        for r in [Renderer::Cpu, Renderer::Gpu] {
            assert_eq!(Renderer::parse(r.slug()), Some(r));
            assert_eq!(Renderer::parse(r.slint_name()), Some(r));
        }
        assert_eq!(Renderer::Cpu.slint_name(), "software");
        assert_eq!(Renderer::Gpu.slint_name(), "femtovg");
    }

    #[test]
    fn the_saved_value_is_cpu_or_gpu_and_a_bad_one_means_cpu() {
        assert_eq!(serde_json::to_string(&Renderer::Gpu).unwrap(), "\"gpu\"");
        assert_eq!(serde_json::to_string(&Renderer::Cpu).unwrap(), "\"cpu\"");
        let p: crate::GeneralPrefs = serde_json::from_str(r#"{"renderer":"gpu"}"#).unwrap();
        assert_eq!(p.renderer, Renderer::Gpu);
        let p: crate::GeneralPrefs = serde_json::from_str(r#"{"renderer":"vulkan"}"#).unwrap();
        assert_eq!(p.renderer, Renderer::Cpu);
        let p: crate::GeneralPrefs = serde_json::from_str(r#"{"renderer":7}"#).unwrap();
        assert_eq!(p.renderer, Renderer::Cpu);
        // A file written before the setting existed.
        let p: crate::GeneralPrefs = serde_json::from_str(r#"{"language":"ar"}"#).unwrap();
        assert_eq!(p.renderer, Renderer::Cpu);
    }
}
