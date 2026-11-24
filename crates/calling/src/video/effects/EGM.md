# video/effects

## Overview
Camera video effects, compiled only with the `effects` feature: background blur and replacement, colour filters, low-light brightening and soft focus. A `Processor` applies an `Effects` setting to I420 frames in place on a dedicated thread; person segmentation runs the bundled MediaPipe selfie model (256x144) on the CPU with `tract-onnx`.

## Interface
- `mod.rs`
  - `Background {None, Blur, StrongBlur, Image(Arc<RgbFrame>)}`, `Filter {None, Warm, Cool, Mono, Vivid}`, `Effects {background, filter, brighten, soft_focus}` with `is_off()`.
  - `EffectsControl::{new, set, get, version}`: settings shared between the UI and the worker; `set` bumps the version.
  - `Processor::{new, apply}`, `process(frames, control) -> Receiver<I420Frame>`, `next_every(every, took) -> u32`.
  - Re-exports `BUILT_IN`, `BuiltIn`, `built_in`; `pub mod pixels`, `pub mod segment`.
- `backgrounds.rs`: `BuiltIn {id, name}`, `BUILT_IN` (26 entries: the original dusk, aurora, office, bookshelf, sage and slate, then gradients and plain colours, bokeh, geometric patterns and rooms such as living-room, home-office, loft and studio), `built_in(id, w, h) -> Option<RgbFrame>`. The drawing of the newer ones is in the private submodule `backgrounds/extra.rs`.
- `pixels.rs`: `Lut`, `apply_lut`, `brighten_lut`, `brighten(y, u, v, w)` (luma LUT plus chroma gain), `filter_luts`, `blur`, `Mask` (`from_probabilities`), `smooth`, `upscale` (a thin wrapper over `scale_plane`), `scale_plane`, `halve_plane`, `Refiner` (`refine`: guided filter), `half`, `composite`, `yuv_to_rgb`, `rgb_to_yuv`, `cover_i420`, `model_input`.
- `segment.rs`: `MODEL_SIZE` (256, 144), `Segmenter::{new, probabilities, mask}`.

## Invariants
- The module exists only with feature `effects`; no further gates inside [../mod.rs:effects].
- Effects apply in a fixed order: brighten, soft focus, background, filter. When `Effects::is_off()` all cached state is released and the frame is untouched [mod.rs:Processor::apply].
- Model input is planar RGB in 0..1 at 256x144, tensor shape `[1, 3, h, w]` [segment.rs:Segmenter::probabilities, pixels.rs:model_input].
- Mask: 255 is person, 0 is background; probabilities below 0.25 map to 0, above 0.75 to 255, smoothstep between; the temporal mix is `(prev + 3 * next + 2) / 4` [pixels.rs:Mask::from_probabilities, smooth].
- The mask is not feathered with a blur at model size: `Processor::person` scales the 256x144 mask to a working size (the frame, or half of it when wider than `WORK_WIDTH` = 640), refines it with a guided filter (`pixels::Refiner`, luma as the guide, radius `REFINE_RADIUS` = 6 at 640 wide, `REFINE_CONTRAST` 1.3), scales it back to the frame and halves it for chroma. The buffers `work_mask`, `work_guide`, `work_alpha` and the refiner are released with the other state when effects are off [mod.rs:Processor::person, release, pixels.rs:Refiner].
- One mask serves at most 4 frames; the adaptive budget is 20 ms per frame [mod.rs:MASK_BUDGET, MASK_EVERY_MAX, next_every].
- Blur radii are defined for 640-wide frames and scaled by width / 640; chroma planes use half the radius and half-size dimensions [mod.rs:scaled].
- Colour conversions are BT.601 limited range; `brighten_lut` changes only luma in 16..=235, and `brighten` also scales each chroma sample by the gain its 2x2 luma block got (held between 1.0 and 1.4) so colours do not wash out [pixels.rs:yuv_to_rgb, brighten_lut, brighten].
- A replacement image is scaled to cover the frame and cached until the frame size or the settings version changes [mod.rs:replace_background].
- If the model fails to load, the background effect is skipped and the frame stays as is [mod.rs:Processor::person].

## Side Effects
- The ONNX model is embedded with `include_bytes!` from `assets/segmentation/selfie_segmentation.onnx`; `Segmenter::new` parses and optimizes it once per camera [segment.rs:MODEL].
- `process` spawns a std thread named `video-effects`; frames are dropped when its 2-frame queue is full; on spawn failure it logs a warning and still returns the receiver [mod.rs:process].
- `EffectsControl` shares a `Mutex<Arc<Effects>>` and an atomic version across threads.
- `tracing` warnings when the model or the thread cannot start. No environment variables, runtime file reads or network.

## Verification
- Run: `cargo test -p calling`
