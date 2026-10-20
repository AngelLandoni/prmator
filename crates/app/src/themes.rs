//! Theme registry: PRmator's own themes, Zed's bundled themes (One, Ayu, Gruvbox), and any
//! Zed themes installed on this machine (`~/.config/zed/themes` and theme extensions).
//!
//! Zed themes are mapped onto [`Theme`]: UI surfaces, diff colors (`created` / `deleted`),
//! and syntax colors. Keys a theme doesn't define fall back to PRmator's light/dark base.

use std::path::PathBuf;

use gpui::{Global, Hsla, Rgba};
use serde_json::Value;

use crate::theme::Theme;

const BUNDLED: &[(&str, &str)] = &[
    ("Zed", include_str!("../assets/themes/one.json")),
    ("Zed", include_str!("../assets/themes/ayu.json")),
    ("Zed", include_str!("../assets/themes/gruvbox.json")),
];

pub const DEFAULT_DARK: &str = "One Dark";
pub const DEFAULT_LIGHT: &str = "One Light";

#[derive(Clone)]
pub struct ThemeEntry {
    pub name: String,
    pub dark: bool,
    /// Where it came from: "PRmator", "Zed", "user" or the extension's name.
    pub source: String,
    pub theme: Theme,
}

pub struct Themes(pub Vec<ThemeEntry>);

impl Global for Themes {}

impl Themes {
    pub fn load() -> Themes {
        let mut entries = vec![
            ThemeEntry { name: "PRmator Dark".into(), dark: true, source: "PRmator".into(), theme: Theme::dark() },
            ThemeEntry { name: "PRmator Light".into(), dark: false, source: "PRmator".into(), theme: Theme::light() },
            ThemeEntry { name: "GitHub Light".into(), dark: false, source: "github.com".into(), theme: Theme::github_light() },
        ];
        for (source, json) in BUNDLED {
            entries.extend(parse_family(json, source));
        }
        for (source, path) in installed_theme_files() {
            match std::fs::read_to_string(&path) {
                Ok(json) => entries.extend(parse_family(&json, &source)),
                Err(e) => eprintln!("theme {}: {e}", path.display()),
            }
        }
        // First one wins on duplicate names.
        let mut seen = std::collections::HashSet::new();
        entries.retain(|e| seen.insert(e.name.clone()));
        Themes(entries)
    }

    pub fn get(&self, name: &str) -> Option<&ThemeEntry> {
        self.0.iter().find(|e| e.name == name)
    }
}

/// Theme JSON files from the user's Zed setup: `(source label, path)`.
fn installed_theme_files() -> Vec<(String, PathBuf)> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return vec![] };
    let mut files = vec![];
    let json_files = |dir: PathBuf| -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        out.sort();
        out
    };
    for path in json_files(home.join(".config/zed/themes")) {
        files.push(("user".to_string(), path));
    }
    let extensions = home.join("Library/Application Support/Zed/extensions/installed");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&extensions).into_iter().flatten().flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        for path in json_files(dir.join("themes")) {
            files.push((name.clone(), path));
        }
    }
    files
}

fn parse_family(json: &str, source: &str) -> Vec<ThemeEntry> {
    let family: Value = match serde_json::from_str(json).or_else(|_| serde_json::from_str(&strip_jsonc(json))) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("theme family from {source}: {e}");
            return vec![];
        }
    };
    family["themes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let name = t["name"].as_str()?.to_string();
            let dark = t["appearance"].as_str() != Some("light");
            Some(ThemeEntry { theme: from_zed(&t["style"], dark), name, dark, source: source.to_string() })
        })
        .collect()
}

/// Removes `//` and `/* */` comments and trailing commas (Zed theme files are JSONC).
pub fn strip_jsonc(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            (',', _) => {
                // Drop the comma if the next non-space character closes a container.
                let rest: String = chars.clone().take_while(|c| c.is_whitespace()).collect();
                let next = chars.clone().nth(rest.chars().count());
                if !matches!(next, Some('}') | Some(']')) {
                    out.push(',');
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa`.
fn parse_color(s: &str) -> Option<Hsla> {
    let hex = s.trim().strip_prefix('#')?;
    let expand = |h: &str| h.chars().flat_map(|c| [c, c]).collect::<String>();
    let hex = match hex.len() {
        3 | 4 => expand(hex),
        6 | 8 => hex.to_string(),
        _ => return None,
    };
    let v = u32::from_str_radix(&hex, 16).ok()?;
    let rgba = if hex.len() == 6 { (v << 8) | 0xff } else { v };
    let c = Rgba {
        r: ((rgba >> 24) & 0xff) as f32 / 255.,
        g: ((rgba >> 16) & 0xff) as f32 / 255.,
        b: ((rgba >> 8) & 0xff) as f32 / 255.,
        a: (rgba & 0xff) as f32 / 255.,
    };
    Some(c.into())
}

fn from_zed(style: &Value, dark: bool) -> Theme {
    let mut t = if dark { Theme::dark() } else { Theme::light() };
    let get = |key: &str| style.get(key).and_then(Value::as_str).and_then(parse_color);
    let first = |keys: &[&str]| keys.iter().find_map(|k| get(k));
    let syntax = |key: &str| style["syntax"].get(key).and_then(|s| s["color"].as_str()).and_then(parse_color);
    let player = |key: &str| style["players"].get(0).and_then(|p| p[key].as_str()).and_then(parse_color);

    let set = |slot: &mut Hsla, value: Option<Hsla>| {
        if let Some(v) = value {
            *slot = v;
        }
    };

    t.is_dark = dark;
    set(&mut t.bg, first(&["editor.background", "background"]));
    set(&mut t.sidebar, first(&["panel.background", "surface.background", "background"]));
    set(&mut t.panel, first(&["panel.background", "surface.background", "background"]));
    set(&mut t.title_bar, first(&["title_bar.background", "background"]));
    set(&mut t.raised, first(&["element.background", "elevated_surface.background", "surface.background"]));
    set(&mut t.hover, first(&["element.hover", "ghost_element.hover"]));
    set(&mut t.border, first(&["border.variant", "border"]));
    set(&mut t.border_strong, first(&["border", "border.variant"]));
    set(&mut t.input_bg, first(&["editor.background", "surface.background"]));
    set(&mut t.code_bg, first(&["editor.background", "background"]));
    set(&mut t.text, first(&["text", "editor.foreground"]));
    set(&mut t.muted, first(&["text.muted", "text"]));
    set(&mut t.dim, first(&["text.placeholder", "text.disabled", "text.muted"]));
    set(&mut t.accent, first(&["text.accent", "icon.accent", "border.focused"]).or(player("cursor")));
    set(&mut t.accent_fg, first(&["editor.background", "background"]));
    t.accent_soft = t.accent.opacity(0.14);
    set(&mut t.warn, first(&["warning", "modified"]));
    set(&mut t.tool, first(&["info", "text.accent"]));
    set(&mut t.line_no, first(&["editor.line_number", "text.muted"]));
    set(&mut t.filler, first(&["editor.gutter.background", "surface.background"]));
    // Translucent surfaces (e.g. "blurred" themes) rely on Zed's window blur: make them
    // opaque here so text stays readable.
    for surface in [&mut t.bg, &mut t.sidebar, &mut t.panel, &mut t.title_bar, &mut t.input_bg, &mut t.code_bg] {
        surface.a = 1.;
    }
    // A gutter color equal to the background makes empty split cells invisible: dim it.
    if t.filler == t.bg {
        t.filler = t.raised.opacity(0.5);
    }
    t.hunk_bg = t.accent.opacity(0.08);
    t.hunk_fg = t.muted;
    t.selected = player("selection").unwrap_or(t.accent.opacity(0.25));

    if let Some(added) = first(&["created", "version_control.added", "success"]) {
        t.add_fg = added;
        t.add_bg = added.opacity(0.13);
        t.add_gutter = added.opacity(0.22);
        t.add_word = first(&["version_control.word_added"]).unwrap_or(added.opacity(0.35));
    }
    if let Some(deleted) = first(&["deleted", "version_control.deleted", "error"]) {
        t.del_fg = deleted;
        t.del_bg = deleted.opacity(0.12);
        t.del_gutter = deleted.opacity(0.22);
        t.del_word = first(&["version_control.word_deleted"]).unwrap_or(deleted.opacity(0.35));
    }

    set(&mut t.syn_keyword, syntax("keyword"));
    set(&mut t.syn_function, syntax("function"));
    set(&mut t.syn_type, syntax("type"));
    set(&mut t.syn_string, syntax("string"));
    set(&mut t.syn_number, syntax("number"));
    set(&mut t.syn_comment, syntax("comment"));
    set(&mut t.syn_constant, syntax("constant").or(syntax("number")));
    set(&mut t.syn_variable, syntax("variable").or(Some(t.text)));
    set(&mut t.syn_property, syntax("property"));
    set(&mut t.syn_operator, syntax("operator"));
    set(&mut t.syn_punctuation, syntax("punctuation"));
    set(&mut t.syn_attribute, syntax("attribute"));
    set(&mut t.syn_tag, syntax("tag"));
    set(&mut t.syn_module, syntax("namespace").or(syntax("type")));
    set(&mut t.syn_macro, syntax("preproc").or(syntax("function")));
    set(&mut t.syn_label, syntax("label").or(syntax("constant")));
    set(&mut t.syn_escape, syntax("string.escape").or(syntax("string")));
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_themes_load() {
        let mut names = vec![];
        for (source, json) in BUNDLED {
            names.extend(parse_family(json, source).into_iter().map(|e| (e.name, e.dark)));
        }
        for expected in ["One Dark", "One Light", "Ayu Dark", "Ayu Light", "Ayu Mirage", "Gruvbox Dark", "Gruvbox Light Soft"] {
            assert!(names.iter().any(|(n, _)| n == expected), "missing {expected}");
        }
        assert!(names.contains(&("One Light".into(), false)));
        assert_eq!(names.len(), 11);
    }

    #[test]
    fn one_dark_colors_map() {
        let one = parse_family(BUNDLED[0].1, "Zed").into_iter().find(|e| e.name == "One Dark").unwrap();
        assert_eq!(one.theme.bg, parse_color("#282c33ff").unwrap());
        assert_eq!(one.theme.syn_keyword, parse_color("#b477cfff").unwrap());
    }

    #[test]
    fn jsonc_is_accepted() {
        let src = r##"{ // comment
            "name": "x", /* block */ "themes": [ { "name": "A // not a comment", "appearance": "light", "style": { "text": "#123" }, }, ],
        }"##;
        let themes = parse_family(src, "user");
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name, "A // not a comment");
        assert!(!themes[0].dark);
        assert_eq!(themes[0].theme.text, parse_color("#112233").unwrap());
    }
}
