//! User settings: fonts, sizes and theme. The code font and the theme default to Zed's
//! settings (`~/.config/zed/settings.json`), so code looks like in the user's editor; the
//! interface keeps the system font unless changed in the settings window.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::theme::ThemeMode;
use crate::themes::{strip_jsonc, DEFAULT_DARK, DEFAULT_LIGHT};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme_mode: ThemeMode,
    pub light_theme: String,
    pub dark_theme: String,
    pub ui_font_family: String,
    /// Base size of the interface (like Zed's `ui_font_size`; everything scales with it).
    pub ui_font_size: f32,
    pub code_font_family: String,
    pub code_font_size: f32,
    /// Code line height as a multiple of the code font size.
    pub code_line_height: f32,
}

pub const SYSTEM_UI_FONT: &str = ".SystemUIFont";
pub const MIN_FONT_SIZE: f32 = 9.;
pub const MAX_FONT_SIZE: f32 = 32.;

impl Default for Settings {
    /// PRmator's own defaults, used for anything Zed doesn't specify.
    fn default() -> Self {
        Settings {
            theme_mode: ThemeMode::System,
            light_theme: DEFAULT_LIGHT.into(),
            dark_theme: DEFAULT_DARK.into(),
            ui_font_family: SYSTEM_UI_FONT.into(),
            ui_font_size: 16.,
            code_font_family: "Menlo".into(),
            code_font_size: 13.,
            code_line_height: 1.45,
        }
    }
}

impl Settings {
    /// Zed's settings mapped onto PRmator's, or the defaults when Zed isn't configured.
    pub fn from_zed() -> Settings {
        match zed_settings_path().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(src) => Self::from_zed_json(&src),
            None => Settings::default(),
        }
    }

    pub fn from_zed_json(src: &str) -> Settings {
        let mut s = Settings::default();
        let Ok(zed) = serde_json::from_str::<Value>(src).or_else(|_| serde_json::from_str(&strip_jsonc(src))) else {
            return s;
        };
        let string = |key: &str| zed[key].as_str().map(String::from);
        let number = |key: &str| zed[key].as_f64().map(|n| n as f32);

        // Zed's own fonts (".ZedSans", ".ZedMono", "Zed Plex …") ship inside Zed only.
        // Only the code font is taken from Zed: the interface keeps the system font.
        let usable = |family: String| (!family.starts_with(".Zed") && !family.starts_with("Zed ")).then_some(family);
        if let Some(f) = string("buffer_font_family").and_then(usable) {
            s.code_font_family = f;
        }
        if let Some(n) = number("buffer_font_size") {
            s.code_font_size = n;
        }
        match &zed["buffer_line_height"] {
            Value::String(k) if k == "comfortable" => s.code_line_height = 1.618,
            Value::String(k) if k == "standard" => s.code_line_height = 1.3,
            Value::Object(o) => {
                if let Some(n) = o.get("custom").and_then(Value::as_f64) {
                    s.code_line_height = n as f32;
                }
            }
            _ => {}
        }
        match &zed["theme"] {
            // A single theme name: always that theme.
            Value::String(name) => {
                s.light_theme = name.clone();
                s.dark_theme = name.clone();
            }
            Value::Object(t) => {
                s.theme_mode = match t.get("mode").and_then(Value::as_str) {
                    Some("light") => ThemeMode::Light,
                    Some("dark") => ThemeMode::Dark,
                    _ => ThemeMode::System,
                };
                if let Some(l) = t.get("light").and_then(Value::as_str) {
                    s.light_theme = l.into();
                }
                if let Some(d) = t.get("dark").and_then(Value::as_str) {
                    s.dark_theme = d.into();
                }
            }
            _ => {}
        }
        s.clamp()
    }

    /// Keeps sizes in a usable range.
    pub fn clamp(mut self) -> Settings {
        self.ui_font_size = self.ui_font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        self.code_font_size = self.code_font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        self.code_line_height = self.code_line_height.clamp(1.0, 2.5);
        self
    }

    /// Height of one diff row.
    pub fn row_height(&self) -> f32 {
        (self.code_font_size * self.code_line_height).round().max(14.)
    }
}

pub fn zed_settings_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/zed/settings.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_zed_settings() {
        let src = r#"// Zed settings
        {
          "vim_mode": true,
          "ui_font_size": 17,
          "ui_font_family": "UbuntuMono Nerd Font Mono",
          "buffer_font_size": 17,
          "buffer_font_family": "UbuntuMono Nerd Font Mono",
          "buffer_line_height": "comfortable",
          "theme": { "mode": "system", "light": "GitHub Light Classic", "dark": "Github Plus Light" },
        }"#;
        let s = Settings::from_zed_json(src);
        // The interface keeps the system font; code uses Zed's buffer font.
        assert_eq!(s.ui_font_family, SYSTEM_UI_FONT);
        assert_eq!((s.ui_font_size, s.code_font_size), (16., 17.));
        assert_eq!(s.code_font_family, "UbuntuMono Nerd Font Mono");
        assert_eq!(s.code_line_height, 1.618);
        assert_eq!(s.theme_mode, ThemeMode::System);
        assert_eq!((s.light_theme.as_str(), s.dark_theme.as_str()), ("GitHub Light Classic", "Github Plus Light"));
    }

    #[test]
    fn zed_defaults_and_bundled_fonts_fall_back() {
        let s = Settings::from_zed_json(r#"{ "buffer_font_family": ".ZedMono", "theme": "Ayu Dark" }"#);
        assert_eq!(s.code_font_family, "Menlo");
        assert_eq!((s.light_theme.as_str(), s.dark_theme.as_str()), ("Ayu Dark", "Ayu Dark"));
        assert_eq!(Settings::from_zed_json("not json"), Settings::default());
    }
}
