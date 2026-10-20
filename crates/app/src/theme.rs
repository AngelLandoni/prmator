//! Light and dark color themes, installed as a GPUI global.

use gpui::{rgb, App, Global, Hsla, SharedString, WindowAppearance};
use prmator_syntax::Highlight;
use serde::{Deserialize, Serialize};

/// What the user picked; `System` follows the macOS appearance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn next(self) -> Self {
        match self {
            ThemeMode::System => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::System,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            ThemeMode::System => "◐",
            ThemeMode::Light => "☀",
            ThemeMode::Dark => "☾",
        }
    }

    /// Whether this mode shows a dark theme under the given system appearance.
    pub fn is_dark(self, appearance: WindowAppearance) -> bool {
        match self {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark),
        }
    }
}

#[derive(Clone)]
pub struct Theme {
    pub is_dark: bool,
    /// Font family for code.
    pub mono: SharedString,
    // Typography from the user's settings (see `with_settings`).
    pub ui_font: SharedString,
    /// Interface base size; the window's rem size.
    pub ui_size: f32,
    pub code_size: f32,
    /// Height of a diff row.
    pub row_h: f32,
    // surfaces
    pub bg: Hsla,
    pub sidebar: Hsla,
    pub title_bar: Hsla,
    pub panel: Hsla,
    pub raised: Hsla,
    pub hover: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub input_bg: Hsla,
    pub code_bg: Hsla,
    pub shadow: Hsla,
    // text
    pub text: Hsla,
    pub muted: Hsla,
    pub dim: Hsla,
    pub accent: Hsla,
    pub accent_fg: Hsla,
    pub accent_soft: Hsla,
    pub warn: Hsla,
    pub tool: Hsla,
    // diff
    pub add_bg: Hsla,
    pub add_word: Hsla,
    pub add_fg: Hsla,
    pub add_gutter: Hsla,
    pub del_bg: Hsla,
    pub del_word: Hsla,
    pub del_fg: Hsla,
    pub del_gutter: Hsla,
    pub hunk_bg: Hsla,
    pub hunk_fg: Hsla,
    pub filler: Hsla,
    pub selected: Hsla,
    pub line_no: Hsla,
    // syntax
    pub syn_keyword: Hsla,
    pub syn_function: Hsla,
    pub syn_type: Hsla,
    pub syn_string: Hsla,
    pub syn_number: Hsla,
    pub syn_comment: Hsla,
    pub syn_constant: Hsla,
    pub syn_variable: Hsla,
    pub syn_property: Hsla,
    pub syn_operator: Hsla,
    pub syn_punctuation: Hsla,
    pub syn_attribute: Hsla,
    pub syn_tag: Hsla,
    pub syn_module: Hsla,
    pub syn_macro: Hsla,
    pub syn_label: Hsla,
    pub syn_escape: Hsla,
}

impl Global for Theme {}

pub fn theme(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex: u32, alpha: f32) -> Hsla {
    c(hex).opacity(alpha)
}

impl Theme {
    pub fn dark() -> Self {
        Theme {
            is_dark: true,
            mono: "Menlo".into(),
            ui_font: ".SystemUIFont".into(),
            ui_size: 16.,
            code_size: 12.5,
            row_h: 22.,
            bg: c(0x15161b),
            sidebar: c(0x191a20),
            title_bar: c(0x191a20),
            panel: c(0x1b1c23),
            raised: c(0x24262e),
            hover: c(0x2b2d37),
            border: c(0x2a2c35),
            border_strong: c(0x3a3d49),
            input_bg: c(0x22242c),
            code_bg: c(0x111217),
            shadow: ca(0x000000, 0.5),
            text: c(0xe4e6ee),
            muted: c(0x9a9fb3),
            dim: c(0x636879),
            accent: c(0x7aa2f7),
            accent_fg: c(0x0f111a),
            accent_soft: ca(0x7aa2f7, 0.16),
            warn: c(0xe0af68),
            tool: c(0x7dcfff),
            add_bg: ca(0x2ea043, 0.15),
            add_word: ca(0x2ea043, 0.40),
            add_fg: c(0x8fd18a),
            add_gutter: ca(0x2ea043, 0.24),
            del_bg: ca(0xf85149, 0.13),
            del_word: ca(0xf85149, 0.38),
            del_fg: c(0xf7768e),
            del_gutter: ca(0xf85149, 0.22),
            hunk_bg: ca(0x7aa2f7, 0.08),
            hunk_fg: c(0x8190bd),
            filler: c(0x18191e),
            selected: ca(0x7aa2f7, 0.28),
            line_no: c(0x4f5465),
            syn_keyword: c(0xbb9af7),
            syn_function: c(0x7aa2f7),
            syn_type: c(0x2ac3de),
            syn_string: c(0x9ece6a),
            syn_number: c(0xff9e64),
            syn_comment: c(0x5f6782),
            syn_constant: c(0xff9e64),
            syn_variable: c(0xd5d9ec),
            syn_property: c(0x73daca),
            syn_operator: c(0x89ddff),
            syn_punctuation: c(0x9099b8),
            syn_attribute: c(0xe0af68),
            syn_tag: c(0xf7768e),
            syn_module: c(0x2ac3de),
            syn_macro: c(0x7dcfff),
            syn_label: c(0xff9e64),
            syn_escape: c(0x89ddff),
        }
    }

    pub fn light() -> Self {
        Theme {
            is_dark: false,
            mono: "Menlo".into(),
            ui_font: ".SystemUIFont".into(),
            ui_size: 16.,
            code_size: 12.5,
            row_h: 22.,
            bg: c(0xffffff),
            sidebar: c(0xf6f7f9),
            title_bar: c(0xf6f7f9),
            panel: c(0xf9fafb),
            raised: c(0xeef0f3),
            hover: c(0xe6e9ee),
            border: c(0xe1e4e8),
            border_strong: c(0xcdd2d9),
            input_bg: c(0xffffff),
            code_bg: c(0xf3f4f6),
            shadow: ca(0x1f2328, 0.18),
            text: c(0x1f2328),
            muted: c(0x59636e),
            dim: c(0x8b949e),
            accent: c(0x2f6feb),
            accent_fg: c(0xffffff),
            accent_soft: ca(0x2f6feb, 0.10),
            warn: c(0x9a6700),
            tool: c(0x0969da),
            add_bg: c(0xe9f8ec),
            add_word: c(0xabf2bc),
            add_fg: c(0x1a7f37),
            add_gutter: c(0xd1f3d9),
            del_bg: c(0xffeef0),
            del_word: c(0xffc1c0),
            del_fg: c(0xcf222e),
            del_gutter: c(0xffd7d5),
            hunk_bg: c(0xeef4ff),
            hunk_fg: c(0x57606a),
            filler: c(0xf6f8fa),
            selected: ca(0x2f6feb, 0.18),
            line_no: c(0x9aa3ad),
            syn_keyword: c(0xcf222e),
            syn_function: c(0x8250df),
            syn_type: c(0x953800),
            syn_string: c(0x0a3069),
            syn_number: c(0x0550ae),
            syn_comment: c(0x6e7781),
            syn_constant: c(0x0550ae),
            syn_variable: c(0x1f2328),
            syn_property: c(0x0550ae),
            syn_operator: c(0xcf222e),
            syn_punctuation: c(0x57606a),
            syn_attribute: c(0x116329),
            syn_tag: c(0x116329),
            syn_module: c(0x953800),
            syn_macro: c(0x8250df),
            syn_label: c(0x0550ae),
            syn_escape: c(0x0a3069),
        }
    }

    /// github.com's light theme (Primer "light" colors and the PR diff palette).
    pub fn github_light() -> Self {
        Theme {
            is_dark: false,
            mono: if sf_mono_loaded() { SF_MONO.into() } else { "Menlo".into() },
            ui_font: ".SystemUIFont".into(),
            ui_size: 16.,
            code_size: 12.5,
            row_h: 22.,
            bg: c(0xffffff),
            sidebar: c(0xffffff),
            title_bar: c(0xf6f8fa),
            panel: c(0xffffff),
            raised: c(0xf6f8fa),
            hover: c(0xeff2f5),
            border: c(0xd1d9e0),
            border_strong: c(0xd1d9e0),
            input_bg: c(0xffffff),
            code_bg: c(0xf6f8fa),
            shadow: ca(0x25292e, 0.16),
            text: c(0x1f2328),
            muted: c(0x59636e),
            dim: c(0x818b98),
            accent: c(0x0969da),
            accent_fg: c(0xffffff),
            accent_soft: c(0xddf4ff),
            warn: c(0x9a6700),
            tool: c(0x0969da),
            add_bg: c(0xdafbe1),
            add_word: c(0xaceebb),
            add_fg: c(0x1a7f37),
            add_gutter: c(0xd1f8d9),
            del_bg: c(0xffebe9),
            del_word: ca(0xff8182, 0.4),
            del_fg: c(0xd1242f),
            del_gutter: c(0xffcecb),
            hunk_bg: c(0xddf4ff),
            hunk_fg: c(0x59636e),
            filler: c(0xf6f8fa),
            // GitHub highlights selected lines in yellow.
            selected: c(0xfff8c5),
            line_no: c(0x59636e),
            // "prettylights" syntax colors.
            syn_keyword: c(0xcf222e),
            syn_function: c(0x6639ba),
            syn_type: c(0x6639ba),
            syn_string: c(0x0a3069),
            syn_number: c(0x0550ae),
            syn_comment: c(0x59636e),
            syn_constant: c(0x0550ae),
            syn_variable: c(0x1f2328),
            syn_property: c(0x1f2328),
            syn_operator: c(0xcf222e),
            syn_punctuation: c(0x1f2328),
            syn_attribute: c(0x0550ae),
            syn_tag: c(0x116329),
            syn_module: c(0x1f2328),
            syn_macro: c(0x6639ba),
            syn_label: c(0x0550ae),
            syn_escape: c(0x116329),
        }
    }

    /// Applies the user's fonts and sizes.
    pub fn with_settings(mut self, s: &crate::settings::Settings) -> Self {
        self.mono = s.code_font_family.clone().into();
        self.ui_font = s.ui_font_family.clone().into();
        self.ui_size = s.ui_font_size;
        self.code_size = s.code_font_size;
        self.row_h = s.row_height();
        self
    }

    /// Width of a line-number column for the code size.
    pub fn line_no_width(&self) -> f32 {
        (self.code_size * 3.4).max(40.)
    }

    pub fn syntax(&self, h: Highlight) -> Hsla {
        match h {
            Highlight::Keyword => self.syn_keyword,
            Highlight::Function => self.syn_function,
            Highlight::Type => self.syn_type,
            Highlight::String => self.syn_string,
            Highlight::Escape => self.syn_escape,
            Highlight::Number => self.syn_number,
            Highlight::Comment => self.syn_comment,
            Highlight::Constant => self.syn_constant,
            Highlight::Variable => self.syn_variable,
            Highlight::Property => self.syn_property,
            Highlight::Operator => self.syn_operator,
            Highlight::Punctuation => self.syn_punctuation,
            Highlight::Attribute => self.syn_attribute,
            Highlight::Tag => self.syn_tag,
            Highlight::Module => self.syn_module,
            Highlight::Macro => self.syn_macro,
            Highlight::Label => self.syn_label,
        }
    }

    /// Stable color for a user avatar.
    pub fn avatar(&self, login: &str) -> Hsla {
        let hash = login.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
        let hue = (hash % 360) as f32 / 360.;
        if self.is_dark {
            gpui::hsla(hue, 0.45, 0.42, 1.)
        } else {
            gpui::hsla(hue, 0.55, 0.55, 1.)
        }
    }
}

/// SF Mono, the font github.com uses for code on macOS. It's a private system font, so it
/// is loaded from its file (see [`load_sf_mono`]) under this family name.
pub const SF_MONO: &str = ".SF NS Mono";

static SF_MONO_LOADED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

pub fn sf_mono_loaded() -> bool {
    SF_MONO_LOADED.get().copied().unwrap_or(false)
}

/// Registers SF Mono with GPUI's text system. Call before themes are built.
pub fn load_sf_mono(cx: &App) {
    let fonts: Vec<std::borrow::Cow<'static, [u8]>> = ["SFNSMono.ttf", "SFNSMonoItalic.ttf"]
        .iter()
        .filter_map(|f| std::fs::read(format!("/System/Library/Fonts/{f}")).ok())
        .map(std::borrow::Cow::Owned)
        .collect();
    let ok = !fonts.is_empty() && cx.text_system().add_fonts(fonts).is_ok();
    let _ = SF_MONO_LOADED.set(ok);
}
