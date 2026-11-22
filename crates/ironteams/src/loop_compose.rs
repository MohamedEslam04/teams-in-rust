//! The composer's Loop paragraph (`ui/molecules/loop_editor.slint`): the templates it inserts, its edits,
//! and the formatted message it is sent as.
//!
//! A real Loop component is a `.loop` file in a SharePoint Embedded container kept live through
//! Fluid, and IronTeams knows no write path for that (docs/research/apps-onedrive-copilot-store.md,
//! Loop; docs/protocol.md, Loop paragraphs). So the paragraph goes out as ordinary rich text: the
//! title in bold, the text, and the structure as an HTML table or list, with ☐ / ☑ for ticks.

use crate::backend::Cmd;
use crate::{AppWindow, LoopRow, LoopStore};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tokio::sync::mpsc::UnboundedSender;

/// What a template inserts: the table's columns and the first rows.
const TABLE_COLUMNS: usize = 3;
const TABLE_ROWS: usize = 2;
const VOTING_ROWS: usize = 2;

/// One row of a paragraph's structure, as `LoopRow` holds it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    pub text: String,
    pub checked: bool,
    pub cells: Vec<String>,
    pub assignee: String,
    pub due: String,
    pub votes: i32,
}

/// A whole paragraph.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Draft {
    pub title: String,
    pub body: String,
    /// "" | "table" | "bullets" | "checklist" | "tasks" | "voting"
    pub kind: String,
    pub header: Vec<String>,
    pub rows: Vec<Row>,
}

/// The table header and rows a template starts with (`None` for an unknown kind).
pub fn template(kind: &str) -> Option<(Vec<String>, Vec<Row>)> {
    let empty = |n: usize, columns: usize| {
        (0..n)
            .map(|_| Row {
                cells: vec![String::new(); columns],
                ..Row::default()
            })
            .collect::<Vec<_>>()
    };
    Some(match kind {
        "table" => (
            (1..=TABLE_COLUMNS).map(|c| format!("Column {c}")).collect(),
            empty(TABLE_ROWS, TABLE_COLUMNS),
        ),
        "bullets" | "checklist" | "tasks" => (Vec::new(), empty(1, 0)),
        "voting" => (Vec::new(), empty(VOTING_ROWS, 0)),
        _ => return None,
    })
}

/// A row added with "+ Add row": as wide as the table.
pub fn new_row(columns: usize) -> Row {
    Row {
        cells: vec![String::new(); columns],
        ..Row::default()
    }
}

/// Ticks or unticks my vote, changing the count with it.
pub fn vote(row: &mut Row) {
    row.checked = !row.checked;
    row.votes = (row.votes + if row.checked { 1 } else { -1 }).max(0);
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn tick(checked: bool) -> &'static str {
    if checked { "☑" } else { "☐" }
}

/// A table with a header row; `None` when no row has anything in it.
fn table(header: &[&str], rows: &[Vec<String>]) -> Option<String> {
    let rows: Vec<_> = rows
        .iter()
        .filter(|r| r.iter().any(|c| !c.trim().is_empty()))
        .collect();
    if rows.is_empty() {
        return None;
    }
    let cell = |tag: &str, s: &str| format!("<{tag}>{}</{tag}>", esc(s.trim()));
    let mut out = String::from("<table><tbody><tr>");
    out.extend(header.iter().map(|h| cell("th", h)));
    out.push_str("</tr>");
    for r in rows {
        out.push_str("<tr>");
        out.extend(r.iter().map(|c| cell("td", c)));
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table>");
    Some(out)
}

/// The message a paragraph is sent as; empty when there is nothing in it.
pub fn to_html(d: &Draft) -> String {
    let mut out = String::new();
    if !d.title.trim().is_empty() {
        out.push_str(&format!("<p><b>{}</b></p>", esc(d.title.trim())));
    }
    for line in d.body.lines().filter(|l| !l.trim().is_empty()) {
        out.push_str(&format!("<p>{}</p>", esc(line.trim())));
    }
    let filled = || d.rows.iter().filter(|r| !r.text.trim().is_empty());
    match d.kind.as_str() {
        "table" => {
            let header: Vec<&str> = d.header.iter().map(String::as_str).collect();
            let rows: Vec<Vec<String>> = d.rows.iter().map(|r| r.cells.clone()).collect();
            if let Some(t) = table(&header, &rows) {
                out.push_str(&t);
            }
        }
        "bullets" => {
            let items: String = filled()
                .map(|r| format!("<li>{}</li>", esc(r.text.trim())))
                .collect();
            if !items.is_empty() {
                out.push_str(&format!("<ul>{items}</ul>"));
            }
        }
        "checklist" => {
            for r in filled() {
                out.push_str(&format!(
                    "<p>{} {}</p>",
                    tick(r.checked),
                    esc(r.text.trim())
                ));
            }
        }
        "tasks" => {
            let rows: Vec<Vec<String>> = filled()
                .map(|r| {
                    vec![
                        format!("{} {}", tick(r.checked), r.text.trim()),
                        r.assignee.clone(),
                        r.due.clone(),
                    ]
                })
                .collect();
            if let Some(t) = table(&["Task", "Assigned to", "Due date"], &rows) {
                out.push_str(&t);
            }
        }
        "voting" => {
            let rows: Vec<Vec<String>> = filled()
                .map(|r| vec![r.text.clone(), r.votes.to_string()])
                .collect();
            if let Some(t) = table(&["Option", "Votes"], &rows) {
                out.push_str(&t);
            }
        }
        _ => {}
    }
    out
}

fn to_ui(r: &Row) -> LoopRow {
    LoopRow {
        text: r.text.as_str().into(),
        checked: r.checked,
        cells: ModelRc::new(VecModel::from(
            r.cells
                .iter()
                .map(|c| SharedString::from(c.as_str()))
                .collect::<Vec<_>>(),
        )),
        assignee: r.assignee.as_str().into(),
        due: r.due.as_str().into(),
        votes: r.votes,
    }
}

fn from_ui(r: &LoopRow) -> Row {
    Row {
        text: r.text.to_string(),
        checked: r.checked,
        cells: r.cells.iter().map(|c| c.to_string()).collect(),
        assignee: r.assignee.to_string(),
        due: r.due.to_string(),
        votes: r.votes,
    }
}

/// The paragraph the store holds.
pub fn read(s: &LoopStore) -> Draft {
    Draft {
        title: s.get_title().to_string(),
        body: s.get_body().to_string(),
        kind: s.get_kind().to_string(),
        header: s.get_header().iter().map(|h| h.to_string()).collect(),
        rows: s.get_rows().iter().map(|r| from_ui(&r)).collect(),
    }
}

fn set_rows(s: &LoopStore, header: &[String], rows: &[Row]) {
    s.set_header(ModelRc::new(VecModel::from(
        header
            .iter()
            .map(|h| SharedString::from(h.as_str()))
            .collect::<Vec<_>>(),
    )));
    s.set_rows(ModelRc::new(VecModel::from(
        rows.iter().map(to_ui).collect::<Vec<_>>(),
    )));
}

/// Empties the paragraph and closes it.
fn reset(s: &LoopStore) {
    s.set_title(SharedString::new());
    s.set_body(SharedString::new());
    s.set_kind(SharedString::new());
    set_rows(s, &[], &[]);
    s.set_open(false);
}

/// Changes row `i` in place (the other rows, and the field being typed in, are left alone).
fn change(s: &LoopStore, i: i32, f: impl FnOnce(&mut Row)) {
    let rows = s.get_rows();
    let Ok(i) = usize::try_from(i) else { return };
    if let Some(r) = rows.row_data(i) {
        let mut row = from_ui(&r);
        f(&mut row);
        rows.set_row_data(i, to_ui(&row));
    }
}

/// Runs `f` on the store, while the window lives.
fn on_store(weak: &slint::Weak<AppWindow>, f: impl FnOnce(&LoopStore)) {
    if let Some(w) = weak.upgrade() {
        f(&w.global::<LoopStore>());
    }
}

/// Wires the editing callbacks, which need no backend.
pub fn install_local(window: &AppWindow) {
    let s = window.global::<LoopStore>();
    let weak = window.as_weak();
    {
        let weak = weak.clone();
        s.on_insert(move |kind| {
            on_store(&weak, |s| {
                if let Some((header, rows)) = template(&kind) {
                    s.set_kind(kind.clone());
                    set_rows(s, &header, &rows);
                }
            })
        });
    }
    {
        let weak = weak.clone();
        s.on_set_text(move |i, t| on_store(&weak, |s| change(s, i, |r| r.text = t.into())));
    }
    {
        let weak = weak.clone();
        s.on_set_cell(move |i, c, t| {
            on_store(&weak, |s| {
                change(s, i, |r| {
                    if let Some(cell) = usize::try_from(c).ok().and_then(|c| r.cells.get_mut(c)) {
                        *cell = t.into();
                    }
                })
            })
        });
    }
    {
        let weak = weak.clone();
        s.on_set_header(move |c, t| {
            on_store(&weak, |s| {
                if let Ok(c) = usize::try_from(c) {
                    s.get_header().set_row_data(c, t);
                }
            })
        });
    }
    {
        let weak = weak.clone();
        s.on_set_assignee(move |i, t| on_store(&weak, |s| change(s, i, |r| r.assignee = t.into())));
    }
    {
        let weak = weak.clone();
        s.on_set_due(move |i, t| on_store(&weak, |s| change(s, i, |r| r.due = t.into())));
    }
    {
        let weak = weak.clone();
        s.on_toggle(move |i| on_store(&weak, |s| change(s, i, |r| r.checked = !r.checked)));
    }
    {
        let weak = weak.clone();
        s.on_vote(move |i| on_store(&weak, |s| change(s, i, vote)));
    }
    {
        let weak = weak.clone();
        s.on_add_row(move || {
            on_store(&weak, |s| {
                let mut d = read(s);
                d.rows.push(new_row(d.header.len()));
                set_rows(s, &d.header, &d.rows);
            })
        });
    }
    s.on_clear(move || on_store(&weak, reset));
}
/// Wires Send: the paragraph goes to the backend as a formatted message, then the editor empties.
pub fn wire(window: &AppWindow, tx: UnboundedSender<Cmd>) {
    let weak = window.as_weak();
    window.global::<LoopStore>().on_send(move || {
        let Some(w) = weak.upgrade() else { return };
        let s = w.global::<LoopStore>();
        let html = to_html(&read(&s));
        if html.is_empty() {
            return;
        }
        let _ = tx.send(Cmd::SendLoop(html));
        reset(&s);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str) -> Row {
        Row {
            text: text.into(),
            ..Row::default()
        }
    }

    #[test]
    fn templates_seed_their_structure() {
        let (header, rows) = template("table").unwrap();
        assert_eq!(header, ["Column 1", "Column 2", "Column 3"]);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.cells.len() == 3));
        assert_eq!(template("bullets").unwrap().1.len(), 1);
        assert_eq!(template("checklist").unwrap().1.len(), 1);
        assert_eq!(template("tasks").unwrap().1.len(), 1);
        assert_eq!(template("voting").unwrap().1.len(), 2);
        assert!(template("poll").is_none());
    }

    #[test]
    fn an_empty_paragraph_sends_nothing() {
        let (header, rows) = template("table").unwrap();
        let d = Draft {
            kind: "table".into(),
            header,
            rows,
            body: "\n  \n".into(),
            ..Draft::default()
        };
        assert_eq!(to_html(&d), "");
    }

    #[test]
    fn title_and_text_are_escaped_paragraphs() {
        let d = Draft {
            title: "Plan <v2>".into(),
            body: "first & foremost\n\nsecond".into(),
            ..Draft::default()
        };
        assert_eq!(
            to_html(&d),
            "<p><b>Plan &lt;v2&gt;</b></p><p>first &amp; foremost</p><p>second</p>"
        );
    }

    #[test]
    fn a_table_keeps_its_header_and_filled_rows() {
        let d = Draft {
            kind: "table".into(),
            header: vec!["Name".into(), "Role".into()],
            rows: vec![
                Row {
                    cells: vec!["Ann".into(), "Lead".into()],
                    ..Row::default()
                },
                new_row(2),
            ],
            ..Draft::default()
        };
        assert_eq!(
            to_html(&d),
            "<table><tbody><tr><th>Name</th><th>Role</th></tr><tr><td>Ann</td><td>Lead</td></tr></tbody></table>"
        );
    }

    #[test]
    fn lists_checklists_tasks_and_votes() {
        let bullets = Draft {
            kind: "bullets".into(),
            rows: vec![row("one"), row(""), row("two")],
            ..Draft::default()
        };
        assert_eq!(to_html(&bullets), "<ul><li>one</li><li>two</li></ul>");

        let mut done = row("ship");
        done.checked = true;
        let checklist = Draft {
            kind: "checklist".into(),
            rows: vec![row("test"), done.clone()],
            ..Draft::default()
        };
        assert_eq!(to_html(&checklist), "<p>☐ test</p><p>☑ ship</p>");

        let mut task = done;
        task.assignee = "Sam".into();
        task.due = "Friday".into();
        let tasks = Draft {
            kind: "tasks".into(),
            rows: vec![task],
            ..Draft::default()
        };
        assert_eq!(
            to_html(&tasks),
            "<table><tbody><tr><th>Task</th><th>Assigned to</th><th>Due date</th></tr><tr><td>☑ ship</td><td>Sam</td><td>Friday</td></tr></tbody></table>"
        );

        let mut a = row("Tea");
        vote(&mut a);
        let mut b = row("Coffee");
        vote(&mut b);
        vote(&mut b);
        let voting = Draft {
            kind: "voting".into(),
            rows: vec![a, b],
            ..Draft::default()
        };
        assert_eq!(
            to_html(&voting),
            "<table><tbody><tr><th>Option</th><th>Votes</th></tr><tr><td>Tea</td><td>1</td></tr><tr><td>Coffee</td><td>0</td></tr></tbody></table>"
        );
    }
}
