//! Text input, adapted from gpui's examples/input.rs (Apache-2.0). Single-line by default
//! (Enter submits); [`TextInput::multiline`] wraps text and accepts newlines, for comments.

use std::ops::Range;

use gpui::{
    actions, div, fill, point, prelude::*, px, relative, rems, size, App, AvailableSpace, Bounds, TextAlign, WrappedLine,
    ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window,
};
use unicode_segmentation::*;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Submit,
        Enter,
        Newline,
        Up,
        Down,
        SelectUp,
        SelectDown,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        LineStart,
        LineEnd,
        SelectLineStart,
        SelectLineEnd,
        TextStart,
        TextEnd,
        SelectTextStart,
        SelectTextEnd,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        DeleteToLineEnd,
    ]
);

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    submit_empty: bool,
    /// Wraps text and accepts newlines.
    multiline: bool,
    /// Multi-line only: Enter inserts a newline (⌘↵ submits) instead of submitting.
    enter_newline: bool,
    /// Multi-line layout from the last paint: one wrapped line per paragraph.
    last_lines: Vec<WrappedLine>,
    last_line_height: Pixels,
}

/// Emitted on Enter with the (non-empty) content; the input clears itself.
pub struct Submitted(pub String);

impl EventEmitter<Submitted> for TextInput {}

/// Emitted whenever the content changes (typing, paste, `set_text`).
pub struct Changed(pub String);

impl EventEmitter<Changed> for TextInput {}

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("TextInput");
    cx.bind_keys([
        KeyBinding::new("enter", Enter, ctx),
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("cmd-a", SelectAll, ctx),
        KeyBinding::new("cmd-v", Paste, ctx),
        KeyBinding::new("cmd-c", Copy, ctx),
        KeyBinding::new("cmd-x", Cut, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        // macOS text navigation: ⌥ by word, ⌘ to the line's (↑↓: the text's) ends, ⇧ selects.
        KeyBinding::new("alt-left", WordLeft, ctx),
        KeyBinding::new("alt-right", WordRight, ctx),
        KeyBinding::new("alt-shift-left", SelectWordLeft, ctx),
        KeyBinding::new("alt-shift-right", SelectWordRight, ctx),
        KeyBinding::new("cmd-left", LineStart, ctx),
        KeyBinding::new("cmd-right", LineEnd, ctx),
        KeyBinding::new("ctrl-a", LineStart, ctx),
        KeyBinding::new("ctrl-e", LineEnd, ctx),
        KeyBinding::new("cmd-shift-left", SelectLineStart, ctx),
        KeyBinding::new("cmd-shift-right", SelectLineEnd, ctx),
        KeyBinding::new("shift-home", SelectTextStart, ctx),
        KeyBinding::new("shift-end", SelectTextEnd, ctx),
        KeyBinding::new("alt-backspace", DeleteWordLeft, ctx),
        KeyBinding::new("alt-delete", DeleteWordRight, ctx),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, ctx),
        KeyBinding::new("cmd-delete", DeleteToLineEnd, ctx),
        KeyBinding::new("ctrl-k", DeleteToLineEnd, ctx),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, ctx),
    ]);
    // Multi-line inputs only, so single-line ones keep ↑/↓ and ⌘↵ for their containers
    // (palette navigation, finishing a chat).
    let multi = Some("TextInput && multiline");
    cx.bind_keys([
        KeyBinding::new("shift-enter", Newline, multi),
        KeyBinding::new("cmd-enter", Submit, multi),
        KeyBinding::new("up", Up, multi),
        KeyBinding::new("down", Down, multi),
        KeyBinding::new("shift-up", SelectUp, multi),
        KeyBinding::new("shift-down", SelectDown, multi),
        KeyBinding::new("cmd-up", TextStart, multi),
        KeyBinding::new("cmd-down", TextEnd, multi),
        KeyBinding::new("cmd-shift-up", SelectTextStart, multi),
        KeyBinding::new("cmd-shift-down", SelectTextEnd, multi),
    ]);
}

impl TextInput {
    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    pub fn new(placeholder: &'static str, cx: &mut Context<Self>) -> Self {
        TextInput {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            submit_empty: false,
            multiline: false,
            enter_newline: false,
            last_lines: vec![],
            last_line_height: px(20.),
        }
    }

    /// Wrapping, multi-line input. With `enter_newline`, Enter adds a line and ⌘↵ submits.
    pub fn multiline(mut self, enter_newline: bool) -> Self {
        self.multiline = true;
        self.enter_newline = enter_newline;
        self
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus_handle);
    }

    /// Also emit [`Submitted`] for an empty input (used by the palette: Enter picks the
    /// first match even with no query).
    pub fn submit_empty(mut self) -> Self {
        self.submit_empty = true;
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// Replaces the content and puts the cursor at the end.
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        cx.emit(Changed(self.content.to_string()));
        cx.notify();
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.content.trim().to_string();
        if !text.is_empty() || self.submit_empty {
            self.reset();
            cx.emit(Changed(String::new()));
            cx.emit(Submitted(text));
            cx.notify();
        }
    }

    fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiline && self.enter_newline {
            self.replace_text_in_range(None, "\n", window, cx);
        } else {
            self.submit(&Submit, window, cx);
        }
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertically(-1., false, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertically(1., false, cx);
    }

    /// Moves the cursor one visual line up (-1) or down (1), extending the selection
    /// when `select`.
    fn move_vertically(&mut self, direction: f32, select: bool, cx: &mut Context<Self>) {
        let Some(bounds) = self.last_bounds else { return };
        let Some(p) = self.area_position(self.cursor_offset()) else { return };
        let lh = self.last_line_height;
        let target = point(bounds.left() + p.x, bounds.top() + p.y + lh * direction + lh / 2.);
        let ix = if target.y < bounds.top() {
            0
        } else if target.y > bounds.bottom() {
            self.content.len()
        } else {
            self.index_for_mouse_position(target)
        };
        self.motion(ix, select, cx);
    }

    /// Moves the cursor to `ix`, or extends the selection there.
    fn motion(&mut self, ix: usize, select: bool, cx: &mut Context<Self>) {
        if select {
            self.select_to(ix, cx)
        } else {
            self.move_to(ix, cx)
        }
    }

    /// Deletes from the cursor to `ix` (or the selection, if any).
    fn delete_to(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(ix, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertically(-1., true, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertically(1., true, cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(word_start(&self.content, self.cursor_offset()), false, cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(word_end(&self.content, self.cursor_offset()), false, cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(word_start(&self.content, self.cursor_offset()), true, cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(word_end(&self.content, self.cursor_offset()), true, cx);
    }

    fn line_start(&mut self, _: &LineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(line_start(&self.content, self.cursor_offset()), false, cx);
    }

    fn line_end(&mut self, _: &LineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(line_end(&self.content, self.cursor_offset()), false, cx);
    }

    fn select_line_start(&mut self, _: &SelectLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(line_start(&self.content, self.cursor_offset()), true, cx);
    }

    fn select_line_end(&mut self, _: &SelectLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(line_end(&self.content, self.cursor_offset()), true, cx);
    }

    fn text_start(&mut self, _: &TextStart, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(0, false, cx);
    }

    fn text_end(&mut self, _: &TextEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(self.content.len(), false, cx);
    }

    fn select_text_start(&mut self, _: &SelectTextStart, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(0, true, cx);
    }

    fn select_text_end(&mut self, _: &SelectTextEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(self.content.len(), true, cx);
    }

    fn delete_word_left(&mut self, _: &DeleteWordLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_to(word_start(&self.content, self.cursor_offset()), window, cx);
    }

    fn delete_word_right(&mut self, _: &DeleteWordRight, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_to(word_end(&self.content, self.cursor_offset()), window, cx);
    }

    fn delete_to_line_start(&mut self, _: &DeleteToLineStart, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_to(line_start(&self.content, self.cursor_offset()), window, cx);
    }

    fn delete_to_line_end(&mut self, _: &DeleteToLineEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_to(line_end(&self.content, self.cursor_offset()), window, cx);
    }

    /// Multi-line: position (relative to the text's origin) of byte `ix`.
    fn area_position(&self, ix: usize) -> Option<Point<Pixels>> {
        area_position(&self.last_lines, self.last_line_height, ix)
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = if self.multiline { text } else { text.replace("\n", " ") };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        if self.multiline {
            let Some(bounds) = self.last_bounds else { return 0 };
            let lh = self.last_line_height;
            let mut y = bounds.top();
            let mut start = 0;
            for line in &self.last_lines {
                let h = line.size(lh).height;
                if position.y < y + h {
                    let local = point(position.x - bounds.left(), (position.y - y).max(px(0.)));
                    let ix = line.closest_index_for_position(local, lh).unwrap_or_else(|i| i);
                    return (start + ix).min(self.content.len());
                }
                y += h;
                start += line.len() + 1;
            }
            return self.content.len();
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    fn reset(&mut self) {
        self.content = "".into();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        self.last_bounds = None;
        self.last_lines.clear();
        self.is_selecting = false;
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// ⌥←: the start of the word before `offset` (skipping spaces and punctuation first).
fn word_start(text: &str, offset: usize) -> usize {
    let mut chars = text[..offset].char_indices().rev().peekable();
    let mut ix = offset;
    for in_word in [false, true] {
        while let Some((i, _)) = chars.next_if(|&(_, c)| is_word_char(c) == in_word) {
            ix = i;
        }
    }
    ix
}

/// ⌥→: the end of the word after `offset` (skipping spaces and punctuation first).
fn word_end(text: &str, offset: usize) -> usize {
    let mut chars = text[offset..].char_indices().peekable();
    let mut ix = offset;
    for in_word in [false, true] {
        while let Some((i, c)) = chars.next_if(|&(_, c)| is_word_char(c) == in_word) {
            ix = offset + i + c.len_utf8();
        }
    }
    ix
}

/// ⌘←: the start of the line (paragraph) `offset` is on.
fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |i| i + 1)
}

/// ⌘→: the end of the line (paragraph) `offset` is on.
fn line_end(text: &str, offset: usize) -> usize {
    text[offset..].find('\n').map_or(text.len(), |i| offset + i)
}

/// Position of byte `ix` within paragraphs laid out top to bottom (`\n` between them).
fn area_position(lines: &[WrappedLine], lh: Pixels, ix: usize) -> Option<Point<Pixels>> {
    let mut y = px(0.);
    let mut start = 0;
    for (i, line) in lines.iter().enumerate() {
        let end = start + line.len();
        if ix <= end || i + 1 == lines.len() {
            let local = (ix.saturating_sub(start)).min(line.len());
            let p = line.position_for_index(local, lh).unwrap_or(point(px(0.), px(0.)));
            return Some(point(p.x, y + p.y));
        }
        y += line.size(lh).height;
        start = end + 1;
    }
    None
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.emit(Changed(self.content.to_string()));
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        cx.emit(Changed(self.content.to_string()));
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        if self.multiline {
            let p = self.area_position(range.start)?;
            let origin = point(bounds.left() + p.x, bounds.top() + p.y);
            return Some(Bounds::new(origin, size(px(2.), self.last_line_height)));
        }
        let last_layout = self.last_layout.as_ref()?;
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        if self.multiline {
            return Some(self.offset_to_utf16(self.index_for_mouse_position(point)));
        }
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;

        assert_eq!(last_layout.text, self.content);
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), crate::theme::theme(cx).dim)
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    crate::theme::theme(cx).accent,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    crate::theme::theme(cx).accent.opacity(0.3),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let line = prepaint.line.take().unwrap();
        line.paint(bounds.origin, window.line_height(), window, cx)
            .unwrap();

        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

/// Wrapped, multi-line text with cursor and selection.
struct TextAreaElement {
    input: Entity<TextInput>,
}

struct AreaPrepaint {
    lines: Vec<WrappedLine>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
}

impl IntoElement for TextAreaElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// The text to show and its runs (placeholder when empty, marked text underlined).
fn display_runs(input: &TextInput, window: &Window, cx: &App) -> (SharedString, Vec<TextRun>) {
    let style = window.text_style();
    let (text, color) = if input.content.is_empty() {
        (input.placeholder.clone(), crate::theme::theme(cx).dim)
    } else {
        (input.content.clone(), style.color)
    };
    let run = TextRun { len: text.len(), font: style.font(), color, background_color: None, underline: None, strikethrough: None };
    let runs = match (&input.marked_range, input.content.is_empty()) {
        (Some(m), false) => vec![
            TextRun { len: m.start, ..run.clone() },
            TextRun {
                len: m.end - m.start,
                underline: Some(UnderlineStyle { color: Some(run.color), thickness: px(1.0), wavy: false }),
                ..run.clone()
            },
            TextRun { len: text.len() - m.end, ..run },
        ]
        .into_iter()
        .filter(|r| r.len > 0)
        .collect(),
        _ => vec![run],
    };
    (text, runs)
}

fn shape_area(input: &TextInput, width: Option<Pixels>, window: &mut Window, cx: &App) -> Vec<WrappedLine> {
    let (text, runs) = display_runs(input, window, cx);
    let font_size = window.text_style().font_size.to_pixels(window.rem_size());
    window
        .text_system()
        .shape_text(text, font_size, &runs, width, None)
        .map(|lines| lines.into_iter().collect())
        .unwrap_or_default()
}

impl Element for TextAreaElement {
    type RequestLayoutState = ();
    type PrepaintState = AreaPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let input = self.input.clone();
        let line_height = window.line_height();
        let layout = window.request_measured_layout(style, move |known, available, window, cx| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let lines = shape_area(input.read(cx), width, window, cx);
            let height = lines.iter().map(|l| l.size(line_height).height).fold(px(0.), |a, b| a + b).max(line_height);
            size(width.unwrap_or(px(200.)), height)
        });
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let lines = shape_area(input, Some(bounds.size.width), window, cx);
        let lh = window.line_height();
        let accent = crate::theme::theme(cx).accent;
        let at = |ix: usize| {
            let p = if input.content.is_empty() { Some(point(px(0.), px(0.))) } else { area_position(&lines, lh, ix) };
            p.map(|p| point(bounds.left() + p.x, bounds.top() + p.y))
        };
        let range = input.selected_range.clone();
        let mut selections = vec![];
        let mut cursor = None;
        if range.is_empty() {
            if let Some(p) = at(input.cursor_offset()) {
                cursor = Some(fill(Bounds::new(p, size(px(2.), lh)), accent));
            }
        } else if let (Some(a), Some(b)) = (at(range.start), at(range.end)) {
            let color = accent.opacity(0.3);
            if a.y == b.y {
                selections.push(fill(Bounds::from_corners(a, point(b.x, b.y + lh)), color));
            } else {
                selections.push(fill(Bounds::from_corners(a, point(bounds.right(), a.y + lh)), color));
                if b.y > a.y + lh {
                    selections.push(fill(
                        Bounds::from_corners(point(bounds.left(), a.y + lh), point(bounds.right(), b.y)),
                        color,
                    ));
                }
                selections.push(fill(Bounds::from_corners(point(bounds.left(), b.y), point(b.x, b.y + lh)), color));
            }
        }
        AreaPrepaint { lines, cursor, selections }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);
        for quad in prepaint.selections.drain(..) {
            window.paint_quad(quad);
        }
        let lh = window.line_height();
        let mut y = bounds.top();
        for line in &prepaint.lines {
            let _ = line.paint(point(bounds.left(), y), lh, TextAlign::Left, Some(bounds), window, cx);
            y += line.size(lh).height;
        }
        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }
        let lines = std::mem::take(&mut prepaint.lines);
        self.input.update(cx, |input, _| {
            input.last_lines = if input.content.is_empty() { vec![] } else { lines };
            input.last_bounds = Some(bounds);
            input.last_line_height = lh;
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let multiline = self.multiline;
        div()
            .flex()
            .key_context(if multiline { "TextInput multiline" } else { "TextInput" })
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::line_start))
            .on_action(cx.listener(Self::line_end))
            .on_action(cx.listener(Self::select_line_start))
            .on_action(cx.listener(Self::select_line_end))
            .on_action(cx.listener(Self::text_start))
            .on_action(cx.listener(Self::text_end))
            .on_action(cx.listener(Self::select_text_start))
            .on_action(cx.listener(Self::select_text_end))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::delete_to_line_end))
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::submit))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            // Scales with the UI font size (the window's rem size).
            .line_height(rems(1.25))
            .text_size(rems(0.875))
            .text_color(crate::theme::theme(cx).text)
            .map(|d| {
                if multiline {
                    d.child(
                        div()
                            .min_h(rems(1.25 * 3. + 1.))
                            .w_full()
                            .px(px(10.))
                            .py(rems(0.5))
                            .child(TextAreaElement { input: cx.entity() }),
                    )
                } else {
                    d.child(
                        div()
                            .h(rems(1.25 + 1.))
                            .w_full()
                            .px(px(10.))
                            .py(rems(0.5))
                            .child(TextElement { input: cx.entity() }),
                    )
                }
            })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_and_line_motions() {
        let text = "let total = items.len();\n  next_line here";
        // ⌥← from the end of `items.len` lands on `len`, then `items`, then `total`.
        let at = text.find("();").unwrap();
        assert_eq!(&text[word_start(text, at)..at], "len");
        let at = word_start(text, at);
        assert_eq!(&text[word_start(text, at)..], &text[text.find("items").unwrap()..]);
        assert_eq!(word_start(text, 4), 0);
        assert_eq!(word_start(text, 0), 0);
        // ⌥→ skips spaces and punctuation, then the word; `_` is part of a word.
        assert_eq!(&text[3..word_end(text, 3)], " total");
        let nl = text.find('\n').unwrap();
        assert_eq!(&text[nl..word_end(text, nl)], "\n  next_line");
        assert_eq!(word_end(text, text.len()), text.len());
        // ⌘← / ⌘→ stay on the cursor's line.
        let here = text.find("here").unwrap();
        assert_eq!(line_start(text, here), nl + 1);
        assert_eq!(line_end(text, here), text.len());
        assert_eq!((line_start(text, 5), line_end(text, 5)), (0, nl));
        // Multi-byte characters stay on boundaries.
        let accents = "café olé";
        assert_eq!(word_start(accents, accents.len()), accents.find("olé").unwrap());
        assert_eq!(word_end(accents, 0), "café".len());
    }
}
