//! The capability types, and resolving them.
//!
//! [`TerminalCaps`] records what the hosting terminal supports. All
//! rendering reads it; [`TerminalCaps::default`] describes a terminal
//! that answered nothing. [`resolve`] builds it from the probe's
//! [`Replies`], the environment ([`TermEnv`]) and the user's settings;
//! [`IconChoice::select`] turns it into the icon kind to draw.

use frext_core::config::{IconsMode, TermConfig, UndercurlMode};

use super::probe::{KittyReply, Replies, TcapReply};

/// XTVERSION substrings naming terminals known to draw undercurl.
const KNOWN_XTVERSION: &[&str] = &[
    "kitty",
    "ghostty",
    "foot",
    "wezterm",
    "freminal",
    "contour",
    "iTerm2",
    "alacritty",
];

/// `TERM_PROGRAM` values naming terminals known to draw undercurl.
const KNOWN_TERM_PROGRAM: &[&str] = &["ghostty", "wezterm", "freminal", "iTerm.app", "vscode"];

/// `TERM` values naming terminals known to draw undercurl.
const KNOWN_TERM: &[&str] = &[
    "xterm-kitty",
    "xterm-ghostty",
    "wezterm",
    "alacritty",
    "contour",
];

/// `TERM` prefixes naming terminals known to draw undercurl.
const KNOWN_TERM_PREFIX: &[&str] = &["foot"];

/// The size of one terminal cell, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPx {
    /// The width of one cell, in pixels.
    pub width: u16,
    /// The height of one cell, in pixels.
    pub height: u16,
}

/// How the terminal can draw images.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImageSupport {
    /// The kitty graphics protocol with unicode placeholders.
    KittyPlaceholders {
        /// The cell size, used to size rasterised images.
        cell_px: CellPx,
    },
    /// No usable image support.
    #[default]
    None,
}

/// How a coloured underline is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UnderlineStyle {
    /// A curly underline ("undercurl", `CSI 4:3 m`).
    Curly,
    /// A plain straight underline.
    #[default]
    Straight,
}

/// Which keyboard protocol the terminal speaks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KeyboardProtocol {
    /// The kitty keyboard protocol.
    Kitty {
        /// The progressive-enhancement flags the terminal reported.
        flags: u16,
    },
    /// Legacy keyboard input only.
    #[default]
    Legacy,
}

/// The terminal multiplexer frext runs inside, if any.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Multiplexer {
    /// No multiplexer.
    #[default]
    None,
    /// tmux.
    Tmux,
}

/// Everything frext knows about the hosting terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalCaps {
    /// How images can be drawn.
    pub images: ImageSupport,
    /// How a coloured underline is drawn.
    pub underline: UnderlineStyle,
    /// Which keyboard protocol the terminal speaks.
    pub keyboard: KeyboardProtocol,
    /// The multiplexer frext runs inside.
    pub multiplexer: Multiplexer,
    /// The raw XTVERSION reply, when the terminal sent one.
    pub version: Option<String>,
}

/// Which kind of file-tree icons to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconChoice {
    /// Kitty graphics icons drawn with unicode placeholders.
    Kitty {
        /// The cell size, used to size rasterised icons.
        cell_px: CellPx,
    },
    /// Nerd Font glyph icons.
    Nerd,
    /// Plain ASCII icons.
    Ascii,
    /// No icons.
    None,
}

impl IconChoice {
    /// Picks the icons to draw: `Auto` and `Kitty` give kitty icons when
    /// the terminal supports kitty placeholders and Nerd Font icons
    /// otherwise; the other modes map to themselves.
    #[must_use]
    pub fn select(requested: IconsMode, caps: &TerminalCaps) -> IconChoice {
        match requested {
            IconsMode::Auto | IconsMode::Kitty => match caps.images {
                ImageSupport::KittyPlaceholders { cell_px } => IconChoice::Kitty { cell_px },
                ImageSupport::None => IconChoice::Nerd,
            },
            IconsMode::Nerd => IconChoice::Nerd,
            IconsMode::Ascii => IconChoice::Ascii,
            IconsMode::None => IconChoice::None,
        }
    }
}

/// The environment variables capability detection reads. A variable set
/// to an empty string counts as unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TermEnv {
    /// `TERM`.
    pub term: Option<String>,
    /// `TERM_PROGRAM`.
    pub term_program: Option<String>,
    /// `TMUX`, set inside a tmux session.
    pub tmux: Option<String>,
    /// `WEZTERM_EXECUTABLE`, set by WezTerm.
    pub wezterm_executable: Option<String>,
    /// `KONSOLE_VERSION`, set by Konsole.
    pub konsole_version: Option<String>,
}

impl TermEnv {
    /// Reads the variables through `lookup`, which returns a variable's
    /// value by name. Empty values become `None`.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let read = |name: &str| lookup(name).filter(|value| !value.is_empty());
        Self {
            term: read("TERM"),
            term_program: read("TERM_PROGRAM"),
            tmux: read("TMUX"),
            wezterm_executable: read("WEZTERM_EXECUTABLE"),
            konsole_version: read("KONSOLE_VERSION"),
        }
    }

    /// Reads the variables from the process environment.
    #[must_use]
    pub fn from_process() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }
}

/// Detects the multiplexer: tmux when `TMUX` is set or `TERM` starts
/// with `tmux`.
#[must_use]
pub fn detect_multiplexer(env: &TermEnv) -> Multiplexer {
    if is_set(env.tmux.as_ref()).is_some()
        || is_set(env.term.as_ref()).is_some_and(|term| term.starts_with("tmux"))
    {
        Multiplexer::Tmux
    } else {
        Multiplexer::None
    }
}

/// Resolves the terminal's capabilities from the probe's replies, the
/// environment and the user's settings (config merged with CLI flags).
#[must_use]
pub fn resolve(replies: &Replies, env: &TermEnv, settings: TermConfig) -> TerminalCaps {
    let multiplexer = detect_multiplexer(env);
    let keyboard = match replies.keyboard_flags {
        Some(flags) => KeyboardProtocol::Kitty { flags },
        None => KeyboardProtocol::Legacy,
    };
    TerminalCaps {
        images: resolve_images(replies, env, settings.icons),
        underline: resolve_underline(replies, env, multiplexer, settings.undercurl),
        keyboard,
        multiplexer,
        version: replies.xtversion.clone(),
    }
}

/// The image support for the requested icons mode.
fn resolve_images(replies: &Replies, env: &TermEnv, icons: IconsMode) -> ImageSupport {
    match icons {
        IconsMode::Auto => {
            let blocklisted = is_set(env.wezterm_executable.as_ref()).is_some()
                || is_set(env.konsole_version.as_ref()).is_some();
            match replies.cell_px {
                Some(cell_px) if replies.kitty_graphics == KittyReply::Ok && !blocklisted => {
                    ImageSupport::KittyPlaceholders { cell_px }
                }
                _ => ImageSupport::None,
            }
        }
        IconsMode::Kitty => match replies.cell_px {
            Some(cell_px) => ImageSupport::KittyPlaceholders { cell_px },
            None => {
                log::warn!(
                    "icons = \"kitty\" needs the terminal's cell size, which it did not report; \
                     falling back to Nerd Font icons"
                );
                ImageSupport::None
            }
        },
        IconsMode::Nerd | IconsMode::Ascii | IconsMode::None => ImageSupport::None,
    }
}

/// The underline style for the requested undercurl mode.
fn resolve_underline(
    replies: &Replies,
    env: &TermEnv,
    multiplexer: Multiplexer,
    undercurl: UndercurlMode,
) -> UnderlineStyle {
    match undercurl {
        UndercurlMode::On => UnderlineStyle::Curly,
        UndercurlMode::Off => UnderlineStyle::Straight,
        UndercurlMode::Auto => {
            if replies.smulx == TcapReply::Supported
                || (multiplexer == Multiplexer::None && names_known_terminal(replies, env))
            {
                UnderlineStyle::Curly
            } else {
                UnderlineStyle::Straight
            }
        }
    }
}

/// Whether XTVERSION, `TERM_PROGRAM` or `TERM` names a terminal known to
/// draw undercurl.
fn names_known_terminal(replies: &Replies, env: &TermEnv) -> bool {
    let by_version = replies.xtversion.as_deref().is_some_and(|version| {
        let version = version.to_ascii_lowercase();
        KNOWN_XTVERSION
            .iter()
            .any(|name| version.contains(&name.to_ascii_lowercase()))
    });
    let by_program = is_set(env.term_program.as_ref()).is_some_and(|program| {
        KNOWN_TERM_PROGRAM
            .iter()
            .any(|name| program.eq_ignore_ascii_case(name))
    });
    let by_term = is_set(env.term.as_ref()).is_some_and(|term| {
        KNOWN_TERM
            .iter()
            .any(|name| term.eq_ignore_ascii_case(name))
            || KNOWN_TERM_PREFIX.iter().any(|prefix| {
                term.as_bytes()
                    .get(..prefix.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
            })
    });
    by_version || by_program || by_term
}

/// The variable's value, treating an empty string as unset.
fn is_set(value: Option<&String>) -> Option<&str> {
    value.map(String::as_str).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::term::probe::{fixtures, parse};

    fn env(pairs: &[(&str, &str)]) -> TermEnv {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        TermEnv::from_lookup(|name| map.get(name).cloned())
    }

    fn kitty(width: u16, height: u16) -> ImageSupport {
        ImageSupport::KittyPlaceholders {
            cell_px: CellPx { width, height },
        }
    }

    const TMUX_ENV: &[(&str, &str)] = &[
        ("TMUX", "/tmp/tmux-1000/default,1,0"),
        ("TERM", "tmux-256color"),
        ("TERM_PROGRAM", "tmux"),
    ];

    fn check(
        fixture: &[u8],
        pairs: &[(&str, &str)],
        images: ImageSupport,
        underline: UnderlineStyle,
        keyboard: KeyboardProtocol,
        multiplexer: Multiplexer,
        version: &str,
    ) {
        let caps = resolve(&parse(fixture), &env(pairs), TermConfig::default());
        assert_eq!(
            caps,
            TerminalCaps {
                images,
                underline,
                keyboard,
                multiplexer,
                version: Some(version.to_owned()),
            }
        );
    }

    #[test]
    fn kitty_caps() {
        check(
            fixtures::KITTY,
            &[("TERM", "xterm-kitty")],
            kitty(10, 20),
            UnderlineStyle::Curly,
            KeyboardProtocol::Kitty { flags: 0 },
            Multiplexer::None,
            "kitty(0.39.1)",
        );
    }

    #[test]
    fn ghostty_caps() {
        check(
            fixtures::GHOSTTY,
            &[("TERM", "xterm-ghostty"), ("TERM_PROGRAM", "ghostty")],
            kitty(8, 17),
            UnderlineStyle::Curly,
            KeyboardProtocol::Kitty { flags: 0 },
            Multiplexer::None,
            "ghostty 1.1.3",
        );
    }

    #[test]
    fn foot_caps() {
        check(
            fixtures::FOOT,
            &[("TERM", "foot")],
            ImageSupport::None,
            UnderlineStyle::Curly,
            KeyboardProtocol::Kitty { flags: 0 },
            Multiplexer::None,
            "foot(1.20.2)",
        );
    }

    #[test]
    fn wezterm_caps() {
        check(
            fixtures::WEZTERM,
            &[
                ("TERM_PROGRAM", "WezTerm"),
                ("WEZTERM_EXECUTABLE", "/usr/bin/wezterm"),
            ],
            ImageSupport::None,
            UnderlineStyle::Curly,
            KeyboardProtocol::Legacy,
            Multiplexer::None,
            "WezTerm 20240203-110809-5046fc22",
        );
    }

    #[test]
    fn freminal_caps() {
        check(
            fixtures::FREMINAL,
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "freminal")],
            kitty(10, 20),
            UnderlineStyle::Curly,
            KeyboardProtocol::Kitty { flags: 0 },
            Multiplexer::None,
            "XTerm(Freminal 0.13.0-beta.3)",
        );
    }

    #[test]
    fn alacritty_caps() {
        let caps = resolve(
            &parse(fixtures::ALACRITTY),
            &env(&[("TERM", "alacritty")]),
            TermConfig::default(),
        );
        assert_eq!(
            caps,
            TerminalCaps {
                images: ImageSupport::None,
                underline: UnderlineStyle::Curly,
                keyboard: KeyboardProtocol::Kitty { flags: 0 },
                multiplexer: Multiplexer::None,
                version: None,
            }
        );
    }

    #[test]
    fn tmux_caps() {
        check(
            fixtures::TMUX,
            TMUX_ENV,
            ImageSupport::None,
            UnderlineStyle::Straight,
            KeyboardProtocol::Legacy,
            Multiplexer::Tmux,
            "tmux 3.5a",
        );
    }

    #[test]
    fn freminal_is_detected_by_query_alone() {
        let caps = resolve(
            &parse(fixtures::FREMINAL),
            &TermEnv::default(),
            TermConfig::default(),
        );
        assert_eq!(caps.images, kitty(10, 20));
        assert_eq!(caps.underline, UnderlineStyle::Curly);
        assert_eq!(caps.keyboard, KeyboardProtocol::Kitty { flags: 0 });
    }

    #[test]
    fn konsole_is_blocklisted_for_images() {
        let caps = resolve(
            &parse(fixtures::KITTY),
            &env(&[("KONSOLE_VERSION", "230800")]),
            TermConfig::default(),
        );
        assert_eq!(caps.images, ImageSupport::None);
    }

    #[test]
    fn smulx_under_tmux_gives_curly() {
        let replies = Replies {
            smulx: TcapReply::Supported,
            ..parse(fixtures::TMUX)
        };
        let caps = resolve(&replies, &env(TMUX_ENV), TermConfig::default());
        assert_eq!(caps.underline, UnderlineStyle::Curly);
    }

    #[test]
    fn known_terminal_list_does_not_apply_under_tmux() {
        let replies = Replies {
            smulx: TcapReply::Missing,
            ..parse(fixtures::GHOSTTY)
        };
        let caps = resolve(&replies, &env(TMUX_ENV), TermConfig::default());
        assert_eq!(caps.underline, UnderlineStyle::Straight);
    }

    #[test]
    fn no_replies_give_the_default_caps() {
        let caps = resolve(
            &Replies::default(),
            &env(&[("TERM", "xterm-256color")]),
            TermConfig::default(),
        );
        assert_eq!(caps, TerminalCaps::default());
    }

    #[test]
    fn undercurl_on_forces_curly() {
        let settings = TermConfig {
            undercurl: UndercurlMode::On,
            ..TermConfig::default()
        };
        let caps = resolve(&Replies::default(), &TermEnv::default(), settings);
        assert_eq!(caps.underline, UnderlineStyle::Curly);
    }

    #[test]
    fn undercurl_off_forces_straight() {
        let settings = TermConfig {
            undercurl: UndercurlMode::Off,
            ..TermConfig::default()
        };
        let caps = resolve(
            &parse(fixtures::KITTY),
            &env(&[("TERM", "xterm-kitty")]),
            settings,
        );
        assert_eq!(caps.underline, UnderlineStyle::Straight);
    }

    #[test]
    fn forced_kitty_icons_ignore_the_blocklist() {
        let settings = TermConfig {
            icons: IconsMode::Kitty,
            ..TermConfig::default()
        };
        let caps = resolve(
            &parse(fixtures::WEZTERM),
            &env(&[
                ("TERM_PROGRAM", "WezTerm"),
                ("WEZTERM_EXECUTABLE", "/usr/bin/wezterm"),
            ]),
            settings,
        );
        assert_eq!(caps.images, kitty(9, 18));
    }

    #[test]
    fn forced_kitty_icons_without_cell_size_give_no_images() {
        let settings = TermConfig {
            icons: IconsMode::Kitty,
            ..TermConfig::default()
        };
        let replies = Replies {
            cell_px: None,
            ..parse(fixtures::FOOT)
        };
        let caps = resolve(&replies, &env(&[("TERM", "foot")]), settings);
        assert_eq!(caps.images, ImageSupport::None);
    }

    #[test]
    fn non_kitty_icon_modes_give_no_images() {
        for icons in [IconsMode::Nerd, IconsMode::Ascii, IconsMode::None] {
            let settings = TermConfig {
                icons,
                ..TermConfig::default()
            };
            let caps = resolve(
                &parse(fixtures::KITTY),
                &env(&[("TERM", "xterm-kitty")]),
                settings,
            );
            assert_eq!(caps.images, ImageSupport::None, "{icons:?}");
        }
    }

    #[test]
    fn tmux_variable_means_tmux() {
        assert_eq!(
            detect_multiplexer(&env(&[("TMUX", "/tmp/tmux-1000/default,1,0")])),
            Multiplexer::Tmux
        );
    }

    #[test]
    fn tmux_term_means_tmux() {
        assert_eq!(
            detect_multiplexer(&env(&[("TERM", "tmux-256color")])),
            Multiplexer::Tmux
        );
    }

    #[test]
    fn screen_term_is_not_tmux() {
        assert_eq!(
            detect_multiplexer(&env(&[("TERM", "screen-256color")])),
            Multiplexer::None
        );
    }

    #[test]
    fn from_lookup_maps_empty_to_none() {
        assert_eq!(env(&[("TERM", "")]).term, None);
    }

    #[test]
    fn from_lookup_reads_each_variable() {
        let read = env(&[
            ("TERM", "a"),
            ("TERM_PROGRAM", "b"),
            ("TMUX", "c"),
            ("WEZTERM_EXECUTABLE", "d"),
            ("KONSOLE_VERSION", "e"),
        ]);
        assert_eq!(
            read,
            TermEnv {
                term: Some("a".to_owned()),
                term_program: Some("b".to_owned()),
                tmux: Some("c".to_owned()),
                wezterm_executable: Some("d".to_owned()),
                konsole_version: Some("e".to_owned()),
            }
        );
    }

    #[test]
    fn select_kitty_when_images_are_supported() {
        let caps = TerminalCaps {
            images: kitty(9, 18),
            ..TerminalCaps::default()
        };
        let cell_px = CellPx {
            width: 9,
            height: 18,
        };
        for mode in [IconsMode::Auto, IconsMode::Kitty] {
            assert_eq!(
                IconChoice::select(mode, &caps),
                IconChoice::Kitty { cell_px },
                "{mode:?}"
            );
        }
    }

    #[test]
    fn select_nerd_when_images_are_unsupported() {
        let caps = TerminalCaps::default();
        for mode in [IconsMode::Auto, IconsMode::Kitty] {
            assert_eq!(
                IconChoice::select(mode, &caps),
                IconChoice::Nerd,
                "{mode:?}"
            );
        }
    }

    #[test]
    fn select_maps_other_modes_to_themselves() {
        let caps = TerminalCaps {
            images: kitty(9, 18),
            ..TerminalCaps::default()
        };
        assert_eq!(IconChoice::select(IconsMode::Nerd, &caps), IconChoice::Nerd);
        assert_eq!(
            IconChoice::select(IconsMode::Ascii, &caps),
            IconChoice::Ascii
        );
        assert_eq!(IconChoice::select(IconsMode::None, &caps), IconChoice::None);
    }

    #[test]
    fn default_caps_describe_a_terminal_that_answered_nothing() {
        let caps = TerminalCaps::default();
        assert_eq!(caps.images, ImageSupport::None);
        assert_eq!(caps.underline, UnderlineStyle::Straight);
        assert_eq!(caps.keyboard, KeyboardProtocol::Legacy);
        assert_eq!(caps.multiplexer, Multiplexer::None);
        assert_eq!(caps.version, None);
    }
}
