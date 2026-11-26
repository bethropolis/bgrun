//! Persistent TUI preferences and the Settings popup model.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::{BaseDirs, ProjectDirs};
use serde::{Deserialize, Serialize};

use super::theme::{ChromeStyle, ThemeKind};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ViewCursor {
    #[default]
    Hidden,
    Dim,
}

impl ViewCursor {
    pub const ALL: [Self; 2] = [Self::Hidden, Self::Dim];

    pub fn label(self) -> &'static str {
        match self {
            Self::Hidden => "Hidden",
            Self::Dim => "Dim",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TuiSettings {
    pub theme: ThemeKind,
    pub chrome: ChromeStyle,
    pub sidebar_width: u16,
    pub default_pty: bool,
    pub mouse: bool,
    pub view_cursor: ViewCursor,
    pub poll_ms: u64,
}

impl Default for TuiSettings {
    fn default() -> Self {
        Self {
            theme: ThemeKind::Terminal,
            chrome: ChromeStyle::Rounded,
            sidebar_width: 28,
            default_pty: true,
            mouse: true,
            view_cursor: ViewCursor::Hidden,
            poll_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRow {
    Theme,
    Chrome,
    SidebarWidth,
    DefaultPty,
    Mouse,
    ViewCursor,
    PollMs,
}

impl SettingsRow {
    pub const ALL: [Self; 7] = [
        Self::Theme,
        Self::Chrome,
        Self::SidebarWidth,
        Self::DefaultPty,
        Self::Mouse,
        Self::ViewCursor,
        Self::PollMs,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Theme => "Theme",
            Self::Chrome => "Layout chrome",
            Self::SidebarWidth => "Sidebar width",
            Self::DefaultPty => "Default job I/O",
            Self::Mouse => "Mouse support",
            Self::ViewCursor => "Cursor in VIEW mode",
            Self::PollMs => "Poll interval",
        }
    }
}

impl TuiSettings {
    pub fn cycle(&mut self, row: SettingsRow, forwards: bool) {
        match row {
            SettingsRow::Theme => cycle_choice(&mut self.theme, &ThemeKind::ALL, forwards),
            SettingsRow::Chrome => cycle_choice(&mut self.chrome, &ChromeStyle::ALL, forwards),
            SettingsRow::SidebarWidth => {
                cycle_choice(&mut self.sidebar_width, &[24, 28, 34], forwards);
            }
            SettingsRow::DefaultPty => self.default_pty = !self.default_pty,
            SettingsRow::Mouse => self.mouse = !self.mouse,
            SettingsRow::ViewCursor => {
                cycle_choice(&mut self.view_cursor, &ViewCursor::ALL, forwards);
            }
            SettingsRow::PollMs => cycle_choice(&mut self.poll_ms, &[500, 1000, 2000], forwards),
        }
    }

    pub fn value_label(&self, row: SettingsRow) -> String {
        match row {
            SettingsRow::Theme => self.theme.label().into(),
            SettingsRow::Chrome => self.chrome.label().into(),
            SettingsRow::SidebarWidth => format!("{} cols", self.sidebar_width),
            SettingsRow::DefaultPty if self.default_pty => "PTY".into(),
            SettingsRow::DefaultPty => "Piped".into(),
            SettingsRow::Mouse if self.mouse => "Enabled".into(),
            SettingsRow::Mouse => "Disabled".into(),
            SettingsRow::ViewCursor => self.view_cursor.label().into(),
            SettingsRow::PollMs => format!("{} ms", self.poll_ms),
        }
    }
}

fn cycle_choice<T: Copy + PartialEq>(current: &mut T, choices: &[T], forwards: bool) {
    let index = choices
        .iter()
        .position(|choice| choice == current)
        .unwrap_or(0);
    let next = if forwards {
        (index + 1) % choices.len()
    } else {
        (index + choices.len() - 1) % choices.len()
    };
    *current = choices[next];
}

pub fn settings_path() -> PathBuf {
    ProjectDirs::from("", "", "bgrun").map_or_else(
        || bgrun_proto::paths::state_dir().join("tui.toml"),
        |dirs| dirs.config_dir().join("tui.toml"),
    )
}

pub fn settings_path_label() -> String {
    let path = settings_path();
    if let Some(home) = BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
        && let Ok(relative) = path.strip_prefix(home)
    {
        return Path::new("~").join(relative).display().to_string();
    }
    path.display().to_string()
}

pub async fn load() -> TuiSettings {
    load_from_path(&settings_path()).await
}

pub async fn load_from_path(path: &Path) -> TuiSettings {
    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return TuiSettings::default();
    };
    toml::from_str(&content).unwrap_or_default()
}

pub async fn save_to_path(path: &Path, settings: &TuiSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create settings directory {}", parent.display()))?;
    }
    let content = toml::to_string_pretty(settings).context("serialize TUI settings")?;
    tokio::fs::write(path, content)
        .await
        .with_context(|| format!("write TUI settings to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn settings_roundtrip_and_partial_defaults() {
        let path =
            std::env::temp_dir().join(format!("bgrun-tui-settings-{}.toml", uuid::Uuid::new_v4()));
        let settings = TuiSettings {
            theme: ThemeKind::Dark,
            chrome: ChromeStyle::Minimal,
            sidebar_width: 34,
            default_pty: false,
            mouse: false,
            view_cursor: ViewCursor::Dim,
            poll_ms: 500,
        };
        save_to_path(&path, &settings).await.unwrap();
        assert_eq!(load_from_path(&path).await, settings);
        tokio::fs::write(&path, "theme = \"dark\"\n").await.unwrap();
        let partial = load_from_path(&path).await;
        assert_eq!(partial.theme, ThemeKind::Dark);
        assert_eq!(partial.sidebar_width, 28);
        assert!(partial.default_pty);
        tokio::fs::write(&path, "not valid toml = [").await.unwrap();
        assert_eq!(load_from_path(&path).await, TuiSettings::default());
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[test]
    fn catppuccin_alias_selects_mocha() {
        let settings: TuiSettings = toml::from_str("theme = \"catppuccin\"\n").unwrap();
        assert_eq!(settings.theme, ThemeKind::CatppuccinMocha);
    }

    #[test]
    fn every_theme_roundtrips_through_settings() {
        for theme in ThemeKind::ALL {
            let settings = TuiSettings {
                theme,
                ..TuiSettings::default()
            };
            let encoded = toml::to_string(&settings).unwrap();
            let decoded: TuiSettings = toml::from_str(&encoded).unwrap();
            assert_eq!(decoded, settings, "theme {}", theme.label());
        }
    }

    #[test]
    fn cycles_through_supported_values() {
        let mut settings = TuiSettings::default();
        settings.cycle(SettingsRow::Theme, true);
        assert_eq!(settings.theme, ThemeKind::Dark);
        settings.cycle(SettingsRow::Theme, false);
        assert_eq!(settings.theme, ThemeKind::Terminal);
        settings.cycle(SettingsRow::SidebarWidth, true);
        assert_eq!(settings.sidebar_width, 34);
    }
}
