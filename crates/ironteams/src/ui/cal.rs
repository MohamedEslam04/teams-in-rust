//! UI-thread side of the calendar: turns the backend's view data into the `CalStore` and
//! `CalForm` globals, and turns what the user does there into `CalCmd`s.

use super::{UiHandle, model, rgb};
use crate::backend::{
    CalCmd, CalData, Cmd, DetailData, FormFields, FormOpen, Panels, PeekData, PermRow, RoomsPush,
    SchedPush, SchedReq, SlotReq,
};
use crate::{
    AppWindow, CalAllDay, CalAttachmentRow, CalAttendeeRow, CalBlock, CalCalendarRow, CalDay,
    CalDetail, CalFilterOpt, CalForm, CalMiniCell, CalMonthCell, CalMonthItem, CalPeek, CalPerm,
    CalProposalRow, CalRoom, CalRoomList, CalSavedView, CalSchedRow, CalSettingsData, CalStore,
    CalSuggest,
};
use slint::{ComponentHandle, Model, SharedString};
use tokio::sync::mpsc::UnboundedSender;

mod push;
use push::*;
mod form;
pub use form::*;
mod wire;
pub use wire::*;

fn int(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn strings(v: Vec<String>) -> slint::ModelRc<SharedString> {
    model(v.into_iter().map(SharedString::from).collect())
}

#[cfg(test)]
mod tests;
