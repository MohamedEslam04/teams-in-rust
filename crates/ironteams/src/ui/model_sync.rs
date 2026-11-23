//! Updates a list's model in place instead of replacing it.
//!
//! A new model makes Slint drop every row of the list and build them again: a `ListView` also
//! forgets the heights it measured and where it is, so the rows on screen jump (and play their
//! entrance animations again, and a row under the pointer forgets the pointer). The engine draws
//! a conversation again for every avatar, presence change or reaction, so a list fed a new model
//! each time moves under the user. `patch` keeps the model: the rows both lists share stay (and
//! only those whose fingerprint changed are written), older rows are inserted above and newer
//! ones added below, so the list sees where rows came and went.

use slint::{Model, ModelRc, VecModel};
use std::rc::Rc;

/// What a row was made from: its id and a fingerprint of everything drawn.
pub(crate) type Print = (String, u64);

/// A list's model, kept between updates, and the print of each of its rows.
pub(crate) type Synced<T> = (Rc<VecModel<T>>, Vec<Print>);

/// Whether a list shows `m` (and not a model set elsewhere, such as an empty one when another
/// conversation opened).
pub(crate) fn is_shown<T: Clone + 'static>(shown: &ModelRc<T>, m: &Rc<VecModel<T>>) -> bool {
    *shown == ModelRc::from(m.clone())
}

/// Makes `m` show `new` (rows given with their [`Print`]), writing as little as it can; `prints`
/// are the prints of the rows `m` holds, kept by the caller between calls. Returns how many rows
/// were written.
pub(crate) fn patch<T: Clone + 'static, D>(
    m: &VecModel<T>,
    prints: &mut Vec<Print>,
    new: Vec<(Print, D)>,
    make: impl Fn(D) -> T,
) -> usize {
    let old = m.row_count();
    // The model was emptied or filled elsewhere: nothing is known about its rows.
    if prints.len() != old {
        *prints = vec![(String::new(), u64::MAX); old];
    }
    let id = |i: usize, prints: &[Print]| prints[i].0.clone();
    // Where the shared run starts in the old list (a) and in the new one (b).
    let (a, b) = match new
        .first()
        .and_then(|(f, _)| (0..old).find(|&i| !f.0.is_empty() && id(i, prints) == f.0))
    {
        Some(a) => (a, 0),
        None => (
            0,
            (0..new.len())
                .find(|&j| old > 0 && !prints[0].0.is_empty() && prints[0].0 == new[j].0.0)
                .unwrap_or(new.len()),
        ),
    };
    let run = (0..(old - a).min(new.len().saturating_sub(b)))
        .take_while(|&k| prints[a + k].0 == new[b + k].0.0)
        .count();
    if old == 0 {
        let n = new.len();
        let (p, rows): (Vec<Print>, Vec<T>) = new.into_iter().map(|(p, d)| (p, make(d))).unzip();
        m.set_vec(rows);
        *prints = p;
        return n;
    }
    let mut written = 0;
    if run * 2 < old.min(new.len()).max(1) {
        // Little in one run: the rows were reordered (a chat moved to the top of the chat list)
        // or it is another list altogether. Each row is written where it now is, so the rows
        // stay and only those that show something else are written.
        let n = new.len();
        while m.row_count() > n {
            m.remove(m.row_count() - 1);
            prints.pop();
        }
        for (j, (p, d)) in new.into_iter().enumerate() {
            if j >= old {
                m.push(make(d));
                prints.push(p);
                written += 1;
            } else if prints[j] != p {
                m.set_row_data(j, make(d));
                prints[j] = p;
                written += 1;
            }
        }
        return written;
    }
    for _ in 0..a {
        m.remove(0);
        prints.remove(0);
    }
    for _ in 0..old - a - run {
        m.remove(run);
        prints.remove(run);
    }
    for (j, (p, d)) in new.into_iter().enumerate() {
        if j < b {
            m.insert(j, make(d));
            prints.insert(j, p);
            written += 1;
        } else if j < b + run {
            if prints[j] != p {
                m.set_row_data(j, make(d));
                prints[j] = p;
                written += 1;
            }
        } else {
            m.push(make(d));
            prints.push(p);
            written += 1;
        }
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    /// Rows as shared strings: a row that was not written is the very same string afterwards.
    fn rows(spec: &[(&str, u64)]) -> Vec<(Print, Rc<str>)> {
        spec.iter()
            .map(|(id, v)| ((id.to_string(), *v), Rc::from(format!("{id}:{v}"))))
            .collect()
    }

    fn shown(m: &VecModel<Rc<str>>) -> Vec<String> {
        m.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rows_are_kept_inserted_and_written_only_when_they_changed() {
        let m = VecModel::<Rc<str>>::default();
        let mut prints = vec![];
        assert_eq!(
            patch(&m, &mut prints, rows(&[("a", 1), ("b", 1)]), |s| s),
            2
        );
        let a = m.row_data(0).unwrap();
        // Nothing changed: nothing is written.
        assert_eq!(
            patch(&m, &mut prints, rows(&[("a", 1), ("b", 1)]), |s| s),
            0
        );
        // b changed, c is new below, o is older above.
        let n = patch(
            &m,
            &mut prints,
            rows(&[("o", 1), ("a", 1), ("b", 2), ("c", 1)]),
            |s| s,
        );
        assert_eq!(n, 3);
        assert_eq!(shown(&m), ["o:1", "a:1", "b:2", "c:1"]);
        assert!(
            Rc::ptr_eq(&a, &m.row_data(1).unwrap()),
            "a was kept, not written"
        );
        // The window slid: o left above, d came below.
        patch(
            &m,
            &mut prints,
            rows(&[("a", 1), ("b", 2), ("c", 1), ("d", 1)]),
            |s| s,
        );
        assert_eq!(shown(&m), ["a:1", "b:2", "c:1", "d:1"]);
        assert!(Rc::ptr_eq(&a, &m.row_data(0).unwrap()));
        // Rows removed in the middle: the rows after them are written again.
        patch(&m, &mut prints, rows(&[("a", 1), ("d", 1)]), |s| s);
        assert_eq!(shown(&m), ["a:1", "d:1"]);
        // Another list altogether.
        assert_eq!(patch(&m, &mut prints, rows(&[("x", 1)]), |s| s), 1);
        assert_eq!(shown(&m), ["x:1"]);
        // The model was changed elsewhere: its rows are written again.
        m.push(Rc::from("y:1"));
        patch(&m, &mut prints, rows(&[("x", 1), ("y", 1)]), |s| s);
        assert_eq!(shown(&m), ["x:1", "y:1"]);
    }
}
