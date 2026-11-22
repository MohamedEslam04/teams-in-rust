//! The card host: the same executable started as `ironteams --toast-host`. It reads cards from
//! stdin, draws them with `CardHost` (`ui/message_toast.slint`) and writes what the user did to
//! stdout, one JSON line each (`protocol`). Nothing in it knows about Teams.
//!
//! The stack logic ([`plan`], [`apply`], the callbacks in [`wire`]) is shared by every backend and
//! runs in the UI tests; the backends only size and place the window: [`run_window`] here for
//! X11 (XWayland), Windows and macOS, `layer.rs` for the Wayland layer shell.

use super::geometry::{self, MAX_CARDS};
use super::protocol::{self, Action, Backend, Card, FromHost, Kind, ToHost};
use super::select;
use crate::{CardHost, MessageToastStore, ToastCard};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::io::{BufRead, Write};
use std::rc::Rc;

/// Writes one message to the app (stdout, flushed: the app reads line by line).
pub fn emit(msg: &FromHost) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", protocol::line(msg));
    let _ = out.flush();
}

/// The cards on the screen and where their news goes.
pub struct Core {
    pub model: Rc<VecModel<ToastCard>>,
    /// Where the user's actions go (stdout in the host, a log in the tests).
    pub out: Rc<dyn Fn(FromHost)>,
    /// The stack changed: the backend resizes (or hides) the window. Called on the UI thread,
    /// possibly from inside a card's callback, so a backend only marks or schedules the work.
    pub relayout: Rc<dyn Fn()>,
}

impl Core {
    pub fn new(out: impl Fn(FromHost) + 'static, relayout: impl Fn() + 'static) -> Core {
        Core {
            model: Rc::new(VecModel::default()),
            out: Rc::new(out),
            relayout: Rc::new(relayout),
        }
    }

    fn act(&self, id: u32, action: Action) {
        (self.out)(FromHost::Action { id, action });
    }

    /// Takes the card out of the stack; says it is gone.
    fn remove(&self, id: u32) -> bool {
        let Some(i) = self.model.iter().position(|c| c.id as u32 == id) else {
            return false;
        };
        self.model.remove(i);
        self.act(id, Action::Closed);
        (self.relayout)();
        true
    }
}

/// Where a shown card goes in the stack.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// A card with this id is there: replace it in place.
    Replace(usize),
    /// Append it at the bottom after taking out the cards at these positions (ascending).
    Append { evict: Vec<usize> },
}

/// The stack is `stack` (ids and kinds, oldest first). A new card goes at the bottom; when the
/// stack is full the oldest card goes, but a ringing call only when nothing else can.
pub fn plan(stack: &[(u32, Kind)], id: u32, max: usize) -> Plan {
    if let Some(i) = stack.iter().position(|(c, _)| *c == id) {
        return Plan::Replace(i);
    }
    let mut evict: Vec<usize> = Vec::new();
    let mut left = stack.len();
    let mut candidates = stack
        .iter()
        .enumerate()
        .filter(|(_, (_, k))| *k != Kind::Call)
        .map(|(i, _)| i)
        .chain(
            stack
                .iter()
                .enumerate()
                .filter(|(_, (_, k))| *k == Kind::Call)
                .map(|(i, _)| i),
        );
    while left >= max.max(1) {
        let Some(i) = candidates.next() else { break };
        evict.push(i);
        left -= 1;
    }
    evict.sort_unstable();
    Plan::Append { evict }
}

fn kind_of(s: &str) -> Kind {
    match s {
        "meeting" => Kind::Meeting,
        "reminder" => Kind::Reminder,
        "call" => Kind::Call,
        _ => Kind::Message,
    }
}

/// A card button's action (`MessageToastStore.act`).
pub fn parse_act(s: &str) -> Option<Action> {
    Some(match s {
        "join" => Action::Join,
        "dismiss" => Action::Dismiss,
        "accept" => Action::Accept,
        "decline" => Action::Decline,
        "open" => Action::Open,
        "settings" => Action::Settings,
        _ => Action::Snooze {
            minutes: s.strip_prefix("snooze:")?.parse().ok().filter(|m| *m > 0)?,
        },
    })
}

fn rtl(s: &str) -> bool {
    render::base_direction(s) == render::Dir::Rtl
}

/// The Slint card for a card from the app. The card's number stands in for the chat, the reply
/// target and the message, so the card's callbacks name it.
pub fn to_toast(c: &Card) -> ToastCard {
    let avatar = c
        .avatar
        .as_deref()
        .and_then(|p| slint::Image::load_from_path(std::path::Path::new(p)).ok());
    let key: slint::SharedString = c.id.to_string().into();
    ToastCard {
        id: c.id as i32,
        kind: c.kind.as_str().into(),
        conversation: key.clone(),
        target: key.clone(),
        message: key,
        title: c.title.as_str().into(),
        sender: c.sender.as_str().into(),
        text: c.text.as_str().into(),
        initials: c.initials.as_str().into(),
        color: slint::Color::from_rgb_u8(
            (c.color >> 16) as u8,
            (c.color >> 8) as u8,
            c.color as u8,
        ),
        has_avatar: avatar.is_some(),
        avatar: avatar.unwrap_or_default(),
        meeting: c.meeting,
        can_reply: c.can_reply,
        can_mute: c.can_mute,
        can_join: c.can_join,
        rtl_title: rtl(&c.title),
        rtl_text: rtl(&c.text),
    }
}

/// Applies one message from the app. True when the host should quit.
pub fn apply(core: &Core, msg: ToHost) -> bool {
    match msg {
        ToHost::Show(card) => {
            let stack: Vec<(u32, Kind)> = core
                .model
                .iter()
                .map(|c| (c.id as u32, kind_of(&c.kind)))
                .collect();
            match plan(&stack, card.id, MAX_CARDS) {
                Plan::Replace(i) => core.model.set_row_data(i, to_toast(&card)),
                Plan::Append { evict } => {
                    for i in evict.into_iter().rev() {
                        let id = core.model.row_data(i).map(|c| c.id as u32);
                        core.model.remove(i);
                        if let Some(id) = id {
                            core.act(id, Action::Closed);
                        }
                    }
                    core.model.push(to_toast(&card));
                }
            }
            (core.relayout)();
            false
        }
        ToHost::Dismiss { id } => {
            core.remove(id);
            false
        }
        ToHost::Quit => true,
    }
}

fn id_of(s: &str) -> Option<u32> {
    s.parse().ok()
}

/// Gives the window the stack and turns the cards' callbacks into actions.
pub fn wire(host: &CardHost, core: &Rc<Core>) {
    let s = host.global::<MessageToastStore>();
    s.set_toasts(ModelRc::from(core.model.clone()));
    let c = core.clone();
    s.on_dismiss(move |id| {
        c.remove(id as u32);
    });
    let c = core.clone();
    s.on_reply(move |target, text| {
        if let Some(id) = id_of(&target) {
            c.act(
                id,
                Action::Reply {
                    text: text.to_string(),
                },
            );
        }
    });
    let c = core.clone();
    s.on_react(move |message, key| {
        if let Some(id) = id_of(&message) {
            c.act(
                id,
                Action::React {
                    key: key.to_string(),
                },
            );
        }
    });
    let c = core.clone();
    s.on_open(move |conversation| {
        if let Some(id) = id_of(&conversation) {
            c.act(id, Action::Open);
        }
    });
    let c = core.clone();
    s.on_mute(move |conversation| {
        if let Some(id) = id_of(&conversation) {
            c.act(id, Action::Mute);
        }
    });
    let c = core.clone();
    s.on_act(move |id, action| {
        if let Some(a) = parse_act(&action) {
            c.act(id as u32, a);
        }
    });
}

/// The host's language follows the app's (Settings › Language).
pub(super) fn apply_language() {
    crate::language::apply(&crate::backend::quiet::load_prefs());
}

/// Runs the host until the app closes its stdin. Called from `main` for `--toast-host`.
pub fn run() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    let probe = super::layer::probe;
    #[cfg(not(target_os = "linux"))]
    let probe = || false;
    let caps = select::detect(probe);
    let Some(backend) = select::choose(select::Os::current(), &caps, select::forced()) else {
        emit(&FromHost::Unavailable {
            reason: format!("no way to draw cards in this session ({caps:?})"),
        });
        return Ok(());
    };
    let result = match backend {
        #[cfg(target_os = "linux")]
        Backend::LayerShell => super::layer::run(),
        Backend::X11 => {
            // winit picks Wayland whenever it can; XWayland needs it hidden.
            // SAFETY: the host's only thread so far; nothing else reads the environment yet.
            unsafe {
                std::env::remove_var("WAYLAND_DISPLAY");
                std::env::remove_var("WAYLAND_SOCKET");
            }
            run_window(Backend::X11)
        }
        other => run_window(other),
    };
    if let Err(err) = &result {
        emit(&FromHost::Unavailable {
            reason: format!("{backend:?}: {err}"),
        });
    }
    result
}

thread_local! {
    /// The window backend's state, on the UI thread (the stdin thread posts messages to it).
    static WINDOW: RefCell<Option<(Rc<Core>, slint::Weak<CardHost>)>> = const { RefCell::new(None) };
}

/// The window attributes of the card window: no frame, above the others, not taking the focus
/// when it appears, and out of the taskbar.
fn window_attributes(
    a: slint::winit_030::winit::window::WindowAttributes,
) -> slint::winit_030::winit::window::WindowAttributes {
    use slint::winit_030::winit::window::WindowLevel;
    let a = a
        .with_active(false)
        .with_decorations(false)
        .with_window_level(WindowLevel::AlwaysOnTop);
    #[cfg(target_os = "linux")]
    let a = {
        use slint::winit_030::winit::platform::x11::{WindowAttributesExtX11, WindowType};
        a.with_x11_window_type(vec![WindowType::Notification])
    };
    #[cfg(windows)]
    let a = {
        use slint::winit_030::winit::platform::windows::WindowAttributesExtWindows;
        a.with_skip_taskbar(true)
    };
    a
}

/// macOS: the host is an accessory app, with no Dock icon and no menu bar of its own.
#[cfg(target_os = "macos")]
fn accessory_event_loop() -> slint::winit_030::EventLoopBuilder {
    use slint::winit_030::winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    let mut b =
        slint::winit_030::winit::event_loop::EventLoop::<slint::winit_030::SlintEvent>::with_user_event();
    b.with_activation_policy(ActivationPolicy::Accessory);
    b
}

/// Sizes the window to the stack and puts it in the bottom-right corner of the primary monitor
/// (the one the window is on when there is no primary), or hides it when the stack is empty.
fn place_window(host: &CardHost) {
    use slint::winit_030::WinitWindowAccessor;
    let (w, h) = (host.get_wanted_width(), host.get_wanted_height());
    if h <= 0.0 {
        let _ = host.hide();
        return;
    }
    host.window().set_size(slint::LogicalSize::new(w, h));
    let _ = host.show();
    let monitor = host
        .window()
        .with_winit_window(|ww| {
            let m = ww
                .primary_monitor()
                .or_else(|| ww.current_monitor())
                .or_else(|| ww.available_monitors().next())?;
            Some((m.position(), m.size(), m.scale_factor()))
        })
        .flatten();
    if let Some((pos, size, scale)) = monitor {
        let screen = geometry::Rect {
            x: pos.x,
            y: pos.y,
            w: size.width,
            h: size.height,
        };
        let area = geometry::work_area(screen, scale, select::Os::current());
        let px = |l: f32| (l as f64 * scale).round() as u32;
        let (x, y) = geometry::corner(area, (px(w), px(h)), px(geometry::MARGIN as f32));
        host.window()
            .set_position(slint::PhysicalPosition::new(x, y));
    }
}

/// A borderless, always-on-top window (X11 through XWayland or an X11 session, Windows, macOS).
/// Software rendering into an opaque window: square cards without the shadow.
fn run_window(backend: Backend) -> anyhow::Result<()> {
    let selector = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .with_winit_window_attributes_hook(window_attributes);
    #[cfg(target_os = "macos")]
    let selector = selector.with_winit_event_loop_builder(accessory_event_loop());
    selector.select()?;
    let host = CardHost::new()?;
    host.global::<MessageToastStore>().set_square(true);
    apply_language();
    let weak = host.as_weak();
    let relayout = move || {
        let weak = weak.clone();
        // Outside the callback that changed the stack.
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(h) = weak.upgrade() {
                place_window(&h);
            }
        });
    };
    let core = Rc::new(Core::new(|m| emit(&m), relayout));
    wire(&host, &core);
    WINDOW.with(|w| *w.borrow_mut() = Some((core, host.as_weak())));
    std::thread::spawn(|| {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Some(msg) = protocol::parse::<ToHost>(&line) else {
                continue;
            };
            let posted = slint::invoke_from_event_loop(move || {
                let quit = WINDOW.with(|w| {
                    w.borrow()
                        .as_ref()
                        .is_some_and(|(core, _)| apply(core, msg))
                });
                if quit {
                    let _ = slint::quit_event_loop();
                }
            });
            if posted.is_err() {
                return;
            }
        }
        // The app is gone.
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    });
    emit(&FromHost::Ready { backend });
    slint::run_event_loop_until_quit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_card_goes_at_the_bottom_and_a_known_one_is_replaced() {
        let stack = [(1, Kind::Message), (2, Kind::Message)];
        assert_eq!(plan(&stack, 3, 3), Plan::Append { evict: vec![] });
        assert_eq!(plan(&stack, 2, 3), Plan::Replace(1));
        assert_eq!(plan(&[], 1, 3), Plan::Append { evict: vec![] });
    }

    #[test]
    fn a_full_stack_drops_its_oldest_card_but_keeps_a_ringing_call() {
        let full = [(1, Kind::Message), (2, Kind::Meeting), (3, Kind::Message)];
        assert_eq!(plan(&full, 4, 3), Plan::Append { evict: vec![0] });
        let call_first = [(1, Kind::Call), (2, Kind::Message), (3, Kind::Message)];
        assert_eq!(plan(&call_first, 4, 3), Plan::Append { evict: vec![1] });
        let calls = [(1, Kind::Call), (2, Kind::Call), (3, Kind::Call)];
        assert_eq!(plan(&calls, 4, 3), Plan::Append { evict: vec![0] });
        // An over-full stack (never happens) is brought back to size.
        let over = [
            (1, Kind::Message),
            (2, Kind::Message),
            (3, Kind::Message),
            (4, Kind::Message),
        ];
        assert_eq!(plan(&over, 5, 3), Plan::Append { evict: vec![0, 1] });
    }

    #[test]
    fn card_buttons_map_to_actions() {
        assert_eq!(parse_act("join"), Some(Action::Join));
        assert_eq!(parse_act("snooze:5"), Some(Action::Snooze { minutes: 5 }));
        assert_eq!(parse_act("snooze:15"), Some(Action::Snooze { minutes: 15 }));
        assert_eq!(parse_act("dismiss"), Some(Action::Dismiss));
        assert_eq!(parse_act("accept"), Some(Action::Accept));
        assert_eq!(parse_act("decline"), Some(Action::Decline));
        assert_eq!(parse_act("open"), Some(Action::Open));
        assert_eq!(parse_act("settings"), Some(Action::Settings));
        assert_eq!(parse_act("snooze:0"), None);
        assert_eq!(parse_act("snooze:x"), None);
        assert_eq!(parse_act("fly"), None);
    }

    #[test]
    fn kinds_round_trip_through_the_slint_card() {
        for k in [Kind::Message, Kind::Meeting, Kind::Reminder, Kind::Call] {
            assert_eq!(kind_of(k.as_str()), k);
        }
        assert_eq!(kind_of(""), Kind::Message);
    }
}
