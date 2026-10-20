//! A small GitHub-flavored markdown renderer for PR descriptions, comments and Claude's
//! answers: headings, paragraphs (with GitHub's hard line breaks), bullet/numbered/task
//! lists, block quotes, tables, rules and highlighted code blocks; inline code, bold,
//! italic, strikethrough and clickable links.

use std::ops::Range;

use gpui::{
    div, font, prelude::*, px, rems, Div, FontStyle, FontWeight, Hsla, InteractiveText, SharedString, StrikethroughStyle,
    StyledText, TextRun, UnderlineStyle,
};

use crate::github;
use crate::theme::Theme;

use super::components::code_line;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MdSize {
    /// Chat answers and the review log.
    Small,
    /// PR descriptions and comments.
    Normal,
}

#[derive(Debug, PartialEq)]
enum Block {
    Heading(u8, String),
    Para(String),
    Code(Option<String>, Vec<String>),
    Quote(String),
    List(Vec<Item>),
    Table(Vec<String>, Vec<Vec<String>>),
    Rule,
}

#[derive(Debug, PartialEq)]
struct Item {
    depth: usize,
    marker: Marker,
    text: String,
}

#[derive(Debug, PartialEq)]
enum Marker {
    Bullet,
    Number(u32),
    Task(bool),
}

pub fn render_markdown(t: &Theme, text: &str) -> Div {
    render(t, text, MdSize::Small, &id_for(text))
}

/// `id` must be unique among the markdown shown at once (it keys link click state).
pub fn render(t: &Theme, text: &str, size: MdSize, id: &str) -> Div {
    let blocks = parse(text);
    let (text_size, gap) = match size {
        MdSize::Small => (rems(0.875), px(6.)),
        MdSize::Normal => (rems(0.9375), px(10.)),
    };
    div()
        .flex()
        .flex_col()
        .gap(gap)
        .text_size(text_size)
        .line_height(rems(1.5))
        .text_color(t.text)
        .children(blocks.into_iter().enumerate().map(|(i, b)| render_block(t, b, &format!("{id}-{i}"), size)))
}

fn id_for(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    format!("md{:x}", h.finish())
}

fn render_block(t: &Theme, block: Block, id: &str, size: MdSize) -> gpui::AnyElement {
    match block {
        Block::Heading(level, text) => {
            let el = div().font_weight(FontWeight::SEMIBOLD).line_height(rems(1.25)).child(inline(t, &text, t.text, id));
            match (level, size) {
                (1, MdSize::Normal) => el.text_size(rems(1.5)).pb_1().mt_2().border_b_1().border_color(t.border),
                (2, MdSize::Normal) => el.text_size(rems(1.25)).pb_1().mt_2().border_b_1().border_color(t.border),
                (3, MdSize::Normal) | (1, MdSize::Small) => el.text_size(rems(1.1)).mt_1(),
                _ => el.text_size(rems(1.0)).mt_1(),
            }
            .into_any_element()
        }
        Block::Para(text) => div().child(inline(t, &text, t.text, id)).into_any_element(),
        Block::Code(lang, lines) => code_block(t, lang.as_deref(), &lines, id).into_any_element(),
        Block::Quote(text) => div()
            .pl_3()
            .border_l_4()
            .border_color(t.border_strong)
            .text_color(t.muted)
            .child(render(t, &text, size, id).text_color(t.muted))
            .into_any_element(),
        Block::Rule => div().h(px(2.)).my_1().bg(t.border).into_any_element(),
        Block::List(items) => div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(items.into_iter().enumerate().map(|(i, item)| {
                let (marker, color): (SharedString, Hsla) = match item.marker {
                    Marker::Bullet => (if item.depth % 2 == 0 { "•" } else { "◦" }.into(), t.muted),
                    Marker::Number(n) => (format!("{n}.").into(), t.muted),
                    Marker::Task(true) => ("☑".into(), t.add_fg),
                    Marker::Task(false) => ("☐".into(), t.muted),
                };
                div()
                    .flex()
                    .gap_2()
                    .pl(px(item.depth as f32 * 20. + 4.))
                    .child(div().flex_none().min_w(px(16.)).flex().justify_end().text_color(color).child(marker))
                    .child(div().flex_1().min_w_0().child(inline(t, &item.text, t.text, &format!("{id}-{i}"))))
            }))
            .into_any_element(),
        Block::Table(header, rows) => {
            let cols = header.len().max(1);
            let row = |cells: &[String], head: bool, key: String| {
                div()
                    .flex()
                    .when(head, |d| d.bg(t.raised).font_weight(FontWeight::SEMIBOLD))
                    .children((0..cols).map(|c| {
                        let text = cells.get(c).map(String::as_str).unwrap_or_default();
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .py_1()
                            .when(c > 0, |d| d.border_l_1().border_color(t.border))
                            .child(inline(t, text, t.text, &format!("{key}-{c}")))
                    }))
            };
            div()
                .flex()
                .flex_col()
                .rounded_md()
                .border_1()
                .border_color(t.border)
                .overflow_hidden()
                .child(row(&header, true, format!("{id}-h")))
                .children(rows.iter().enumerate().map(|(r, cells)| {
                    row(cells, false, format!("{id}-{r}"))
                        .border_t_1()
                        .border_color(t.border)
                        .when(r % 2 == 1, |d| d.bg(t.raised.opacity(0.5)))
                }))
                .into_any_element()
        }
    }
}

fn code_block(t: &Theme, lang: Option<&str>, lines: &[String], id: &str) -> impl IntoElement {
    let lang = lang.and_then(prmator_syntax::Lang::from_name);
    let highlights = lang.map(|l| prmator_syntax::highlight(l, &lines.join("\n")));
    div()
        .id(SharedString::from(format!("{id}-code")))
        .p_3()
        .rounded_md()
        .bg(t.code_bg)
        .border_1()
        .border_color(t.border)
        .font_family(t.mono.clone())
        .text_size(px(t.code_size * 0.9))
        .line_height(rems(1.35))
        .overflow_x_scroll()
        .children(lines.iter().enumerate().map(|(i, l)| {
            let diff_color = match l.as_bytes().first() {
                Some(b'+') if lang.is_none() => Some(t.add_fg),
                Some(b'-') if lang.is_none() => Some(t.del_fg),
                _ => None,
            };
            let spans = highlights.as_ref().and_then(|h| h.get(i));
            let (_, styled) = code_line(t, if l.is_empty() { " " } else { l }, spans, None, t.code_bg);
            div().whitespace_nowrap().text_color(diff_color.unwrap_or(t.text)).child(styled)
        }))
}

// ---- inline

#[derive(Clone, Copy, Default, PartialEq)]
struct Flags {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    /// Index into the link list.
    link: Option<usize>,
}

#[derive(Default)]
struct Inline {
    text: String,
    spans: Vec<(Range<usize>, Flags)>,
    links: Vec<String>,
}

impl Inline {
    fn push(&mut self, s: &str, flags: Flags) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(s);
        match self.spans.last_mut() {
            Some((r, f)) if *f == flags && r.end == start => r.end = self.text.len(),
            _ => self.spans.push((start..self.text.len(), flags)),
        }
    }
}

/// Inline markdown as styled text; links are clickable.
fn inline(t: &Theme, src: &str, color: Hsla, id: &str) -> gpui::AnyElement {
    let mut out = Inline::default();
    parse_inline(src, Flags::default(), &mut out);
    let runs: Vec<TextRun> = out
        .spans
        .iter()
        .map(|(r, f)| {
            let mut fnt = font(if f.code { t.mono.clone() } else { t.ui_font.clone() });
            if f.bold {
                fnt.weight = FontWeight::SEMIBOLD;
            }
            if f.italic {
                fnt.style = FontStyle::Italic;
            }
            TextRun {
                len: r.len(),
                font: fnt,
                color: if f.link.is_some() { t.accent } else { color },
                background_color: f.code.then(|| t.muted.opacity(0.15)),
                underline: f.link.map(|_| UnderlineStyle { color: Some(t.accent.opacity(0.5)), thickness: px(1.), wavy: false }),
                strikethrough: f.strike.then(|| StrikethroughStyle { color: Some(color), thickness: px(1.) }),
            }
        })
        .collect();
    let link_ranges: Vec<(Range<usize>, String)> = out
        .spans
        .iter()
        .filter_map(|(r, f)| Some((r.clone(), out.links.get(f.link?)?.clone())))
        .collect();
    let styled = StyledText::new(SharedString::from(out.text.clone())).with_runs(runs);
    if link_ranges.is_empty() {
        return styled.into_any_element();
    }
    let ranges: Vec<Range<usize>> = link_ranges.iter().map(|(r, _)| r.clone()).collect();
    let urls: Vec<String> = link_ranges.into_iter().map(|(_, u)| u).collect();
    InteractiveText::new(SharedString::from(format!("{id}-links")), styled)
        .on_click(ranges, move |ix, _, _| {
            if let Some(url) = urls.get(ix) {
                github::open_in_browser(url);
            }
        })
        .into_any_element()
}

fn parse_inline(src: &str, flags: Flags, out: &mut Inline) {
    let bytes = src.as_bytes();
    let mut plain_start = 0;
    let mut i = 0;
    let flush = |out: &mut Inline, from: usize, to: usize| out.push(&src[from..to], flags);
    while i < bytes.len() {
        let rest = &src[i..];
        // `code`
        if rest.starts_with('`') && !flags.code {
            let ticks = rest.len() - rest.trim_start_matches('`').len();
            let fence = &rest[..ticks];
            if let Some(end) = rest[ticks..].find(fence) {
                flush(out, plain_start, i);
                out.push(rest[ticks..ticks + end].trim(), Flags { code: true, ..flags });
                i += ticks + end + ticks;
                plain_start = i;
                continue;
            }
        }
        // ![alt](url) and [text](url)
        if rest.starts_with("![") || rest.starts_with('[') {
            let image = rest.starts_with('!');
            let open = if image { 1 } else { 0 };
            if let Some((label, url, len)) = link_at(&rest[open..]) {
                flush(out, plain_start, i);
                out.links.push(url.to_string());
                let link = Some(out.links.len() - 1);
                if image {
                    out.push(&format!("🖼 {}", if label.is_empty() { "image" } else { label }), Flags { link, ..flags });
                } else {
                    parse_inline(label, Flags { link, ..flags }, out);
                }
                i += open + len;
                plain_start = i;
                continue;
            }
        }
        // Bare URLs.
        if (rest.starts_with("https://") || rest.starts_with("http://")) && flags.link.is_none() {
            let len = rest.find(|c: char| c.is_whitespace() || c == ')' || c == '>').unwrap_or(rest.len());
            let url = rest[..len].trim_end_matches(['.', ',', ';']);
            flush(out, plain_start, i);
            out.links.push(url.to_string());
            out.push(url, Flags { link: Some(out.links.len() - 1), ..flags });
            i += url.len();
            plain_start = i;
            continue;
        }
        // **bold**, __bold__, ~~strike~~, *italic*, _italic_
        for (marker, set) in [("**", 0), ("__", 0), ("~~", 2), ("*", 1), ("_", 1)] {
            if !rest.starts_with(marker) {
                continue;
            }
            // `_` only at word boundaries (snake_case stays as is).
            let before = src[..i].chars().last();
            if marker.starts_with('_') && before.is_some_and(|c| c.is_alphanumeric()) {
                continue;
            }
            let inner_start = marker.len();
            let Some(end) = rest[inner_start..].find(marker) else { continue };
            let inner = &rest[inner_start..inner_start + end];
            if inner.is_empty() || inner.starts_with(' ') || inner.ends_with(' ') {
                continue;
            }
            let after = rest[inner_start + end + marker.len()..].chars().next();
            if marker.starts_with('_') && after.is_some_and(|c| c.is_alphanumeric()) {
                continue;
            }
            flush(out, plain_start, i);
            let mut f = flags;
            match set {
                0 => f.bold = true,
                1 => f.italic = true,
                _ => f.strike = true,
            }
            parse_inline(inner, f, out);
            i += inner_start + end + marker.len();
            plain_start = i;
            break;
        }
        if plain_start > i {
            continue;
        }
        if plain_start == i && i < bytes.len() && i != plain_start {
            continue;
        }
        // Advance one character.
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    if plain_start < src.len() {
        flush(out, plain_start, src.len());
    }
}

/// `[label](url)` at the start of `s`: (label, url, total length).
fn link_at(s: &str) -> Option<(&str, &str, usize)> {
    if !s.starts_with('[') {
        return None;
    }
    let mut depth = 0;
    let mut close = None;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let rest = &s[close + 1..];
    if !rest.starts_with('(') {
        return None;
    }
    let end = rest.find(')')?;
    let url = rest[1..end].split_whitespace().next().unwrap_or_default();
    Some((&s[1..close], url, close + 1 + end + 1))
}

// ---- blocks

fn parse(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = vec![];
    let mut para: Vec<&str> = vec![];
    let flush = |para: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !para.is_empty() {
            blocks.push(Block::Para(para.iter().map(|l| l.trim()).collect::<Vec<_>>().join("\n")));
            para.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            flush(&mut para, &mut blocks);
            i += 1;
            continue;
        }
        // Fenced code.
        if let Some(info) = trimmed.strip_prefix("```").or_else(|| trimmed.strip_prefix("~~~")) {
            flush(&mut para, &mut blocks);
            let fence = &trimmed[..3];
            let lang = info.trim().split_whitespace().next().filter(|l| !l.is_empty()).map(String::from);
            let mut code = vec![];
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with(fence) {
                code.push(lines[i].to_string());
                i += 1;
            }
            blocks.push(Block::Code(lang, code));
            i += 1;
            continue;
        }
        // Heading.
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush(&mut para, &mut blocks);
            blocks.push(Block::Heading(hashes as u8, trimmed[hashes..].trim().trim_end_matches('#').trim().to_string()));
            i += 1;
            continue;
        }
        // Horizontal rule.
        let compact: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.len() >= 3 && (compact.chars().all(|c| c == '-') || compact.chars().all(|c| c == '*') || compact.chars().all(|c| c == '_')) {
            flush(&mut para, &mut blocks);
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }
        // Block quote.
        if trimmed.starts_with('>') {
            flush(&mut para, &mut blocks);
            let mut quoted = vec![];
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let l = lines[i].trim_start()[1..].strip_prefix(' ').unwrap_or(&lines[i].trim_start()[1..]);
                quoted.push(l.to_string());
                i += 1;
            }
            blocks.push(Block::Quote(quoted.join("\n")));
            continue;
        }
        // Table: a row with pipes followed by a separator row.
        if trimmed.contains('|') && lines.get(i + 1).is_some_and(|n| is_table_separator(n)) {
            flush(&mut para, &mut blocks);
            let header = table_cells(trimmed);
            let mut rows = vec![];
            i += 2;
            while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
                rows.push(table_cells(lines[i]));
                i += 1;
            }
            blocks.push(Block::Table(header, rows));
            continue;
        }
        // List.
        if list_item(line).is_some() {
            flush(&mut para, &mut blocks);
            let mut items: Vec<Item> = vec![];
            while i < lines.len() {
                let l = lines[i];
                if let Some(item) = list_item(l) {
                    items.push(item);
                } else if !l.trim().is_empty() && l.starts_with(' ') && !items.is_empty() {
                    // Continuation of the previous item.
                    let last = items.last_mut().unwrap();
                    last.text.push('\n');
                    last.text.push_str(l.trim());
                } else {
                    break;
                }
                i += 1;
            }
            blocks.push(Block::List(items));
            continue;
        }
        para.push(line);
        i += 1;
    }
    flush(&mut para, &mut blocks);
    blocks
}

fn list_item(line: &str) -> Option<Item> {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim_start();
    let depth = indent / 2;
    let (marker, rest) = if let Some(rest) = t.strip_prefix("- ").or(t.strip_prefix("* ")).or(t.strip_prefix("+ ")) {
        (Marker::Bullet, rest)
    } else {
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        let after = t.get(digits.len()..)?;
        let rest = after.strip_prefix(". ").or(after.strip_prefix(") "))?;
        if digits.is_empty() {
            return None;
        }
        (Marker::Number(digits.parse().ok()?), rest)
    };
    let (marker, rest) = if let Some(r) = rest.strip_prefix("[ ] ") {
        (Marker::Task(false), r)
    } else if let Some(r) = rest.strip_prefix("[x] ").or(rest.strip_prefix("[X] ")) {
        (Marker::Task(true), r)
    } else {
        (marker, rest)
    };
    Some(Item { depth, marker, text: rest.trim().to_string() })
}

fn is_table_separator(line: &str) -> bool {
    let t = line.trim().trim_matches('|');
    !t.is_empty()
        && t.split('|').all(|cell| {
            let c = cell.trim().trim_matches(':');
            !c.is_empty() && c.chars().all(|ch| ch == '-')
        })
}

fn table_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks() {
        let md = "## Why\n\nLine one\nline two\n\n- a\n  more a\n- [x] done\n1. first\n\n> quoted\n\n| A | B |\n| --- | ---: |\n| 1 | 2 |\n\n---\n```rust\nfn x() {}\n```";
        let blocks = parse(md);
        assert_eq!(blocks[0], Block::Heading(2, "Why".into()));
        assert_eq!(blocks[1], Block::Para("Line one\nline two".into()));
        assert_eq!(
            blocks[2],
            Block::List(vec![
                Item { depth: 0, marker: Marker::Bullet, text: "a\nmore a".into() },
                Item { depth: 0, marker: Marker::Task(true), text: "done".into() },
                Item { depth: 0, marker: Marker::Number(1), text: "first".into() },
            ])
        );
        assert_eq!(blocks[3], Block::Quote("quoted".into()));
        assert_eq!(blocks[4], Block::Table(vec!["A".into(), "B".into()], vec![vec!["1".into(), "2".into()]]));
        assert_eq!(blocks[5], Block::Rule);
        assert_eq!(blocks[6], Block::Code(Some("rust".into()), vec!["fn x() {}".into()]));
    }

    #[test]
    fn parses_inline() {
        let mut out = Inline::default();
        parse_inline("Use **`foo`** and *bar*, see [docs](https://x.y) or https://a.b. snake_case_name", Flags::default(), &mut out);
        assert_eq!(out.text, "Use foo and bar, see docs or https://a.b. snake_case_name");
        let styled: Vec<(&str, bool, bool, bool, bool)> = out
            .spans
            .iter()
            .map(|(r, f)| (&out.text[r.clone()], f.bold, f.italic, f.code, f.link.is_some()))
            .collect();
        assert!(styled.contains(&("foo", true, false, true, false)));
        assert!(styled.contains(&("bar", false, true, false, false)));
        assert!(styled.contains(&("docs", false, false, false, true)));
        assert!(styled.contains(&("https://a.b", false, false, false, true)));
        assert_eq!(out.links, vec!["https://x.y", "https://a.b"]);
        assert!(styled.iter().any(|(s, ..)| s.contains("snake_case_name")));
    }
}
