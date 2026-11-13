//! Token-wise selection in the code views (diff and full file): a click selects the whole
//! token under the mouse, ←/→ move to the previous/next token (wrapping across lines), ↑/↓
//! to the token at the same column on the line above/below, and ⇧ extends. There is never a
//! caret in the middle of a word.
//!
//! Positions are byte offsets in a row's display text (tabs expanded, see [`display_text`]).
//! Rows that have no code on a side (headers, expanders, the empty half of a split row) are
//! skipped when moving.

use std::ops::Range;

/// Tabs are drawn as this many spaces.
pub const TAB_WIDTH: usize = 4;

/// The text as drawn: tabs expanded.
pub fn display_text(text: &str) -> String {
    text.replace('\t', &" ".repeat(TAB_WIDTH))
}

/// One selected token: a byte range on a row's side (0 = left/unified, 1 = right).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caret {
    pub row: usize,
    pub side: usize,
    pub start: usize,
    pub end: usize,
}

impl Caret {
    pub fn new(row: usize, side: usize, token: Range<usize>) -> Self {
        Caret { row, side, start: token.start, end: token.end }
    }
}

/// From the token where the selection started (anchor) to the one it was extended to (head),
/// both included. Both are on the same side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenSel {
    pub anchor: Caret,
    pub head: Caret,
}

/// One row of the selection shape: its columns, and those of the selected rows above and
/// below (which decide where it has borders and rounded corners).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShapeRow {
    pub cols: (usize, usize),
    pub prev: Option<(usize, usize)>,
    pub next: Option<(usize, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl TokenSel {
    pub fn single(caret: Caret) -> Self {
        TokenSel { anchor: caret, head: caret }
    }

    pub fn side(&self) -> usize {
        self.anchor.side
    }

    /// First and last rows, in order.
    pub fn rows(&self) -> (usize, usize) {
        (self.anchor.row.min(self.head.row), self.anchor.row.max(self.head.row))
    }

    /// (row, start) of the first selected byte and (row, end) past the last one.
    fn bounds(&self) -> ((usize, usize), (usize, usize)) {
        let (a, h) = (self.anchor, self.head);
        let start = (a.row, a.start).min((h.row, h.start));
        let end = (a.row, a.end).max((h.row, h.end));
        (start, end)
    }

    /// Whether (row, side, byte index) falls inside the selection.
    pub fn contains(&self, row: usize, side: usize, ix: usize) -> bool {
        let (start, end) = self.bounds();
        side == self.side() && start <= (row, ix) && (row, ix) < end
    }

    /// The selected byte range of a row (for painting), clamped to `len`.
    pub fn range_on(&self, row: usize, side: usize, len: usize) -> Option<Range<usize>> {
        let ((r0, s), (r1, e)) = self.bounds();
        if side != self.side() || row < r0 || row > r1 {
            return None;
        }
        let s = if row == r0 { s.min(len) } else { 0 };
        let e = if row == r1 { e.min(len) } else { len };
        (s < e).then_some(s..e)
    }

    /// Columns (in characters) the selection covers on a row, for drawing it as one shape:
    /// rows before the last extend one column past their end (the newline, as in Sublime
    /// Text), so empty lines inside show and consecutive rows join up. `None` when the row
    /// is outside the selection or has no code on this side.
    pub fn columns_on(&self, row: usize, side: usize, text: Option<&str>) -> Option<(usize, usize)> {
        let text = text?;
        let ((r0, s), (r1, e)) = self.bounds();
        if side != self.side() || row < r0 || row > r1 {
            return None;
        }
        let c0 = if row == r0 { column(text, s) } else { 0 };
        let c1 = if row == r1 { column(text, e) } else { column(text, text.len()) + 1 };
        (c0 < c1).then_some((c0, c1))
    }

    /// A row's part of the selection shape, with the rows above and below it.
    pub fn shape_on(&self, row: usize, side: usize, text: impl Fn(usize) -> Option<String>) -> Option<ShapeRow> {
        let cols = |r: usize| self.columns_on(r, side, text(r).as_deref());
        Some(ShapeRow { cols: cols(row)?, prev: row.checked_sub(1).and_then(cols), next: cols(row + 1) })
    }

    /// The selected text, lines joined with `\n`. `text(row, side)` is a row's display text.
    pub fn text(&self, text: impl Fn(usize, usize) -> Option<String>) -> String {
        let (r0, r1) = self.rows();
        let mut out = vec![];
        for row in r0..=r1 {
            let Some(line) = text(row, self.side()) else { continue };
            if let Some(r) = self.range_on(row, self.side(), line.len()) {
                out.push(line[r].to_string());
            } else if row != r0 && row != r1 {
                out.push(String::new()); // blank line inside the selection
            }
        }
        out.join("\n")
    }
}

/// The tokens of a line, whitespace excluded: identifier/number runs (letters, digits, `_`)
/// and single punctuation characters.
pub fn tokens(text: &str) -> Vec<Range<usize>> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = vec![];
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        let mut end = start + c.len_utf8();
        if word(c) {
            while let Some(&(i, n)) = chars.peek() {
                if !word(n) {
                    break;
                }
                end = i + n.len_utf8();
                chars.next();
            }
        }
        out.push(start..end);
    }
    out
}

/// The token at byte `ix`, else the first one after it, else the last one on the line.
pub fn token_at(text: &str, ix: usize) -> Option<Range<usize>> {
    let tokens = tokens(text);
    tokens.iter().find(|t| ix < t.end).or(tokens.last()).cloned()
}

/// Display column (in characters) of a byte offset.
fn column(text: &str, ix: usize) -> usize {
    text[..ix.min(text.len())].chars().count()
}

/// The token closest to a display column.
fn token_near_column(text: &str, col: usize) -> Option<Range<usize>> {
    let tokens = tokens(text);
    let distance = |t: &Range<usize>| {
        let (a, b) = (column(text, t.start), column(text, t.end));
        if col < a { a - col } else { col.saturating_sub(b - 1) }
    };
    tokens.into_iter().min_by_key(distance)
}

/// Where `caret` moves to in `dir`, or `None` at the edge. `rows` is the row count and
/// `text(row, side)` a row's display text (`None` for rows without code on that side).
pub fn step(caret: Caret, dir: Dir, rows: usize, text: impl Fn(usize, usize) -> Option<String>) -> Option<Caret> {
    let side = caret.side;
    let line_tokens = |row: usize| text(row, side).map(|t| (tokens(&t), t)).filter(|(tk, _)| !tk.is_empty());
    let up = (0..caret.row).rev();
    let down = caret.row + 1..rows;
    match dir {
        Dir::Left => {
            if let Some((tk, _)) = line_tokens(caret.row) {
                if let Some(t) = tk.iter().rev().find(|t| t.end <= caret.start) {
                    return Some(Caret::new(caret.row, side, t.clone()));
                }
            }
            up.into_iter()
                .find_map(|r| line_tokens(r).map(|(tk, _)| Caret::new(r, side, tk.last().unwrap().clone())))
        }
        Dir::Right => {
            if let Some((tk, _)) = line_tokens(caret.row) {
                if let Some(t) = tk.iter().find(|t| t.start >= caret.end) {
                    return Some(Caret::new(caret.row, side, t.clone()));
                }
            }
            down.into_iter().find_map(|r| line_tokens(r).map(|(tk, _)| Caret::new(r, side, tk[0].clone())))
        }
        Dir::Up | Dir::Down => {
            let col = text(caret.row, side).map_or(0, |t| column(&t, caret.start));
            let pick = |r: usize| {
                let (_, t) = line_tokens(r)?;
                Some(Caret::new(r, side, token_near_column(&t, col)?))
            };
            if dir == Dir::Up {
                up.into_iter().find_map(pick)
            } else {
                down.into_iter().find_map(pick)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINES: &[&str] = &["let total = items.len();", "", "    foo(bar_baz)", "}"];

    fn text(row: usize, _side: usize) -> Option<String> {
        LINES.get(row).map(|s| s.to_string())
    }

    fn word(c: Caret) -> &'static str {
        &LINES[c.row][c.start..c.end]
    }

    #[test]
    fn tokenizes_words_and_punctuation() {
        let t = "a.b_c(12)";
        let words: Vec<&str> = tokens(t).into_iter().map(|r| &t[r]).collect();
        assert_eq!(words, ["a", ".", "b_c", "(", "12", ")"]);
        assert_eq!(token_at("  foo bar", 0), Some(2..5)); // whitespace: next token
        assert_eq!(token_at("foo bar", 5), Some(4..7)); // mid-word: whole word
        assert_eq!(token_at("foo  ", 4), Some(0..3)); // past the end: last token
        assert_eq!(token_at("   ", 1), None);
    }

    #[test]
    fn moves_by_token_across_lines() {
        let start = Caret::new(0, 0, 18..21); // "len"
        let right = step(start, Dir::Right, 4, text).unwrap();
        assert_eq!(word(right), "(");
        let mut c = Caret::new(0, 0, 23..24); // ";"
        c = step(c, Dir::Right, 4, text).unwrap(); // skips the empty line
        assert_eq!((c.row, word(c)), (2, "foo"));
        c = step(c, Dir::Left, 4, text).unwrap();
        assert_eq!((c.row, word(c)), (0, ";"));
        assert_eq!(step(Caret::new(3, 0, 0..1), Dir::Right, 4, text), None);
    }

    #[test]
    fn up_and_down_keep_the_column() {
        let c = Caret::new(0, 0, 12..17); // "items", columns 12..17
        let down = step(c, Dir::Down, 4, text).unwrap();
        assert_eq!((down.row, word(down)), (2, "bar_baz"));
        let up = step(down, Dir::Up, 4, text).unwrap(); // from column 8
        assert_eq!(word(up), "total");
    }

    #[test]
    fn multi_line_selection_ranges_and_text() {
        let sel = TokenSel { anchor: Caret::new(2, 0, 8..15), head: Caret::new(0, 0, 12..17) };
        assert_eq!(sel.rows(), (0, 2));
        assert_eq!(sel.range_on(0, 0, 24), Some(12..24));
        assert_eq!(sel.range_on(1, 0, 0), None);
        assert_eq!(sel.range_on(2, 0, 16), Some(0..15));
        assert_eq!(sel.range_on(2, 1, 16), None);
        assert_eq!(sel.text(text), "items.len();\n\n    foo(bar_baz");
        assert!(sel.contains(0, 0, 12) && !sel.contains(0, 0, 11) && !sel.contains(2, 0, 15));
        // One shape: rows before the last run one column past the line end.
        let shape = |r| sel.shape_on(r, 0, |r| text(r, 0));
        assert_eq!(shape(0), Some(ShapeRow { cols: (12, 25), prev: None, next: Some((0, 1)) }));
        assert_eq!(shape(1), Some(ShapeRow { cols: (0, 1), prev: Some((12, 25)), next: Some((0, 15)) }));
        assert_eq!(shape(2), Some(ShapeRow { cols: (0, 15), prev: Some((0, 1)), next: None }));
    }
}
