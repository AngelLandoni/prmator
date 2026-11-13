//! Mouse selection of rendered (read-only) text, like in a browser: drag to select across
//! the text blocks of one scope (a chat conversation, a review-log entry, a comment…),
//! double-click selects a word, triple-click a whole block, ⌘C copies.
//!
//! Text blocks wrap their `StyledText` in [`Selectable`]. Each frame [`root`] (painted
//! first) clears the block registry; blocks register themselves as they paint, so a block's
//! place in its scope is its paint order. Selection positions are (block, byte index).

use std::cell::RefCell;
use std::ops::Range;

use gpui::{
    canvas, fill, point, AnyElement, App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, GlobalElementId,
    Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, Styled, StyledText, TextLayout, Window,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Pos {
    block: usize,
    ix: usize,
}

struct Block {
    scope: SharedString,
    layout: TextLayout,
}

#[derive(Default)]
struct State {
    scope: SharedString,
    anchor: Option<Pos>,
    head: Option<Pos>,
    dragging: bool,
    /// Blocks painted this frame, in paint order.
    blocks: Vec<Block>,
}

impl State {
    /// (first, last) positions, ordered; `None` when nothing (or an empty range) is selected.
    fn bounds(&self) -> Option<(Pos, Pos)> {
        let (a, h) = (self.anchor?, self.head?);
        (a != h).then(|| (a.min(h), a.max(h)))
    }

    /// Blocks of `scope`, with their index in it.
    fn scope_blocks<'a>(&'a self, scope: &'a SharedString) -> impl Iterator<Item = (usize, &'a Block)> + 'a {
        self.blocks.iter().filter(move |b| &b.scope == scope).enumerate()
    }

    /// The position under the mouse among the blocks of the selection's scope.
    fn position_at(&self, p: Point<Pixels>) -> Option<Pos> {
        let mut last = None;
        for (i, b) in self.scope_blocks(&self.scope) {
            let bounds = b.layout.bounds();
            if p.y < bounds.top() {
                // Above this block (in a gap, or above the first one).
                return Some(Pos { block: i, ix: 0 });
            }
            if p.y < bounds.bottom() {
                if p.x < bounds.left() || p.x >= bounds.right() {
                    // Beside a block laid out in columns (list marker, table cell): pick the
                    // nearer end of the line.
                    let ix = b.layout.index_for_position(point(p.x.max(bounds.left()), p.y)).unwrap_or_else(|i| i);
                    return Some(Pos { block: i, ix: ix.min(b.layout.len()) });
                }
                let ix = b.layout.index_for_position(p).unwrap_or_else(|i| i);
                return Some(Pos { block: i, ix: ix.min(b.layout.len()) });
            }
            last = Some(Pos { block: i, ix: b.layout.len() });
        }
        last
    }

    fn text(&self) -> Option<String> {
        let (a, b) = self.bounds()?;
        let mut out = vec![];
        for (i, block) in self.scope_blocks(&self.scope) {
            if i < a.block || i > b.block {
                continue;
            }
            let text = block.layout.text();
            let s = if i == a.block { a.ix.min(text.len()) } else { 0 };
            let e = if i == b.block { b.ix.min(text.len()) } else { text.len() };
            out.push(text.get(s..e.max(s)).unwrap_or_default().to_string());
        }
        Some(out.join("\n"))
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// The selected text, if any.
pub fn selected_text() -> Option<String> {
    STATE.with(|s| s.borrow().text()).filter(|t| !t.is_empty())
}

/// Copies the selection to the clipboard; false when nothing is selected.
pub fn copy(cx: &mut App) -> bool {
    match selected_text() {
        Some(text) => {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            true
        }
        None => false,
    }
}

/// Paint this first in the window: it starts the frame's registry and handles the mouse
/// events that aren't on a particular block.
pub fn root() -> impl IntoElement {
    canvas(
        |_, _, _| STATE.with(|s| s.borrow_mut().blocks.clear()),
        |_, _, window, _| {
            // Any click starts over; a click on text then sets the new anchor (bubble phase).
            window.on_mouse_event(|ev: &MouseDownEvent, phase, window, _| {
                if phase == DispatchPhase::Capture && ev.button == MouseButton::Left {
                    let had = STATE.with(|s| {
                        let mut s = s.borrow_mut();
                        let had = s.bounds().is_some();
                        (s.anchor, s.head, s.dragging) = (None, None, false);
                        had
                    });
                    if had {
                        window.refresh();
                    }
                }
            });
            window.on_mouse_event(|ev: &MouseMoveEvent, phase, window, _| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let changed = STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    if !s.dragging {
                        return false;
                    }
                    if ev.pressed_button != Some(MouseButton::Left) {
                        s.dragging = false;
                        return false;
                    }
                    let head = s.position_at(ev.position);
                    let changed = head.is_some() && head != s.head;
                    if changed {
                        s.head = head;
                    }
                    changed
                });
                if changed {
                    window.refresh();
                }
            });
            window.on_mouse_event(|_: &MouseUpEvent, phase, _, _| {
                if phase == DispatchPhase::Capture {
                    STATE.with(|s| s.borrow_mut().dragging = false);
                }
            });
        },
    )
    .absolute()
    .size_0()
}

/// Text that can be selected with the mouse. `child` renders `layout` (a `StyledText`, or
/// an `InteractiveText` around it).
pub struct Selectable {
    scope: SharedString,
    layout: TextLayout,
    child: AnyElement,
    color: Hsla,
}

pub fn selectable(scope: impl Into<SharedString>, text: StyledText, color: Hsla) -> Selectable {
    let layout = text.layout().clone();
    Selectable { scope: scope.into(), layout, child: text.into_any_element(), color }
}

impl Selectable {
    /// For text wrapped in another element (e.g. `InteractiveText` for links).
    pub fn wrapping(scope: impl Into<SharedString>, layout: TextLayout, child: AnyElement, color: Hsla) -> Self {
        Selectable { scope: scope.into(), layout, child, color }
    }
}

impl IntoElement for Selectable {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Selectable {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.child.prepaint(window, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let scope = self.scope.clone();
        let layout = self.layout.clone();
        let (index, selected) = STATE.with(|s| {
            let mut s = s.borrow_mut();
            let index = s.blocks.iter().filter(|b| b.scope == scope).count();
            s.blocks.push(Block { scope: scope.clone(), layout: layout.clone() });
            let selected = match s.bounds() {
                Some((a, b)) if s.scope == scope && (a.block..=b.block).contains(&index) => {
                    let s0 = if index == a.block { a.ix } else { 0 };
                    let e0 = if index == b.block { b.ix } else { layout.len() };
                    Some(s0..e0)
                }
                _ => None,
            };
            (index, selected)
        });
        if let Some(range) = selected {
            // Rows before the last run to the block's right edge (the newline).
            let whole_last = range.end >= layout.len() && STATE.with(|s| s.borrow().bounds().is_some_and(|(_, b)| b.block > index));
            for quad in highlight(&layout, range, whole_last) {
                window.paint_quad(fill(quad, self.color));
            }
        }
        self.child.paint(window, cx);
        window.set_cursor_style(CursorStyle::IBeam, hitbox);

        let hitbox = hitbox.clone();
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, _| {
            if phase != DispatchPhase::Bubble || ev.button != MouseButton::Left || !hitbox.is_hovered(window) {
                return;
            }
            let ix = layout.index_for_position(ev.position).unwrap_or_else(|i| i).min(layout.len());
            let text = layout.text();
            let (start, end) = match ev.click_count {
                1 => (ix, ix),
                2 => word_at(&text, ix),
                _ => (0, text.len()),
            };
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.scope = scope.clone();
                s.anchor = Some(Pos { block: index, ix: start });
                s.head = Some(Pos { block: index, ix: end });
                s.dragging = ev.click_count == 1;
            });
            window.refresh();
        });
    }
}

/// The word (or run of punctuation/space) around byte `ix`.
fn word_at(text: &str, ix: usize) -> (usize, usize) {
    let class = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
    let ix = ix.min(text.len());
    let Some(c) = text[ix..].chars().next().or_else(|| text[..ix].chars().next_back()) else { return (ix, ix) };
    let k = class(c);
    let start = text[..ix].char_indices().rev().take_while(|(_, c)| class(*c) == k).last().map_or(ix, |(i, _)| i);
    let end = text[ix..].char_indices().find(|(_, c)| class(*c) != k).map_or(text.len(), |(i, _)| ix + i);
    (start, end)
}

/// Rectangles covering `range` of a laid out (possibly wrapped) text.
fn highlight(layout: &TextLayout, range: Range<usize>, to_edge: bool) -> Vec<Bounds<Pixels>> {
    let bounds = layout.bounds();
    let lh = layout.line_height();
    let (Some(a), Some(b)) = (layout.position_for_index(range.start), layout.position_for_index(range.end)) else {
        return vec![];
    };
    let mut out = vec![];
    if a.y == b.y {
        let right = if to_edge { bounds.right() } else { b.x };
        if right > a.x {
            out.push(Bounds::from_corners(a, point(right, a.y + lh)));
        }
    } else {
        out.push(Bounds::from_corners(a, point(bounds.right(), a.y + lh)));
        if b.y > a.y + lh {
            out.push(Bounds::from_corners(point(bounds.left(), a.y + lh), point(bounds.right(), b.y)));
        }
        let right = if to_edge { bounds.right() } else { b.x };
        if right > bounds.left() {
            out.push(Bounds::from_corners(point(bounds.left(), b.y), point(right, b.y + lh)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::word_at;

    #[test]
    fn words() {
        let t = "let foo_bar = 1;";
        assert_eq!(word_at(t, 5), (4, 11));
        assert_eq!(word_at(t, 4), (4, 11));
        assert_eq!(word_at(t, 3), (3, 4));
        assert_eq!(word_at(t, t.len()), (15, 16));
    }
}
