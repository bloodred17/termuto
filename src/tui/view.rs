//! Sorting and filtering for the list screens.
//!
//! Every list keeps its rows in whatever order its source gave them and layers
//! a view on top: `n` orders by name, `d` by date, `f` narrows the rows to the
//! ones whose name matches what is typed, and `t` steps through the types the
//! list holds. The two filters narrow together. The view never touches the rows
//! themselves — it produces the positions to draw, in the order to draw them —
//! so a filtered listing still opens the title it appears to be pointing at.

use std::cmp::Ordering;

/// What a row gives the view: the text `n` orders and `f` matches against, the
/// date `d` orders by, and the type `t` steps through.
pub(crate) struct RowKeys {
    pub(crate) name: String,
    /// Dates arrive already formatted `YYYY-MM-DD`, which orders correctly as
    /// text, so they are compared as they are drawn. A row that knows only its
    /// broadcast season keeps that label and sorts after the dated rows — the
    /// best a string comparison can do, and better than dropping it.
    pub(crate) date: Option<String>,
    /// `TV`, `Movie`, `OVA` — whatever the source called it. `None` on a list
    /// with no such column, which is what makes `t` a no-op there.
    pub(crate) kind: Option<String>,
}

impl RowKeys {
    pub(crate) fn new(name: impl Into<String>, date: Option<String>) -> Self {
        Self {
            name: name.into(),
            date,
            kind: None,
        }
    }

    pub(crate) fn with_kind(mut self, kind: Option<String>) -> Self {
        self.kind = kind;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SortKey {
    /// The order the source gave. Meaningful on its own — a season listing is
    /// ranked, and "latest releases" is already by date — so it is what a list
    /// starts and returns to.
    #[default]
    Source,
    Name,
    Date,
}

/// How one list is sorted and filtered. Screens that show the same rows share
/// one; unrelated lists keep their own, so filtering a listing and stepping
/// into it does not filter the episodes too.
#[derive(Clone, Debug, Default)]
pub(crate) struct ListView {
    key: SortKey,
    /// Whether the key runs against its natural direction: names read A–Z and
    /// dates newest first, since that is what each is usually wanted in.
    reversed: bool,
    filter: String,
    /// Whether typing edits the filter rather than driving the list.
    editing: bool,
    /// The one type of row being shown, or every type.
    kind: Option<String>,
}

impl ListView {
    /// Applies a sort key. Pressing the same key again reverses it, which is
    /// the only way to ask for oldest-first or Z–A.
    pub(crate) fn sort_by(&mut self, key: SortKey) {
        if self.key == key {
            self.reversed = !self.reversed;
        } else {
            self.key = key;
            self.reversed = false;
        }
    }

    pub(crate) fn editing(&self) -> bool {
        self.editing
    }

    pub(crate) fn begin_filter(&mut self) {
        self.editing = true;
    }

    /// Keeps the filter and hands the keyboard back to the list.
    pub(crate) fn accept_filter(&mut self) {
        self.editing = false;
    }

    pub(crate) fn cancel_filter(&mut self) {
        self.editing = false;
        self.filter.clear();
    }

    pub(crate) fn push_filter(&mut self, character: char) {
        self.filter.push(character);
    }

    pub(crate) fn pop_filter(&mut self) {
        self.filter.pop();
    }

    /// Steps to the next type the list holds, and past the last one back to
    /// showing every type. Cycling beats prompting for one: the column has a
    /// handful of values, and they are already on screen to be read off.
    pub(crate) fn cycle_kind(&mut self, rows: &[RowKeys]) {
        let kinds = self.kinds(rows);
        self.kind = match &self.kind {
            None => kinds.first().cloned(),
            // A type the list no longer holds — the name filter moved under it —
            // gives everything back rather than stranding an empty screen.
            Some(current) => match kinds.iter().position(|kind| kind == current) {
                Some(position) => kinds.get(position + 1).cloned(),
                None => None,
            },
        };
    }

    /// The types `t` steps through: the ones the name filter has left, so the
    /// key never lands on a type with nothing under it.
    fn kinds(&self, rows: &[RowKeys]) -> Vec<String> {
        let mut kinds: Vec<String> = self
            .named(rows)
            .filter_map(|index| rows[index].kind.clone())
            .collect();
        kinds.sort();
        kinds.dedup();
        kinds
    }

    /// The rows the typed filter leaves, before the type filter narrows them.
    fn named<'a>(&'a self, rows: &'a [RowKeys]) -> impl Iterator<Item = usize> + 'a {
        let needle = self.filter.trim().to_lowercase();
        (0..rows.len()).filter(move |index| {
            needle.is_empty() || rows[*index].name.to_lowercase().contains(&needle)
        })
    }

    fn shows(&self, row: &RowKeys) -> bool {
        match &self.kind {
            Some(kind) => row.kind.as_deref().is_some_and(|other| other == kind),
            None => true,
        }
    }

    /// The rows to draw, as positions into `rows`, in the order to draw them.
    pub(crate) fn order(&self, rows: &[RowKeys]) -> Vec<usize> {
        // Names are folded once rather than per comparison: the filter, the
        // name sort, and the date tiebreak all want the same lowercased text.
        let names: Vec<String> = rows.iter().map(|row| row.name.to_lowercase()).collect();

        let mut order: Vec<usize> = self
            .named(rows)
            .filter(|index| self.shows(&rows[*index]))
            .collect();

        match self.key {
            SortKey::Source => {}
            SortKey::Name => order.sort_by(|left, right| {
                let ordering = names[*left].cmp(&names[*right]);
                if self.reversed {
                    ordering.reverse()
                } else {
                    ordering
                }
            }),
            // Undated rows sort last in both directions: they are the rows the
            // source knows least about, and burying them is the point of
            // sorting by date at all.
            SortKey::Date => order.sort_by(|left, right| {
                let newest_first = !self.reversed;
                match (&rows[*left].date, &rows[*right].date) {
                    (Some(earlier), Some(later)) if newest_first => later.cmp(earlier),
                    (Some(earlier), Some(later)) => earlier.cmp(later),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                }
                .then_with(|| names[*left].cmp(&names[*right]))
            }),
        }
        order
    }

    /// The line under the list's bottom border, saying how it is ordered and
    /// what the filter is holding back. `None` while the list is untouched, so
    /// an ordinary list draws exactly as it did before.
    pub(crate) fn status(&self, visible: usize, total: usize) -> Option<String> {
        let mut parts = Vec::new();
        if self.editing {
            // The bar is where the typing lands, so it carries the cursor.
            parts.push(format!("filter: {}▏", self.filter));
        } else if !self.filter.trim().is_empty() {
            parts.push(format!("filter: {}", self.filter));
        }
        if let Some(kind) = &self.kind {
            parts.push(format!("type: {kind}"));
        }
        if let Some(label) = self.sort_label() {
            parts.push(format!("sort: {label}"));
        }
        if visible != total {
            parts.push(format!("{visible}/{total}"));
        }
        (!parts.is_empty()).then(|| format!(" {} ", parts.join(" · ")))
    }

    /// Spelled out rather than arrowed: an arrow beside "name" says nothing
    /// about which end the As are at.
    fn sort_label(&self) -> Option<&'static str> {
        match (self.key, self.reversed) {
            (SortKey::Source, _) => None,
            (SortKey::Name, false) => Some("name a–z"),
            (SortKey::Name, true) => Some("name z–a"),
            (SortKey::Date, false) => Some("date newest"),
            (SortKey::Date, true) => Some("date oldest"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ListView, RowKeys, SortKey};

    fn rows() -> Vec<RowKeys> {
        vec![
            RowKeys::new("Cowboy Bebop", Some("1998-04-03".into())).with_kind(Some("TV".into())),
            RowKeys::new("attack on titan", Some("2013-04-07".into())).with_kind(Some("TV".into())),
            RowKeys::new("Bocchi the Rock!", None).with_kind(Some("Movie".into())),
        ]
    }

    fn names(view: &ListView, rows: &[RowKeys]) -> Vec<String> {
        view.order(rows)
            .into_iter()
            .map(|index| rows[index].name.clone())
            .collect()
    }

    #[test]
    fn an_untouched_view_keeps_the_source_order() {
        let view = ListView::default();
        assert_eq!(
            names(&view, &rows()),
            ["Cowboy Bebop", "attack on titan", "Bocchi the Rock!"]
        );
        assert_eq!(view.status(3, 3), None);
    }

    /// Case has to fold, or a lowercase title lands after every capitalised one.
    #[test]
    fn the_name_sort_ignores_case_and_reverses_on_a_second_press() {
        let mut view = ListView::default();
        view.sort_by(SortKey::Name);
        assert_eq!(
            names(&view, &rows()),
            ["attack on titan", "Bocchi the Rock!", "Cowboy Bebop"]
        );

        view.sort_by(SortKey::Name);
        assert_eq!(
            names(&view, &rows()),
            ["Cowboy Bebop", "Bocchi the Rock!", "attack on titan"]
        );
    }

    #[test]
    fn the_date_sort_runs_newest_first_and_keeps_undated_rows_last() {
        let mut view = ListView::default();
        view.sort_by(SortKey::Date);
        assert_eq!(
            names(&view, &rows()),
            ["attack on titan", "Cowboy Bebop", "Bocchi the Rock!"]
        );

        // Reversed, the dated rows swap and the undated one stays put.
        view.sort_by(SortKey::Date);
        assert_eq!(
            names(&view, &rows()),
            ["Cowboy Bebop", "attack on titan", "Bocchi the Rock!"]
        );
    }

    #[test]
    fn the_filter_matches_anywhere_in_a_name_whatever_its_case() {
        let mut view = ListView::default();
        for character in "TITAN".chars() {
            view.push_filter(character);
        }
        assert_eq!(names(&view, &rows()), ["attack on titan"]);

        view.pop_filter();
        assert_eq!(names(&view, &rows()), ["attack on titan"]);
    }

    #[test]
    fn cancelling_a_filter_puts_every_row_back() {
        let mut view = ListView::default();
        view.begin_filter();
        view.push_filter('z');
        assert!(names(&view, &rows()).is_empty());

        view.cancel_filter();
        assert_eq!(names(&view, &rows()).len(), 3);
        assert!(!view.editing());
    }

    #[test]
    fn t_steps_through_the_types_the_list_holds_and_back_to_all_of_them() {
        let rows = rows();
        let mut view = ListView::default();

        view.cycle_kind(&rows);
        assert_eq!(names(&view, &rows), ["Bocchi the Rock!"]);
        view.cycle_kind(&rows);
        assert_eq!(names(&view, &rows), ["Cowboy Bebop", "attack on titan"]);
        // Past the last type, everything is back.
        view.cycle_kind(&rows);
        assert_eq!(names(&view, &rows).len(), 3);
    }

    /// The two filters narrow together, and the types on offer are only the
    /// ones the typed filter has left — `t` never lands on an empty screen.
    #[test]
    fn the_type_cycle_only_offers_types_the_typed_filter_left() {
        let rows = rows();
        let mut view = ListView::default();
        for character in "bo".chars() {
            view.push_filter(character);
        }
        assert_eq!(names(&view, &rows), ["Cowboy Bebop", "Bocchi the Rock!"]);

        view.cycle_kind(&rows);
        assert_eq!(names(&view, &rows), ["Bocchi the Rock!"]);
        view.cycle_kind(&rows);
        assert_eq!(names(&view, &rows), ["Cowboy Bebop"]);
    }

    /// A list with no type column has nothing to step through, which is what
    /// makes the key harmless on the episode pickers.
    #[test]
    fn t_does_nothing_to_a_list_without_types() {
        let rows = vec![RowKeys::new("Episode 1", None)];
        let mut view = ListView::default();
        view.cycle_kind(&rows);
        assert_eq!(names(&view, &rows), ["Episode 1"]);
        assert_eq!(view.status(1, 1), None);
    }

    #[test]
    fn the_status_line_reports_the_filter_the_sort_and_what_was_held_back() {
        let mut view = ListView::default();
        view.sort_by(SortKey::Date);
        assert_eq!(view.status(3, 3).as_deref(), Some(" sort: date newest "));

        view.begin_filter();
        view.push_filter('b');
        assert_eq!(
            view.status(1, 3).as_deref(),
            Some(" filter: b▏ · sort: date newest · 1/3 ")
        );

        // The cursor goes once the filter is accepted; the filter itself stays.
        view.accept_filter();
        assert_eq!(
            view.status(1, 3).as_deref(),
            Some(" filter: b · sort: date newest · 1/3 ")
        );
    }
}
