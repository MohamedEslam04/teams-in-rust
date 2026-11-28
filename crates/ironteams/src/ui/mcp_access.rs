//! UI-thread side of the per-conversation AI access: fills the `AiAccessStore` global from the
//! backend's [`AiAccessData`] and sends its commands to the engine.
//!
//! Without the `mcp` cargo feature the store's `available` stays false (its default), so every
//! control is hidden: a build without the MCP server has nothing to offer.

use super::{UiHandle, model};
use crate::backend::{Cmd, McpAccessCmd};
use crate::mcp_access_data::{AiAccessData, AiAccessRow};
use crate::{AiAccessRow as SlintAiAccessRow, AiAccessStore, AppWindow};
use slint::{ComponentHandle, SharedString};
use tokio::sync::mpsc::UnboundedSender;

fn row(r: AiAccessRow) -> SlintAiAccessRow {
    SlintAiAccessRow {
        id: r.id.into(),
        title: r.title.into(),
        level: r.level.into(),
        last_read: r.last_read.into(),
    }
}

impl UiHandle {
    /// Shows the AI access state: the switches, the controls' target and the enabled
    /// conversations of the overview.
    pub fn ai_access(&self, d: AiAccessData) {
        self.run(move |w: AppWindow| {
            let s = w.global::<AiAccessStore>();
            s.set_master_paused(d.master_paused);
            s.set_account_paused(d.account_paused);
            s.set_calendar_people(d.calendar_people);
            s.set_target_id(d.target_id.into());
            s.set_target_level(d.target_level.into());
            s.set_meeting_id(d.meeting_id.into());
            s.set_meeting_level(d.meeting_level.into());
            s.set_rows(model(d.rows.into_iter().map(row).collect()));
            s.set_connect_text(d.connect_text.into());
            s.set_account_label(d.account_label.into());
            s.set_bulk_team(d.bulk_team.into());
            s.set_bulk_names(model(
                d.bulk_names.into_iter().map(SharedString::from).collect(),
            ));
            s.set_bulk_count(i32::try_from(d.bulk_count).unwrap_or(i32::MAX));
            s.set_can_undo_bulk(d.can_undo_bulk);
            super::popout::sync_ai_access(&w);
        });
    }

    /// Opens the first-time explainer (the first time something is set above Blocked).
    pub fn ai_access_explainer(&self) {
        self.run(|w: AppWindow| w.global::<AiAccessStore>().set_explainer_open(true));
    }

    /// Opens the confirmation of the Team menu's "Let AI read all channels…".
    pub fn ai_access_bulk(&self) {
        self.run(|w: AppWindow| w.global::<AiAccessStore>().set_bulk_open(true));
    }
}

/// Connects the store's `command` callback to the engine's command channel, and turns the
/// controls on when this build has the MCP server.
pub fn install(w: &AppWindow, tx: UnboundedSender<Cmd>) {
    let store = w.global::<AiAccessStore>();
    #[cfg(feature = "mcp")]
    store.set_available(true);
    store.on_command(move |name, arg| match McpAccessCmd::parse(&name, &arg) {
        Some(c) => {
            let _ = tx.send(Cmd::McpAccess(c));
        }
        None => tracing::warn!(%name, "unknown AI access command"),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    fn window() -> AppWindow {
        thread_local! { static INIT: std::sync::Once = const { std::sync::Once::new() }; }
        INIT.with(|once| once.call_once(i_slint_backend_testing::init_no_event_loop));
        AppWindow::new().unwrap()
    }

    fn handle(w: &AppWindow) -> UiHandle {
        UiHandle::new(w.as_weak())
    }

    #[test]
    fn a_row_keeps_every_field() {
        let r = row(AiAccessRow {
            id: "19:x".into(),
            title: "Zoe".into(),
            level: "read_draft".into(),
            last_read: "last read by AI: today 14:30".into(),
        });
        assert_eq!(
            (
                r.id.as_str(),
                r.title.as_str(),
                r.level.as_str(),
                r.last_read.as_str()
            ),
            ("19:x", "Zoe", "read_draft", "last read by AI: today 14:30")
        );
    }

    #[test]
    fn the_data_fills_the_store() {
        let w = window();
        handle(&w).ai_access(AiAccessData {
            master_paused: true,
            account_paused: true,
            calendar_people: true,
            target_id: "19:x".into(),
            target_level: "read".into(),
            meeting_id: "19:meeting_x@thread.v2".into(),
            meeting_level: "read_draft".into(),
            rows: vec![AiAccessRow {
                id: "19:x".into(),
                title: "Zoe".into(),
                level: "read".into(),
                last_read: String::new(),
            }],
            connect_text: "claude mcp add".into(),
            account_label: "work".into(),
            bulk_team: "Zebra".into(),
            bulk_names: vec!["General".into(), "Roadmap".into()],
            bulk_count: 2,
            can_undo_bulk: true,
        });
        let s = w.global::<AiAccessStore>();
        assert!(s.get_master_paused() && s.get_account_paused() && s.get_calendar_people());
        assert_eq!(s.get_bulk_team().as_str(), "Zebra");
        let names: Vec<String> = s.get_bulk_names().iter().map(|n| n.to_string()).collect();
        assert_eq!(names, ["General", "Roadmap"]);
        assert_eq!(s.get_bulk_count(), 2);
        assert!(s.get_can_undo_bulk());
        assert_eq!(s.get_target_id().as_str(), "19:x");
        assert_eq!(s.get_target_level().as_str(), "read");
        assert_eq!(s.get_meeting_id().as_str(), "19:meeting_x@thread.v2");
        assert_eq!(s.get_meeting_level().as_str(), "read_draft");
        assert_eq!(s.get_rows().row_count(), 1);
        assert_eq!(s.get_rows().row_data(0).unwrap().title.as_str(), "Zoe");
        assert_eq!(s.get_connect_text().as_str(), "claude mcp add");
        assert_eq!(s.get_account_label().as_str(), "work");
        // Empty data clears what was there.
        handle(&w).ai_access(AiAccessData::default());
        assert_eq!(s.get_rows().row_count(), 0);
        assert!(!s.get_master_paused());
        assert_eq!(s.get_bulk_names().row_count(), 0);
        assert_eq!(s.get_bulk_count(), 0);
        assert!(!s.get_can_undo_bulk());
    }

    #[test]
    fn the_explainer_opens_from_rust() {
        let w = window();
        assert!(!w.global::<AiAccessStore>().get_explainer_open());
        handle(&w).ai_access_explainer();
        assert!(w.global::<AiAccessStore>().get_explainer_open());
    }

    #[test]
    fn the_bulk_confirmation_opens_from_rust() {
        let w = window();
        assert!(!w.global::<AiAccessStore>().get_bulk_open());
        handle(&w).ai_access_bulk();
        assert!(w.global::<AiAccessStore>().get_bulk_open());
    }

    #[test]
    fn commands_reach_the_engine_and_unknown_ones_do_not() {
        let w = window();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        install(&w, tx);
        let s = w.global::<AiAccessStore>();
        s.invoke_command("block-all".into(), "".into());
        s.invoke_command("set-level".into(), "19:x\tread".into());
        s.invoke_command("team-channels".into(), "19:t@thread.tacv2".into());
        s.invoke_command("nonsense".into(), "".into());
        assert!(matches!(
            rx.try_recv(),
            Ok(Cmd::McpAccess(McpAccessCmd::BlockAll))
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(Cmd::McpAccess(McpAccessCmd::SetLevel(_, Some(_))))
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(Cmd::McpAccess(McpAccessCmd::TeamChannels(_)))
        ));
        assert!(rx.try_recv().is_err(), "the unknown command sent nothing");
    }

    #[test]
    fn the_controls_are_available_only_with_the_mcp_feature() {
        let w = window();
        assert!(!w.global::<AiAccessStore>().get_available());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        install(&w, tx);
        assert_eq!(
            w.global::<AiAccessStore>().get_available(),
            cfg!(feature = "mcp")
        );
    }
}
