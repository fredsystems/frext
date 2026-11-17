//! Command-line argument parsing.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use frext_core::config::{IconsMode, TermConfig, UndercurlMode};

const ICONS: &str = "--icons";
const UNDERCURL: &str = "--undercurl";

/// Paths and overrides given on the command line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// Files to open as tabs, in command-line order.
    pub files: Vec<PathBuf>,
    /// The workspace root for the sidebar: the first directory argument.
    pub dir: Option<PathBuf>,
    /// The `--icons` override, if given.
    pub icons: Option<IconsMode>,
    /// The `--undercurl` override, if given.
    pub undercurl: Option<UndercurlMode>,
}

impl Args {
    /// Apply the command-line overrides to `config`: each flag that was
    /// given replaces the matching field.
    #[must_use]
    pub fn apply_to(&self, config: TermConfig) -> TermConfig {
        TermConfig {
            icons: self.icons.unwrap_or(config.icons),
            undercurl: self.undercurl.unwrap_or(config.undercurl),
        }
    }
}

/// A command-line argument that could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// A flag that takes a value was the last argument.
    MissingValue {
        /// The flag, as typed without a value (e.g. `--icons`).
        flag: &'static str,
    },
    /// A flag was given a value it does not accept.
    InvalidValue {
        /// The flag, as typed without a value (e.g. `--icons`).
        flag: &'static str,
        /// The rejected value (lossily converted if not UTF-8).
        value: String,
    },
    /// An argument starting with `--` that is not a known option.
    UnknownOption(String),
}

impl CliError {
    /// The values `flag` accepts, for the error message.
    fn expected(flag: &str) -> &'static str {
        if flag == ICONS {
            "auto, kitty, nerd, ascii, none"
        } else {
            "auto, on, off"
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingValue { flag } => {
                write!(f, "{flag} needs a value ({})", Self::expected(flag))
            }
            Self::InvalidValue { flag, value } => write!(
                f,
                "invalid value \"{value}\" for {flag} (expected {})",
                Self::expected(flag)
            ),
            Self::UnknownOption(option) => write!(
                f,
                "unknown option \"{option}\" (put -- before a file name that starts with --)"
            ),
        }
    }
}

impl std::error::Error for CliError {}

/// Parse `value` for `flag` with the core [`FromStr`] impl.
fn parse_value<T: FromStr>(flag: &'static str, value: &OsStr) -> Result<T, CliError> {
    let invalid = || CliError::InvalidValue {
        flag,
        value: value.to_string_lossy().into_owned(),
    };
    value
        .to_str()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())
}

/// Parse the arguments after the program name.
///
/// `--icons <mode>` / `--icons=<mode>` and `--undercurl <mode>` /
/// `--undercurl=<mode>` set the overrides; the last of a repeated flag
/// wins. `--` ends option parsing. Every other argument is a path: the
/// first directory becomes [`Args::dir`], later directories are ignored,
/// and everything else is a file to open.
///
/// # Errors
///
/// Returns [`CliError::MissingValue`] if a flag is the last argument,
/// [`CliError::InvalidValue`] if its value is not a valid mode, and
/// [`CliError::UnknownOption`] for any other argument starting with `--`
/// before `--`.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Args, CliError> {
    let mut parsed = Args::default();
    let mut args = args.into_iter();
    let mut options_ended = false;
    while let Some(arg) = args.next() {
        if !options_ended {
            let text = arg.to_string_lossy();
            if text.starts_with("--") {
                let (name, inline) = match text.split_once('=') {
                    Some((name, _)) => (name, true),
                    None => (text.as_ref(), false),
                };
                let flag = match name {
                    "--" if !inline => {
                        options_ended = true;
                        continue;
                    }
                    ICONS => ICONS,
                    UNDERCURL => UNDERCURL,
                    _ => return Err(CliError::UnknownOption(text.into_owned())),
                };
                let value = if inline {
                    inline_value(&arg, flag)
                } else {
                    args.next().ok_or(CliError::MissingValue { flag })?
                };
                if flag == ICONS {
                    parsed.icons = Some(parse_value(flag, &value)?);
                } else {
                    parsed.undercurl = Some(parse_value(flag, &value)?);
                }
                continue;
            }
        }
        let path = PathBuf::from(arg);
        if path.is_dir() {
            if parsed.dir.is_none() {
                parsed.dir = Some(path);
            }
        } else {
            parsed.files.push(path);
        }
    }
    Ok(parsed)
}

/// The value after `flag=` in `arg`, keeping non-UTF-8 bytes where the
/// platform allows it.
fn inline_value(arg: &OsStr, flag: &str) -> OsString {
    let skip = flag.len() + 1;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(arg.as_bytes().get(skip..).unwrap_or_default()).to_owned()
    }
    #[cfg(not(unix))]
    {
        OsString::from(arg.to_string_lossy().get(skip..).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use std::fs;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-term-cli-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn os(paths: &[&PathBuf]) -> Vec<OsString> {
        paths.iter().map(|p| p.as_os_str().to_owned()).collect()
    }

    fn strs(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn no_args_is_empty() {
        assert_eq!(parse(Vec::new()).unwrap(), Args::default());
    }

    #[test]
    fn files_keep_order() {
        let root = temp_dir("order");
        let b = root.join("b.txt");
        let a = root.join("a.txt");
        let missing = root.join("missing.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let args = parse(os(&[&b, &missing, &a])).unwrap();

        assert_eq!(args.files, vec![b, missing, a]);
        assert_eq!(args.dir, None);
    }

    #[test]
    fn first_dir_wins() {
        let root = temp_dir("first-dir");
        let file = root.join("f.txt");
        let dir = root.join("ws");
        fs::write(&file, "f").unwrap();
        fs::create_dir_all(&dir).unwrap();

        let args = parse(os(&[&file, &dir])).unwrap();

        assert_eq!(args.files, vec![file]);
        assert_eq!(args.dir, Some(dir));
    }

    #[test]
    fn second_dir_is_ignored() {
        let root = temp_dir("second-dir");
        let first = root.join("one");
        let second = root.join("two");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();

        let args = parse(os(&[&first, &second])).unwrap();

        assert_eq!(args.dir, Some(first));
        assert!(args.files.is_empty());
    }

    #[test]
    fn icons_separate_value() {
        let args = parse(strs(&["--icons", "nerd"])).unwrap();
        assert_eq!(args.icons, Some(IconsMode::Nerd));
        assert_eq!(args.undercurl, None);
        assert!(args.files.is_empty());
    }

    #[test]
    fn icons_inline_value() {
        let args = parse(strs(&["--icons=ascii"])).unwrap();
        assert_eq!(args.icons, Some(IconsMode::Ascii));
        assert!(args.files.is_empty());
    }

    #[test]
    fn undercurl_separate_value() {
        let args = parse(strs(&["--undercurl", "off"])).unwrap();
        assert_eq!(args.undercurl, Some(UndercurlMode::Off));
        assert_eq!(args.icons, None);
    }

    #[test]
    fn undercurl_inline_value() {
        let args = parse(strs(&["--undercurl=on"])).unwrap();
        assert_eq!(args.undercurl, Some(UndercurlMode::On));
    }

    #[test]
    fn last_icons_wins() {
        let args = parse(strs(&["--icons", "nerd", "--icons=none"])).unwrap();
        assert_eq!(args.icons, Some(IconsMode::None));
    }

    #[test]
    fn flags_between_paths_keep_path_rules() {
        let root = temp_dir("mixed");
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        let first = root.join("one");
        let second = root.join("two");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();

        let argv = vec![
            b.clone().into_os_string(),
            OsString::from("--icons"),
            OsString::from("kitty"),
            first.clone().into_os_string(),
            OsString::from("--undercurl=off"),
            a.clone().into_os_string(),
            second.into_os_string(),
        ];
        let args = parse(argv).unwrap();

        assert_eq!(args.files, vec![b, a]);
        assert_eq!(args.dir, Some(first));
        assert_eq!(args.icons, Some(IconsMode::Kitty));
        assert_eq!(args.undercurl, Some(UndercurlMode::Off));
    }

    #[test]
    fn icons_last_is_missing_value() {
        assert_eq!(
            parse(strs(&["file.txt", "--icons"])),
            Err(CliError::MissingValue { flag: "--icons" })
        );
    }

    #[test]
    fn undercurl_last_is_missing_value() {
        assert_eq!(
            parse(strs(&["--undercurl"])),
            Err(CliError::MissingValue {
                flag: "--undercurl"
            })
        );
    }

    #[test]
    fn icons_empty_inline_is_invalid() {
        assert_eq!(
            parse(strs(&["--icons="])),
            Err(CliError::InvalidValue {
                flag: "--icons",
                value: String::new(),
            })
        );
    }

    #[test]
    fn icons_bad_value_is_invalid() {
        assert_eq!(
            parse(strs(&["--icons", "sixel"])),
            Err(CliError::InvalidValue {
                flag: "--icons",
                value: "sixel".to_owned(),
            })
        );
    }

    #[test]
    fn undercurl_bad_value_is_invalid() {
        assert_eq!(
            parse(strs(&["--undercurl", "yes"])),
            Err(CliError::InvalidValue {
                flag: "--undercurl",
                value: "yes".to_owned(),
            })
        );
    }

    #[test]
    fn flag_value_may_start_with_dashes() {
        assert_eq!(
            parse(strs(&["--icons", "--undercurl"])),
            Err(CliError::InvalidValue {
                flag: "--icons",
                value: "--undercurl".to_owned(),
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_value_is_invalid_with_lossy_text() {
        use std::os::unix::ffi::OsStringExt;

        let separate = vec![
            OsString::from("--icons"),
            OsString::from_vec(vec![b'n', 0xff]),
        ];
        let inline = vec![OsString::from_vec(b"--undercurl=o\xff".to_vec())];

        assert_eq!(
            parse(separate),
            Err(CliError::InvalidValue {
                flag: "--icons",
                value: "n\u{fffd}".to_owned(),
            })
        );
        assert_eq!(
            parse(inline),
            Err(CliError::InvalidValue {
                flag: "--undercurl",
                value: "o\u{fffd}".to_owned(),
            })
        );
    }

    #[test]
    fn unknown_option_is_rejected() {
        assert_eq!(
            parse(strs(&["--frobnicate"])),
            Err(CliError::UnknownOption("--frobnicate".to_owned()))
        );
        assert_eq!(
            parse(strs(&["--iconsx=nerd"])),
            Err(CliError::UnknownOption("--iconsx=nerd".to_owned()))
        );
    }

    #[test]
    fn double_dash_ends_options() {
        let args = parse(strs(&["--", "--icons"])).unwrap();
        assert_eq!(args.files, vec![PathBuf::from("--icons")]);
        assert_eq!(args.icons, None);
    }

    #[test]
    fn single_dash_is_a_file() {
        let args = parse(strs(&["-", "-x"])).unwrap();
        assert_eq!(args.files, vec![PathBuf::from("-"), PathBuf::from("-x")]);
    }

    #[test]
    fn error_messages() {
        let cases = [
            (
                CliError::MissingValue { flag: "--icons" },
                "--icons needs a value (auto, kitty, nerd, ascii, none)",
            ),
            (
                CliError::InvalidValue {
                    flag: "--icons",
                    value: "x".to_owned(),
                },
                "invalid value \"x\" for --icons (expected auto, kitty, nerd, ascii, none)",
            ),
            (
                CliError::MissingValue {
                    flag: "--undercurl",
                },
                "--undercurl needs a value (auto, on, off)",
            ),
            (
                CliError::InvalidValue {
                    flag: "--undercurl",
                    value: "x".to_owned(),
                },
                "invalid value \"x\" for --undercurl (expected auto, on, off)",
            ),
            (
                CliError::UnknownOption("--foo".to_owned()),
                "unknown option \"--foo\" (put -- before a file name that starts with --)",
            ),
        ];
        for (err, message) in cases {
            assert_eq!(err.to_string(), message);
        }
    }

    #[test]
    fn apply_to_without_flags_keeps_config() {
        let config = TermConfig {
            icons: IconsMode::Nerd,
            undercurl: UndercurlMode::Off,
        };
        assert_eq!(Args::default().apply_to(config), config);
    }

    #[test]
    fn apply_to_icons_overrides_only_icons() {
        let config = TermConfig {
            icons: IconsMode::Nerd,
            undercurl: UndercurlMode::Off,
        };
        let args = Args {
            icons: Some(IconsMode::Ascii),
            ..Args::default()
        };
        assert_eq!(
            args.apply_to(config),
            TermConfig {
                icons: IconsMode::Ascii,
                undercurl: UndercurlMode::Off,
            }
        );
    }

    #[test]
    fn apply_to_undercurl_overrides_only_undercurl() {
        let config = TermConfig {
            icons: IconsMode::Nerd,
            undercurl: UndercurlMode::Off,
        };
        let args = Args {
            undercurl: Some(UndercurlMode::On),
            ..Args::default()
        };
        assert_eq!(
            args.apply_to(config),
            TermConfig {
                icons: IconsMode::Nerd,
                undercurl: UndercurlMode::On,
            }
        );
    }
}
