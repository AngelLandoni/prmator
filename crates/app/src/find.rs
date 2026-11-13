//! ⌘F find bar for the code on the current page (Files changed, or the story commit being
//! read): every match is highlighted in place, ↵ / ⇧↵ step through them. It never leaves
//! the page, so in story mode it only searches the current commit.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{prelude::*, Bounds, Context, Pixels, Entity, Focusable, ScrollStrategy, Subscription, Window};

use crate::app::Prmator;
use crate::model::{PrPage, PrView, Row};
use crate::text_input::{Changed, TextInput};

/// One occurrence: a byte range of a row's display text (tabs expanded) on one side
/// (0 = left/unified, 1 = right).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindMatch {
    pub row: usize,
    pub side: usize,
    pub range: Range<usize>,
}

pub struct FindBar {
    pub input: Entity<TextInput>,
    pub query: String,
    pub matches: Vec<FindMatch>,
    pub current: Option<usize>,
    /// The rows the matches were found in; searched again when they change (expanding
    /// context, switching unified/split or story commit…).
    rows: Vec<Row>,
    /// Bounds of the code list, recorded while painting it; the bar floats over it.
    pub anchor: Rc<Cell<Option<Bounds<Pixels>>>>,
    _subscriptions: Vec<Subscription>,
}

impl FindBar {
    /// Matches on a row side, with whether each is the current one.
    pub fn ranges_on(&self, row: usize, side: usize, row_kind: Row) -> Vec<(Range<usize>, bool)> {
        let start = self.matches.partition_point(|m| m.row < row);
        self.matches[start..]
            .iter()
            .enumerate()
            .take_while(|(_, m)| m.row == row)
            // Context rows show the same line on both sides; it's searched once.
            .filter(|(_, m)| m.side == side || matches!(row_kind, Row::Context(..)))
            .map(|(i, m)| (m.range.clone(), self.current == Some(start + i)))
            .collect()
    }

    pub fn status(&self) -> String {
        match (self.query.is_empty(), self.matches.len(), self.current) {
            (true, ..) => String::new(),
            (false, 0, _) => "No results".into(),
            (false, n, Some(c)) => format!("{} of {n}", c + 1),
            (false, n, None) => format!("{n} results"),
        }
    }
}

/// Non-overlapping occurrences of `needle` in `text`. Case-insensitive unless the needle
/// has an uppercase letter ("smart case"). ASCII case folding keeps byte offsets intact.
pub fn find_all(text: &str, needle: &str) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return vec![];
    }
    let fold = !needle.chars().any(|c| c.is_uppercase());
    let (hay, needle) = if fold { (text.to_ascii_lowercase(), needle.to_ascii_lowercase()) } else { (text.to_string(), needle.to_string()) };
    let mut out = vec![];
    let mut from = 0;
    while let Some(pos) = hay[from..].find(&needle) {
        let start = from + pos;
        out.push(start..start + needle.len());
        from = start + needle.len();
    }
    out
}

/// Every match in the page's rows, in row order.
fn search(pv: &PrView, query: &str) -> Vec<FindMatch> {
    let mut out = vec![];
    for (row, kind) in pv.rows.iter().enumerate() {
        let sides: &[usize] = match kind {
            Row::Split(..) => &[0, 1],
            Row::Line(..) | Row::Context(..) => &[0],
            _ => &[],
        };
        for &side in sides {
            let Some(text) = pv.row_text(row, side) else { continue };
            out.extend(find_all(&text, query).into_iter().map(|range| FindMatch { row, side, range }));
        }
    }
    out
}

impl Prmator {
    /// ⌘F: the find bar on the Files and Story pages; elsewhere the palette's text search.
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_code = self.browser.is_none()
            && self.pr.as_ref().is_some_and(|pv| matches!(pv.page, PrPage::Files | PrPage::Story) && !pv.rows.is_empty());
        if !on_code {
            self.open_palette(crate::ui::palette::PaletteMode::Text, window, cx);
            return;
        }
        if let Some(f) = &self.find {
            // Already open: focus it with the query selected, like a browser.
            let input = f.input.clone();
            window.focus(&input.focus_handle(cx));
            input.update(cx, |i, cx| i.select_all_text(cx));
            return;
        }
        // Start from the selected text, if any.
        let seed = self
            .pr
            .as_ref()
            .and_then(|pv| {
                let sel = pv.token_sel()?;
                (sel.anchor.row == sel.head.row).then(|| sel.text(|r, s| pv.row_text(r, s)))
            })
            .unwrap_or_default();
        let input = cx.new(|cx| TextInput::new("Find in this page", cx).submit_empty());
        let subscriptions = vec![cx.subscribe(&input, |this, _, Changed(text), cx| this.set_find_query(text.clone(), cx))];
        window.focus(&input.focus_handle(cx));
        self.find = Some(FindBar { input: input.clone(), query: String::new(), matches: vec![], current: None, rows: vec![], anchor: Rc::default(), _subscriptions: subscriptions });
        if !seed.is_empty() {
            input.update(cx, |i, cx| {
                i.set_text(seed, cx);
                i.select_all_text(cx);
            });
        }
        cx.notify();
    }

    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        window.focus(&self.diff_focus);
        cx.notify();
    }

    fn set_find_query(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(f) = &mut self.find else { return };
        if f.query == query {
            return;
        }
        f.query = query;
        f.rows.clear(); // search again
        self.refresh_find();
        // Go to the first match from the top of the view.
        let top = self.pr.as_ref().map_or(0, |pv| {
            let s = pv.scroll.0.borrow();
            s.deferred_scroll_to_item.as_ref().map(|d| d.item_index).unwrap_or_else(|| s.base_handle.logical_scroll_top().0)
        });
        if let Some(f) = &mut self.find {
            f.current = f.matches.iter().position(|m| m.row >= top).or((!f.matches.is_empty()).then_some(0));
        }
        self.reveal_find_match();
        cx.notify();
    }

    /// Searches again when the page's rows changed since the last search.
    pub fn refresh_find(&mut self) {
        let (Some(f), Some(pv)) = (&mut self.find, &self.pr) else { return };
        if !matches!(pv.page, PrPage::Files | PrPage::Story) {
            return;
        }
        if f.rows == pv.rows {
            return;
        }
        let current = f.current.and_then(|c| f.matches.get(c)).map(|m| (m.row, m.side, m.range.start));
        f.matches = search(pv, &f.query);
        f.rows = pv.rows.clone();
        // Stay on the same match if it's still there, else the next one.
        f.current = current.and_then(|cur| {
            f.matches.iter().position(|m| (m.row, m.side, m.range.start) >= cur).or((!f.matches.is_empty()).then_some(0))
        });
    }

    /// ↵ / ⇧↵: the next or previous match, wrapping around.
    pub fn find_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.refresh_find();
        let Some(f) = &mut self.find else { return };
        let n = f.matches.len() as isize;
        if n == 0 {
            return;
        }
        f.current = Some(match f.current {
            Some(c) => (c as isize + delta).rem_euclid(n) as usize,
            None if delta < 0 => (n - 1) as usize,
            None => 0,
        });
        self.reveal_find_match();
        cx.notify();
    }

    fn reveal_find_match(&mut self) {
        let row = self.find.as_ref().and_then(|f| f.matches.get(f.current?)).map(|m| m.row);
        if let (Some(row), Some(pv)) = (row, &mut self.pr) {
            pv.scroll.scroll_to_item(row, ScrollStrategy::Center);
            pv.current_file = pv.rows[row].file();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::find_all;

    #[test]
    fn smart_case() {
        assert_eq!(find_all("Foo foo FOO", "foo"), vec![0..3, 4..7, 8..11]);
        assert_eq!(find_all("Foo foo FOO", "Foo"), vec![0..3]);
        assert_eq!(find_all("aaaa", "aa"), vec![0..2, 2..4]);
        assert_eq!(find_all("été x", "x"), vec![6..7]);
        assert!(find_all("abc", "").is_empty());
    }
}
