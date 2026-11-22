//! UI-thread side of video clips: fills `ClipStore` (the recorder in the composer) and wires its
//! callbacks, and the clip cards' play button, to `Cmd::Clip`.

use super::UiHandle;
use crate::backend::{ClipCmd, Cmd};
use crate::ui_api::{ClipView, Picture};
use crate::{AppWindow, ClipStore};
use slint::{ComponentHandle, Image, Rgb8Pixel, SharedPixelBuffer};
use tokio::sync::mpsc::UnboundedSender;

fn image(p: &Picture) -> Option<Image> {
    if p.rgb.len() != (p.width * p.height * 3) as usize || p.width == 0 {
        return None;
    }
    Some(Image::from_rgb8(
        SharedPixelBuffer::<Rgb8Pixel>::clone_from_slice(&p.rgb, p.width, p.height),
    ))
}

impl UiHandle {
    pub fn clip_recorder(&self, v: ClipView) {
        self.run(move |w: AppWindow| {
            let s = w.global::<ClipStore>();
            s.set_state(v.state.into());
            s.set_elapsed_ms(v.elapsed_ms);
            s.set_limit_ms(v.limit_ms);
            match v.picture.as_ref().and_then(image) {
                Some(img) => {
                    s.set_picture(img);
                    s.set_has_picture(true);
                }
                None => s.set_has_picture(false),
            }
            s.set_can_record(v.can_record);
            s.set_problem(v.problem.into());
        });
    }
}

/// Connects the store's callbacks to the backend.
pub fn wire(window: &AppWindow, tx: UnboundedSender<Cmd>) {
    let s = window.global::<ClipStore>();
    let on = |c: ClipCmd| {
        let tx = tx.clone();
        move || {
            let _ = tx.send(Cmd::Clip(c.clone()));
        }
    };
    s.on_open(on(ClipCmd::Open));
    s.on_record(on(ClipCmd::Record));
    s.on_stop(on(ClipCmd::Stop));
    s.on_retake(on(ClipCmd::Retake));
    s.on_cancel(on(ClipCmd::Cancel));
    s.on_send(on(ClipCmd::Send));
    s.on_play_take(on(ClipCmd::PlayTake));
    s.on_play(move |url| {
        let _ = tx.send(Cmd::Clip(ClipCmd::Play(url.into())));
    });
}
