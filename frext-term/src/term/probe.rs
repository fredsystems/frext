//! The startup probe: the query bytes, the reply parser and the read
//! loop.
//!
//! [`query`] builds the single batch of queries written to the terminal;
//! [`parse`] turns whatever came back into [`Replies`]. Both are pure.
//! [`read_replies`] collects the replies from the terminal's input and
//! [`probe`] connects the three.
//! The DA1 query goes last because every terminal answers it, so its
//! reply marks the end of the batch.

use std::io::{self, Write};
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::io::Errno;

use super::caps::{CellPx, Multiplexer};
use super::tmux;

/// How long the probe waits for the terminal's replies.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// The size of each read from the input.
const READ_CHUNK: usize = 1024;

/// The escape byte.
const ESC: u8 = 0x1b;

/// The bell byte, an alternative string terminator.
const BEL: u8 = 0x07;

/// The kitty graphics support query (image id 31, one RGB pixel).
pub const KITTY_QUERY: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";

/// Asks for the cell size in pixels.
const CELL_SIZE_QUERY: &[u8] = b"\x1b[16t";

/// XTVERSION: asks for the terminal's name and version.
const XTVERSION_QUERY: &[u8] = b"\x1b[>q";

/// XTGETTCAP for `Smulx` (styled underlines), the name hex-encoded.
const SMULX_QUERY: &[u8] = b"\x1bP+q536d756c78\x1b\\";

/// Asks for the kitty keyboard protocol flags.
const KEYBOARD_QUERY: &[u8] = b"\x1b[?u";

/// DA1, the terminator every terminal answers.
const DA1_QUERY: &[u8] = b"\x1b[c";

/// `Smulx`, hex-encoded as XTGETTCAP names it.
const SMULX_HEX: &[u8] = b"536d756c78";

/// Builds the probe's queries in order. Under tmux the kitty graphics
/// query is wrapped for passthrough; the rest are answered by tmux
/// itself.
#[must_use]
pub fn query(multiplexer: Multiplexer) -> Vec<u8> {
    let mut out = tmux::wrap_for(multiplexer, KITTY_QUERY);
    for seq in [
        CELL_SIZE_QUERY,
        XTVERSION_QUERY,
        SMULX_QUERY,
        KEYBOARD_QUERY,
        DA1_QUERY,
    ] {
        out.extend_from_slice(seq);
    }
    out
}

/// The terminal's answer to the kitty graphics query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KittyReply {
    /// The terminal answered `OK`.
    Ok,
    /// The terminal answered with an error.
    Error,
    /// No answer.
    #[default]
    Missing,
}

/// The terminal's answer to an XTGETTCAP query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TcapReply {
    /// The capability is supported.
    Supported,
    /// The terminal reported the capability as unsupported.
    Unsupported,
    /// No answer.
    #[default]
    Missing,
}

/// How the replies ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReplyEnd {
    /// The DA1 reply arrived, so every answer is in.
    Da1,
    /// The input ended before the DA1 reply.
    #[default]
    Incomplete,
}

/// Everything the terminal answered to [`query`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replies {
    /// The answer to the kitty graphics query.
    pub kitty_graphics: KittyReply,
    /// The cell size, when the terminal reported a non-zero one.
    pub cell_px: Option<CellPx>,
    /// The XTVERSION text (the part after `>|`).
    pub xtversion: Option<String>,
    /// The answer to the XTGETTCAP `Smulx` query.
    pub smulx: TcapReply,
    /// The kitty keyboard protocol flags, when the terminal reported them.
    pub keyboard_flags: Option<u16>,
    /// Whether the DA1 reply arrived.
    pub end: ReplyEnd,
}

/// Parses the terminal's replies. Unknown sequences and stray bytes are
/// skipped, a sequence cut off at the end is ignored, and parsing stops
/// after the DA1 reply.
#[must_use]
pub fn parse(bytes: &[u8]) -> Replies {
    let mut replies = Replies::default();
    let mut input = bytes;
    loop {
        match step(input) {
            Step::End => break,
            Step::Skip(rest) => input = rest,
            Step::Token(token, rest) => {
                if interpret(&token, &mut replies) == ReplyEnd::Da1 {
                    replies.end = ReplyEnd::Da1;
                    break;
                }
                input = rest;
            }
        }
    }
    replies
}

/// Reads the terminal's replies from `input` until the DA1 reply has
/// arrived, the input reaches end of file, or `timeout` has passed with
/// nothing more to read. Interrupted calls are retried.
///
/// # Errors
///
/// Returns the error from polling or reading `input`, other than an
/// interruption.
pub fn read_replies(input: impl AsFd, timeout: Duration) -> io::Result<Vec<u8>> {
    let deadline = Instant::now().checked_add(timeout);
    let mut bytes = Vec::new();
    let mut buf = [0_u8; READ_CHUNK];
    loop {
        let remaining = deadline.map(|d| to_timespec(d.saturating_duration_since(Instant::now())));
        let mut fds = [PollFd::new(&input, PollFlags::IN)];
        match rustix::event::poll(&mut fds, remaining.as_ref()) {
            Ok(0) => break,
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(err) => return Err(io::Error::from(err)),
        }
        let read = match rustix::io::read(&input, &mut buf) {
            Ok(0) => break,
            Ok(read) => read,
            Err(Errno::INTR) => continue,
            Err(err) => return Err(io::Error::from(err)),
        };
        bytes.extend_from_slice(buf.get(..read).unwrap_or_default());
        if parse(&bytes).end == ReplyEnd::Da1 {
            break;
        }
    }
    Ok(bytes)
}

/// Converts a poll timeout, saturating one too large for a [`Timespec`].
fn to_timespec(duration: Duration) -> Timespec {
    Timespec::try_from(duration).unwrap_or(Timespec {
        tv_sec: i64::MAX,
        tv_nsec: 0,
    })
}

/// Writes [`query`] to `out`, reads the replies from `input` with
/// [`read_replies`] and parses them. The raw replies are logged at
/// `debug`.
///
/// # Errors
///
/// Returns the error from writing the query or reading the replies.
pub fn probe(
    out: &mut impl Write,
    input: impl AsFd,
    multiplexer: Multiplexer,
    timeout: Duration,
) -> io::Result<Replies> {
    out.write_all(&query(multiplexer))?;
    out.flush()?;
    let bytes = read_replies(input, timeout)?;
    log::debug!("terminal probe replies: {}", bytes.escape_ascii());
    Ok(parse(&bytes))
}

/// One complete escape sequence.
enum Token<'a> {
    /// `ESC [ params intermediates final`.
    Csi {
        /// The parameter bytes (`0x30..=0x3F`).
        params: &'a [u8],
        /// The intermediate bytes (`0x20..=0x2F`).
        intermediates: &'a [u8],
        /// The final byte (`0x40..=0x7E`).
        final_byte: u8,
    },
    /// `ESC P body ST`.
    Dcs(&'a [u8]),
    /// `ESC _ body ST`.
    Apc(&'a [u8]),
    /// `ESC ] body ST`.
    Osc,
}

/// One step of the tokenizer.
enum Step<'a> {
    /// A complete sequence, and the input after it.
    Token(Token<'a>, &'a [u8]),
    /// Nothing usable here; continue with the given input.
    Skip(&'a [u8]),
    /// The input is exhausted, or ends in a cut-off sequence.
    End,
}

/// Reads one token (or skips one byte) from the front of `input`.
fn step(input: &[u8]) -> Step<'_> {
    let Some((&first, rest)) = input.split_first() else {
        return Step::End;
    };
    if first != ESC {
        return Step::Skip(rest);
    }
    let Some((&kind, body)) = rest.split_first() else {
        return Step::End;
    };
    match kind {
        b'[' => csi(body),
        b'P' => string(body, Token::Dcs),
        b'_' => string(body, Token::Apc),
        b']' => string(body, |_| Token::Osc),
        _ => Step::Skip(rest),
    }
}

/// Reads a CSI sequence from `input`, which starts after `ESC [`.
fn csi(input: &[u8]) -> Step<'_> {
    let params_len = input
        .iter()
        .take_while(|b| (0x30..=0x3F).contains(*b))
        .count();
    let Some((params, rest)) = input.split_at_checked(params_len) else {
        return Step::End;
    };
    let intermediates_len = rest
        .iter()
        .take_while(|b| (0x20..=0x2F).contains(*b))
        .count();
    let Some((intermediates, rest)) = rest.split_at_checked(intermediates_len) else {
        return Step::End;
    };
    match rest.split_first() {
        None => Step::End,
        Some((&final_byte, after)) if (0x40..=0x7E).contains(&final_byte) => Step::Token(
            Token::Csi {
                params,
                intermediates,
                final_byte,
            },
            after,
        ),
        // Malformed: drop what was read and resume at the offending byte.
        Some(_) => Step::Skip(rest),
    }
}

/// Reads a string sequence (DCS, APC, OSC) from `input`, which starts
/// after its introducer, up to `ESC \` or BEL.
fn string<'a>(input: &'a [u8], make: impl Fn(&'a [u8]) -> Token<'a>) -> Step<'a> {
    for (i, &byte) in input.iter().enumerate() {
        let terminator_len = match byte {
            BEL => 1,
            ESC if input.get(i + 1) == Some(&b'\\') => 2,
            _ => continue,
        };
        return match (input.get(..i), input.get(i + terminator_len..)) {
            (Some(body), Some(rest)) => Step::Token(make(body), rest),
            _ => Step::End,
        };
    }
    Step::End
}

/// Records what `token` says in `replies`. Returns [`ReplyEnd::Da1`] for
/// the DA1 reply.
fn interpret(token: &Token<'_>, replies: &mut Replies) -> ReplyEnd {
    match *token {
        Token::Csi {
            params,
            intermediates: [],
            final_byte,
        } => match final_byte {
            b'c' if params.starts_with(b"?") => return ReplyEnd::Da1,
            b't' => {
                if let Some(cell_px) = cell_size(params) {
                    replies.cell_px = Some(cell_px);
                }
            }
            b'u' => {
                if let Some(flags) = params.strip_prefix(b"?").and_then(number) {
                    replies.keyboard_flags = Some(flags);
                }
            }
            _ => {}
        },
        Token::Apc(body) => {
            if let Some(reply) = kitty_reply(body) {
                replies.kitty_graphics = reply;
            }
        }
        Token::Dcs(body) => dcs(body, replies),
        Token::Csi { .. } | Token::Osc => {}
    }
    ReplyEnd::Incomplete
}

/// Parses `6 ; h ; w` into a cell size; zero or unparsable values give
/// `None`.
fn cell_size(params: &[u8]) -> Option<CellPx> {
    let mut parts = params.split(|&b| b == b';');
    if parts.next() != Some(b"6") {
        return None;
    }
    let height = parts.next().and_then(number)?;
    let width = parts.next().and_then(number)?;
    if parts.next().is_some() || height == 0 || width == 0 {
        return None;
    }
    Some(CellPx { width, height })
}

/// Parses an APC body `G<keys>;<message>`; only replies for image id 31
/// count.
fn kitty_reply(body: &[u8]) -> Option<KittyReply> {
    let body = body.strip_prefix(b"G")?;
    let separator = body.iter().position(|&b| b == b';')?;
    let (keys, message) = body.split_at_checked(separator)?;
    let message = message.get(1..)?;
    if !keys.split(|&b| b == b',').any(|key| key == b"i=31") {
        return None;
    }
    Some(if message == b"OK" {
        KittyReply::Ok
    } else {
        KittyReply::Error
    })
}

/// Records an XTVERSION or XTGETTCAP DCS reply.
fn dcs(body: &[u8], replies: &mut Replies) {
    if let Some(text) = body.strip_prefix(b">|") {
        replies.xtversion = Some(String::from_utf8_lossy(text).into_owned());
    } else if let Some(names) = body.strip_prefix(b"1+r") {
        if names_smulx(names) {
            replies.smulx = TcapReply::Supported;
        }
    } else if let Some(names) = body.strip_prefix(b"0+r")
        && names_smulx(names)
    {
        replies.smulx = TcapReply::Unsupported;
    }
}

/// Whether the `;`-separated XTGETTCAP `name[=value]` list names `Smulx`.
fn names_smulx(names: &[u8]) -> bool {
    names.split(|&b| b == b';').any(|entry| {
        entry
            .split(|&b| b == b'=')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case(SMULX_HEX))
    })
}

/// Parses ASCII decimal digits into a `u16`.
fn number(digits: &[u8]) -> Option<u16> {
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// Reply fixtures, one per terminal.
///
/// These are synthesised from each terminal's documented reply format,
/// not recorded from a live terminal. The manual smoke checklist records
/// real ones. `FREMINAL` is the exception: it is recorded.
#[cfg(test)]
pub(crate) mod fixtures {
    /// kitty.
    pub(crate) const KITTY: &[u8] = b"\x1b_Gi=31;OK\x1b\\\
        \x1b[6;20;10t\
        \x1bP>|kitty(0.39.1)\x1b\\\
        \x1bP1+r536d756c78=1b5b343a25703125646d\x1b\\\
        \x1b[?0u\
        \x1b[?62;c";

    /// Ghostty.
    pub(crate) const GHOSTTY: &[u8] = b"\x1b_Gi=31;OK\x1b\\\
        \x1b[6;17;8t\
        \x1bP>|ghostty 1.1.3\x1b\\\
        \x1bP1+r536d756c78=1b5b343a25703125646d\x1b\\\
        \x1b[?0u\
        \x1b[?62;22c";

    /// foot.
    pub(crate) const FOOT: &[u8] = b"\x1b[6;16;8t\
        \x1bP>|foot(1.20.2)\x1b\\\
        \x1bP1+r536d756c78=1b5b343a25703125646d\x1b\\\
        \x1b[?0u\
        \x1b[?62;4;22c";

    /// WezTerm.
    pub(crate) const WEZTERM: &[u8] = b"\x1b_Gi=31;OK\x1b\\\
        \x1b[6;18;9t\
        \x1bP>|WezTerm 20240203-110809-5046fc22\x1b\\\
        \x1bP0+r536d756c78\x1b\\\
        \x1b[?65;4;6;18;22c";

    /// freminal, recorded from freminal 0.13.0-beta.3.
    pub(crate) const FREMINAL: &[u8] = b"\x1b_Gi=31;OK\x1b\\\
        \x1b[6;20;10t\
        \x1bP>|XTerm(Freminal 0.13.0-beta.3)\x1b\\\
        \x1bP1+r536d756c78=1B5B343A25703125646D\x1b\\\
        \x1b[?0u\
        \x1b[?65;1;2;4;6;17;18;22c";

    /// Alacritty.
    pub(crate) const ALACRITTY: &[u8] = b"\x1b[?0u\x1b[?6c";

    /// tmux.
    pub(crate) const TMUX: &[u8] = b"\x1bP>|tmux 3.5a\x1b\\\x1b[?1;2c";
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use super::fixtures::{ALACRITTY, FOOT, FREMINAL, GHOSTTY, KITTY, TMUX, WEZTERM};
    use super::*;

    const TAIL: &[u8] = b"\x1b[16t\x1b[>q\x1bP+q536d756c78\x1b\\\x1b[?u\x1b[c";

    fn cell(width: u16, height: u16) -> Option<CellPx> {
        Some(CellPx { width, height })
    }

    #[test]
    fn query_without_multiplexer_is_the_plain_batch() {
        let mut expected = KITTY_QUERY.to_vec();
        expected.extend_from_slice(TAIL);
        assert_eq!(query(Multiplexer::None), expected);
    }

    #[test]
    fn query_under_tmux_wraps_only_the_kitty_query() {
        let out = query(Multiplexer::Tmux);
        let head = tmux::passthrough(KITTY_QUERY);
        assert!(out.starts_with(&head));
        assert_eq!(out.get(head.len()..), Some(TAIL));
    }

    #[test]
    fn kitty_fixture() {
        assert_eq!(
            parse(KITTY),
            Replies {
                kitty_graphics: KittyReply::Ok,
                cell_px: cell(10, 20),
                xtversion: Some("kitty(0.39.1)".to_owned()),
                smulx: TcapReply::Supported,
                keyboard_flags: Some(0),
                end: ReplyEnd::Da1,
            }
        );
    }

    #[test]
    fn ghostty_fixture() {
        assert_eq!(
            parse(GHOSTTY),
            Replies {
                kitty_graphics: KittyReply::Ok,
                cell_px: cell(8, 17),
                xtversion: Some("ghostty 1.1.3".to_owned()),
                smulx: TcapReply::Supported,
                keyboard_flags: Some(0),
                end: ReplyEnd::Da1,
            }
        );
    }

    #[test]
    fn foot_fixture() {
        assert_eq!(
            parse(FOOT),
            Replies {
                kitty_graphics: KittyReply::Missing,
                cell_px: cell(8, 16),
                xtversion: Some("foot(1.20.2)".to_owned()),
                smulx: TcapReply::Supported,
                keyboard_flags: Some(0),
                end: ReplyEnd::Da1,
            }
        );
    }

    #[test]
    fn wezterm_fixture() {
        assert_eq!(
            parse(WEZTERM),
            Replies {
                kitty_graphics: KittyReply::Ok,
                cell_px: cell(9, 18),
                xtversion: Some("WezTerm 20240203-110809-5046fc22".to_owned()),
                smulx: TcapReply::Unsupported,
                keyboard_flags: None,
                end: ReplyEnd::Da1,
            }
        );
    }

    #[test]
    fn freminal_fixture() {
        assert_eq!(
            parse(FREMINAL),
            Replies {
                kitty_graphics: KittyReply::Ok,
                cell_px: cell(10, 20),
                xtversion: Some("XTerm(Freminal 0.13.0-beta.3)".to_owned()),
                smulx: TcapReply::Supported,
                keyboard_flags: Some(0),
                end: ReplyEnd::Da1,
            }
        );
    }

    #[test]
    fn alacritty_fixture() {
        assert_eq!(
            parse(ALACRITTY),
            Replies {
                kitty_graphics: KittyReply::Missing,
                cell_px: None,
                xtversion: None,
                smulx: TcapReply::Missing,
                keyboard_flags: Some(0),
                end: ReplyEnd::Da1,
            }
        );
    }

    #[test]
    fn tmux_fixture() {
        assert_eq!(
            parse(TMUX),
            Replies {
                kitty_graphics: KittyReply::Missing,
                cell_px: None,
                xtversion: Some("tmux 3.5a".to_owned()),
                smulx: TcapReply::Missing,
                keyboard_flags: None,
                end: ReplyEnd::Da1,
            }
        );
    }

    #[test]
    fn empty_input_is_the_default() {
        assert_eq!(parse(b""), Replies::default());
    }

    #[test]
    fn bel_terminated_xtversion_parses() {
        let replies = parse(b"\x1bP>|foot(1.20.2)\x07");
        assert_eq!(replies.xtversion.as_deref(), Some("foot(1.20.2)"));
    }

    #[test]
    fn kitty_error_reply_is_error() {
        let replies = parse(b"\x1b_Gi=31;ENOTSUPPORTED:no\x1b\\");
        assert_eq!(replies.kitty_graphics, KittyReply::Error);
    }

    #[test]
    fn kitty_reply_for_another_id_is_ignored() {
        let replies = parse(b"\x1b_Gi=99;OK\x1b\\");
        assert_eq!(replies.kitty_graphics, KittyReply::Missing);
    }

    #[test]
    fn zero_cell_size_is_ignored() {
        assert_eq!(parse(b"\x1b[6;0;0t").cell_px, None);
    }

    #[test]
    fn uppercase_smulx_name_is_supported() {
        let replies = parse(b"\x1bP1+r536D756C78=1b5b343a25703125646d\x1b\\");
        assert_eq!(replies.smulx, TcapReply::Supported);
    }

    #[test]
    fn noise_before_a_reply_is_skipped() {
        let replies = parse(b"xy\x1b[?7u");
        assert_eq!(replies.keyboard_flags, Some(7));
    }

    #[test]
    fn bytes_after_da1_are_ignored() {
        let mut input = KITTY.to_vec();
        input.extend_from_slice(b"x\x1b[?5u");
        let replies = parse(&input);
        assert_eq!(replies.keyboard_flags, Some(0));
        assert_eq!(replies.end, ReplyEnd::Da1);
    }

    #[test]
    fn input_cut_off_mid_sequence_is_incomplete() {
        let cut = KITTY.get(..KITTY.len() - 3).unwrap_or_default();
        assert_eq!(
            parse(cut),
            Replies {
                kitty_graphics: KittyReply::Ok,
                cell_px: cell(10, 20),
                xtversion: Some("kitty(0.39.1)".to_owned()),
                smulx: TcapReply::Supported,
                keyboard_flags: Some(0),
                end: ReplyEnd::Incomplete,
            }
        );
    }

    #[test]
    fn read_replies_stops_at_da1() {
        let (reader, mut writer) = io::pipe().unwrap();
        writer.write_all(KITTY).unwrap();
        let start = Instant::now();
        let bytes = read_replies(&reader, Duration::from_secs(60)).unwrap();
        assert_eq!(bytes, KITTY);
        assert!(start.elapsed() < Duration::from_secs(30));
        drop(writer);
    }

    #[test]
    fn read_replies_on_an_empty_pipe_times_out_empty() {
        let (reader, _writer) = io::pipe().unwrap();
        let bytes = read_replies(&reader, Duration::from_millis(50)).unwrap();
        assert_eq!(bytes, Vec::<u8>::new());
    }

    #[test]
    fn read_replies_stops_at_end_of_file() {
        let (reader, mut writer) = io::pipe().unwrap();
        writer.write_all(b"\x1b[?1u").unwrap();
        drop(writer);
        let start = Instant::now();
        let bytes = read_replies(&reader, Duration::from_secs(60)).unwrap();
        assert_eq!(bytes, b"\x1b[?1u");
        assert!(start.elapsed() < Duration::from_secs(30));
    }

    #[test]
    fn probe_writes_the_query_and_parses_the_replies() {
        let (reader, mut writer) = io::pipe().unwrap();
        writer.write_all(FREMINAL).unwrap();
        let mut out = Vec::new();
        let replies = probe(
            &mut out,
            &reader,
            Multiplexer::None,
            Duration::from_secs(60),
        )
        .unwrap();
        assert_eq!(out, query(Multiplexer::None));
        assert_eq!(replies, parse(FREMINAL));
        drop(writer);
    }
}
