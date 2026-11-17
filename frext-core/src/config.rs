//! The shared frext config file.
//!
//! Both front ends read one TOML file, `$XDG_CONFIG_HOME/frext/config.toml`
//! on Linux (see [`Config::default_path`]). Each front end reads its own
//! table: the terminal front end reads `[term]`.
//!
//! ```toml
//! [term]
//! icons = "auto"      # "auto" | "kitty" | "nerd" | "ascii" | "none"
//! undercurl = "auto"  # "auto" | "on" | "off"
//! ```
//!
//! Unknown keys and tables are ignored, because the Home Manager module
//! writes its free-form `settings` to this file verbatim. A missing table
//! or key keeps its default.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::Deserialize;

use crate::error::ConfigError;

/// Which kind of file-tree icons to draw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconsMode {
    /// Pick the best icons the terminal supports.
    #[default]
    Auto,
    /// Force kitty graphics icons (still needs the terminal's cell size).
    Kitty,
    /// Nerd Font glyph icons.
    Nerd,
    /// Plain ASCII icons.
    Ascii,
    /// No icons.
    None,
}

impl FromStr for IconsMode {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "kitty" => Ok(Self::Kitty),
            "nerd" => Ok(Self::Nerd),
            "ascii" => Ok(Self::Ascii),
            "none" => Ok(Self::None),
            _ => Err(ConfigError::InvalidValue {
                field: "icons",
                value: s.to_owned(),
            }),
        }
    }
}

/// Whether to draw proofreading underlines curly ("undercurl").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UndercurlMode {
    /// Use undercurl when the terminal is detected to support it.
    #[default]
    Auto,
    /// Always use undercurl.
    On,
    /// Never use undercurl; draw straight underlines.
    Off,
}

impl FromStr for UndercurlMode {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            _ => Err(ConfigError::InvalidValue {
                field: "undercurl",
                value: s.to_owned(),
            }),
        }
    }
}

/// The `[term]` table: settings for the terminal front end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TermConfig {
    /// Which file-tree icons to draw.
    pub icons: IconsMode,
    /// Whether proofreading underlines are curly.
    pub undercurl: UndercurlMode,
}

/// The whole config file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The `[term]` table.
    pub term: TermConfig,
}

impl Config {
    /// Parse config file text.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] if `text` is not valid TOML or a
    /// known key has an invalid value.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(text)?)
    }

    /// Read and parse the config file at `path`. A missing file gives the
    /// default config.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Read`] if the file exists but cannot be read,
    /// or [`ConfigError::Parse`] if its contents are invalid.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            }),
        }
    }

    /// The platform config file path (`$XDG_CONFIG_HOME/frext/config.toml`
    /// on Linux), or `None` if no home directory can be determined.
    #[must_use]
    pub fn default_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("io.github", "fredsystems", "frext")
            .map(|dirs| dirs.config_dir().join("config.toml"))
    }

    /// Load the config from [`Config::default_path`]. Any failure is
    /// logged and gives the default config; it never aborts startup.
    #[must_use]
    pub fn load() -> Self {
        let Some(path) = Self::default_path() else {
            return Self::default();
        };
        match Self::load_from(&path) {
            Ok(config) => config,
            Err(e) => {
                log::warn!("ignoring config file {}: {e}", path.display());
                Self::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected
    // `Err` is exactly the failure signal we want.
    #![allow(clippy::unwrap_used)]

    use std::fs;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn empty_text_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn full_term_table_parses_every_value() {
        let icons = [
            ("auto", IconsMode::Auto),
            ("kitty", IconsMode::Kitty),
            ("nerd", IconsMode::Nerd),
            ("ascii", IconsMode::Ascii),
            ("none", IconsMode::None),
        ];
        let undercurl = [
            ("auto", UndercurlMode::Auto),
            ("on", UndercurlMode::On),
            ("off", UndercurlMode::Off),
        ];
        for (icon_text, icon_mode) in icons {
            for (curl_text, curl_mode) in undercurl {
                let text =
                    format!("[term]\nicons = \"{icon_text}\"\nundercurl = \"{curl_text}\"\n");
                assert_eq!(
                    Config::parse(&text).unwrap(),
                    Config {
                        term: TermConfig {
                            icons: icon_mode,
                            undercurl: curl_mode,
                        },
                    }
                );
            }
        }
    }

    #[test]
    fn unknown_keys_and_tables_are_ignored() {
        let text = "theme = \"catppuccin-mocha\"\n\n[gui]\nfont_size = 14\n";
        assert_eq!(Config::parse(text).unwrap(), Config::default());
    }

    #[test]
    fn partial_term_table_keeps_other_default() {
        let config = Config::parse("[term]\nundercurl = \"off\"\n").unwrap();
        assert_eq!(config.term.undercurl, UndercurlMode::Off);
        assert_eq!(config.term.icons, IconsMode::Auto);
    }

    #[test]
    fn invalid_value_is_parse_error() {
        let result = Config::parse("[term]\nicons = \"sixel\"\n");
        assert!(matches!(result, Err(ConfigError::Parse(_))));
    }

    #[test]
    fn icons_from_str_round_trips_and_rejects() {
        for (text, mode) in [
            ("auto", IconsMode::Auto),
            ("kitty", IconsMode::Kitty),
            ("nerd", IconsMode::Nerd),
            ("ascii", IconsMode::Ascii),
            ("none", IconsMode::None),
        ] {
            assert_eq!(text.parse::<IconsMode>().unwrap(), mode);
        }
        for bad in ["Auto", "", "sixel"] {
            match bad.parse::<IconsMode>() {
                Err(ConfigError::InvalidValue { field, value }) => {
                    assert_eq!(field, "icons");
                    assert_eq!(value, bad);
                }
                other => panic!("expected InvalidValue for {bad:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn undercurl_from_str_round_trips_and_rejects() {
        for (text, mode) in [
            ("auto", UndercurlMode::Auto),
            ("on", UndercurlMode::On),
            ("off", UndercurlMode::Off),
        ] {
            assert_eq!(text.parse::<UndercurlMode>().unwrap(), mode);
        }
        for bad in ["Auto", "", "sixel"] {
            match bad.parse::<UndercurlMode>() {
                Err(ConfigError::InvalidValue { field, value }) => {
                    assert_eq!(field, "undercurl");
                    assert_eq!(value, bad);
                }
                other => panic!("expected InvalidValue for {bad:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn load_from_missing_path_is_default() {
        let dir = temp_dir("config-missing");
        let config = Config::load_from(&dir.join("config.toml")).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn load_from_file_parses_content() {
        let dir = temp_dir("config-file");
        let path = dir.join("config.toml");
        fs::write(&path, "[term]\nicons = \"nerd\"\nundercurl = \"on\"\n").unwrap();
        assert_eq!(
            Config::load_from(&path).unwrap(),
            Config {
                term: TermConfig {
                    icons: IconsMode::Nerd,
                    undercurl: UndercurlMode::On,
                },
            }
        );
    }

    #[test]
    fn load_from_directory_is_read_error() {
        let dir = temp_dir("config-dir");
        let result = Config::load_from(&dir);
        assert!(matches!(result, Err(ConfigError::Read { path, .. }) if path == dir));
    }

    // Other platforms name the directory differently
    // (e.g. `io.github.fredsystems.frext` on macOS).
    #[cfg(target_os = "linux")]
    #[test]
    fn default_path_ends_with_frext_config_toml() {
        if let Some(path) = Config::default_path() {
            assert!(path.ends_with("frext/config.toml"), "{}", path.display());
        }
    }
}
