//! Persistent settings and per-PR review progress, so a review can be resumed later.
//! Stored as JSON in `~/Library/Application Support/PRmator/state.json`.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use protocol::{ChangeContext, ReviewPlan};
use serde::{Deserialize, Serialize};

use crate::settings::Settings;
use crate::theme::ThemeMode;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffMode {
    #[default]
    Unified,
    Split,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// Fonts, sizes and theme customized in PRmator; `None` follows Zed's settings.
    pub settings: Option<Settings>,
    /// Before the settings window existed, the theme was stored here. Read once to keep
    /// the user's pick, never written back.
    #[serde(skip_serializing)]
    pub theme: Option<ThemeMode>,
    #[serde(skip_serializing)]
    pub light_theme: Option<String>,
    #[serde(skip_serializing)]
    pub dark_theme: Option<String>,
    pub diff_mode: DiffMode,
    /// Where the reviewer put the comment box and how big they made it; `None` opens it
    /// next to the line at its default size.
    pub comment_box: Option<BoxGeometry>,
    /// Stacked PRs folded in the PR list, by bottom PR (`owner/repo#123`).
    pub collapsed_stacks: BTreeSet<String>,
    /// Keyed by `owner/repo#123`.
    pub prs: HashMap<String, PrProgress>,
}

/// A floating box's bounds, in window pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoxGeometry {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PrProgress {
    /// Paths of files marked as viewed.
    pub viewed: BTreeSet<String>,
    pub plan: Option<SavedPlan>,
    /// Finished Claude conversations (the review log), oldest first.
    pub log: Vec<SavedConversation>,
    /// The PR retold as virtual commits.
    pub story: Option<crate::story::SavedStory>,
    /// Review comments not posted yet.
    pub drafts: Vec<crate::comments::DraftComment>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SavedConversation {
    pub id: u64,
    pub context: Option<ChangeContext>,
    pub turns: Vec<SavedTurn>,
    /// Unix seconds.
    pub created: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SavedTurn {
    pub question: String,
    pub answer: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SavedPlan {
    pub plan: ReviewPlan,
    /// Indices of the steps already reviewed.
    #[serde(default)]
    pub done: BTreeSet<usize>,
    /// Head commit the plan was made for; the PR may have moved since.
    #[serde(default)]
    pub head_sha: String,
}

impl SavedPlan {
    /// The step to continue with: the first one not done yet.
    pub fn next_step(&self) -> Option<usize> {
        (0..self.plan.steps.len()).find(|i| !self.done.contains(i))
    }
}

pub fn pr_key(repo: &str, number: u64) -> String {
    format!("{repo}#{number}")
}

fn path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/PRmator/state.json"))
}

impl Default for Store {
    fn default() -> Self {
        Store {
            settings: None,
            theme: None,
            light_theme: None,
            dark_theme: None,
            diff_mode: DiffMode::default(),
            comment_box: None,
            collapsed_stacks: BTreeSet::new(),
            prs: HashMap::new(),
        }
    }
}

impl Store {
    /// The settings in effect: PRmator's own if customized, otherwise Zed's. A theme
    /// picked in an older PRmator version is kept (and then counts as customized).
    pub fn effective_settings(&mut self) -> Settings {
        if let Some(s) = &self.settings {
            return s.clone().clamp();
        }
        let mut s = Settings::from_zed();
        let light = self.light_theme.take().filter(|t| t != crate::themes::DEFAULT_LIGHT);
        let dark = self.dark_theme.take().filter(|t| t != crate::themes::DEFAULT_DARK);
        if light.is_some() || dark.is_some() {
            s.light_theme = light.unwrap_or(s.light_theme);
            s.dark_theme = dark.unwrap_or(s.dark_theme);
            s.theme_mode = self.theme.unwrap_or(s.theme_mode);
            self.settings = Some(s.clone());
            self.save();
        }
        s
    }

    pub fn load() -> Store {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Best effort; a failed save only loses progress, so it is logged and ignored.
    pub fn save(&self) {
        let Some(path) = path() else { return };
        let result = (|| -> anyhow::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
            std::fs::rename(tmp, &path)?;
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("could not save state: {e:#}");
        }
    }

    pub fn progress(&mut self, key: &str) -> &mut PrProgress {
        self.prs.entry(key.to_string()).or_default()
    }
}
