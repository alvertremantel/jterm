use jones_config::{
    ConfigError, data_dir as app_data_dir, load_app_config, load_toml_from_path, save_app_config,
    save_toml_to_path,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "writerm";
pub const CONFIG_FILE_NAME: &str = "config.toml";

#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub autosave: AutosaveConfig,
    #[serde(default)]
    pub workspace: WorkspaceConfig,
    #[serde(default)]
    pub layout: LayoutConfig,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UiConfig {
    #[serde(default = "default_true")]
    pub mouse: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AutosaveConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_autosave_delay")]
    pub delay_ms: u64,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WorkspaceConfig {
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default = "default_true")]
    pub markdown_first: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LayoutConfig {
    #[serde(default = "default_headings_width")]
    pub headings_width: u16,
    #[serde(default = "default_files_width")]
    pub files_width: u16,
    #[serde(default = "default_true")]
    pub paragraph_indent: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { mouse: true }
    }
}

impl Default for AutosaveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            delay_ms: default_autosave_delay(),
        }
    }
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            show_hidden: false,
            markdown_first: true,
        }
    }
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            headings_width: default_headings_width(),
            files_width: default_files_width(),
            paragraph_indent: true,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_autosave_delay() -> u64 {
    1000
}

fn default_headings_width() -> u16 {
    28
}

fn default_files_width() -> u16 {
    34
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        load_app_config(APP_NAME, CONFIG_FILE_NAME)
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        save_app_config(APP_NAME, CONFIG_FILE_NAME, self)
    }

    pub fn load_from_path(path: &Path) -> Result<Self, ConfigError> {
        load_toml_from_path(path)
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), ConfigError> {
        save_toml_to_path(path, self)
    }

    pub fn data_dir() -> PathBuf {
        app_data_dir(APP_NAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn defaults_match_writerm_plan() {
        let config = Config::default();

        assert!(config.ui.mouse);
        assert!(config.autosave.enabled);
        assert_eq!(config.autosave.delay_ms, 1000);
        assert!(!config.workspace.show_hidden);
        assert!(config.workspace.markdown_first);
        assert_eq!(config.layout.headings_width, 28);
        assert_eq!(config.layout.files_width, 34);
        assert!(config.layout.paragraph_indent);
    }

    #[test]
    fn toml_roundtrip_preserves_config() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let config = Config {
            ui: UiConfig { mouse: false },
            autosave: AutosaveConfig {
                enabled: false,
                delay_ms: 2500,
            },
            workspace: WorkspaceConfig {
                show_hidden: true,
                markdown_first: false,
            },
            layout: LayoutConfig {
                headings_width: 20,
                files_width: 40,
                paragraph_indent: false,
            },
        };

        config.save_to_path(&path).unwrap();

        assert_eq!(Config::load_from_path(&path).unwrap(), config);
    }
}
