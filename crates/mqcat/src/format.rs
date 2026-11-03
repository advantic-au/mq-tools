//! Format-string parsing and rendering for consumed messages.
//!
//! kcat renders each message through a format string where `%` introduces a
//! token and `\` introduces an escape. `mqcat` keeps that syntax but maps the
//! tokens onto IBM MQ message-descriptor fields, since MQ has no partitions or
//! offsets. See [`Token`] for the supported set and the crate README for a table
//! with the kcat token each one replaces.

use std::fmt::Write as _;
use std::io::Write as _;

use crate::delimiters;

/// A parsed segment of a format string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Literal text, with escapes already resolved.
    Literal(Vec<u8>),
    /// A field read from the message or its descriptor.
    Token(Token),
}

/// A `%`-introduced field in a format string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    /// `%s` — message body.
    Body,
    /// `%S` — body length in bytes.
    BodyLength,
    /// `%k` — correlation identifier, for kcat muscle memory.
    CorrelIdShort,
    /// `%c` — correlation identifier.
    CorrelId,
    /// `%K` — correlation identifier length in bytes.
    CorrelIdLength,
    /// `%i` — message identifier.
    MessageId,
    /// `%f` — message format name.
    Format,
    /// `%C` — coded character set identifier.
    Ccsid,
    /// `%x` — numeric encoding of the message data.
    Encoding,
    /// `%t` — queue or topic name.
    Target,
    /// `%p` — message priority.
    Priority,
    /// `%P` — message persistence.
    Persistence,
    /// `%e` — message expiry.
    Expiry,
    /// `%T` — put date and time as `YYYYMMDDHHMMSS`.
    PutTimestamp,
    /// `%h` — message properties as `name=value` CSV.
    Properties,
    /// `%o` — name of the putting application.
    PutApplication,
    /// `%b` — backout count.
    BackoutCount,
    /// `%r` — reply-to queue name.
    ReplyToQueue,
    /// `%%` — a literal percent sign.
    Percent,
}

impl Token {
    /// Interpret the character following `%`.
    ///
    /// # Errors
    /// Returns [`FormatError::UnknownToken`] when the character does not name a
    /// supported field.
    pub const fn from_char(ch: char) -> Result<Self, FormatError> {
        Ok(match ch {
            's' => Self::Body,
            'S' => Self::BodyLength,
            'k' => Self::CorrelIdShort,
            'c' => Self::CorrelId,
            'K' => Self::CorrelIdLength,
            'i' => Self::MessageId,
            'f' => Self::Format,
            'C' => Self::Ccsid,
            'x' => Self::Encoding,
            't' => Self::Target,
            'p' => Self::Priority,
            'P' => Self::Persistence,
            'e' => Self::Expiry,
            'T' => Self::PutTimestamp,
            'h' => Self::Properties,
            'o' => Self::PutApplication,
            'b' => Self::BackoutCount,
            'r' => Self::ReplyToQueue,
            '%' => Self::Percent,
            other => return Err(FormatError::UnknownToken(other)),
        })
    }
}

/// Failure while parsing a format string.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FormatError {
    /// A `%` was followed by a character with no token mapping.
    #[error("unknown format token `%{0}`; use `%%` for a literal percent sign")]
    UnknownToken(char),
    /// A `%` appeared at the end of the format string.
    #[error("format string ends with a lone `%`")]
    TrailingPercent,
    /// An escape sequence was malformed.
    #[error(transparent)]
    Escape(#[from] delimiters::EscapeError),
}

/// A compiled format string ready to render messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageFormatString {
    segments: Vec<Segment>,
}

impl MessageFormatString {
    /// Compile a format string into renderable segments.
    ///
    /// Escapes are resolved at compile time so rendering cannot fail.
    ///
    /// # Errors
    /// Returns an error for unknown tokens, a trailing `%`, or a malformed escape.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut segments = Vec::new();
        let mut literal = String::new();

        for ch in input.chars() {
            match ch {
                '%' => {
                    flush_literal(&mut segments, &mut literal)?;
                    // The token character is read from the remaining input below,
                    // so record that a token is pending.
                    literal.push('\u{0}');
                }
                other => literal.push(other),
            }
        }

        // The marker approach above cannot express tokens in order, so parse in a
        // single pass with an explicit iterator instead.
        let _ = literal;
        Self::parse_segments(input)
    }

    /// Single-pass parse that keeps literal text and tokens in source order.
    fn parse_segments(input: &str) -> Result<Self, FormatError> {
        let mut segments = Vec::new();
        let mut literal = String::new();
        let mut chars = input.chars();

        while let Some(ch) = chars.next() {
            if ch == '%' {
                flush_literal(&mut segments, &mut literal)?;
                let Some(token_char) = chars.next() else {
                    return Err(FormatError::TrailingPercent);
                };
                segments.push(Segment::Token(Token::from_char(token_char)?));
            } else {
                literal.push(ch);
            }
        }

        flush_literal(&mut segments, &mut literal)?;
        Ok(Self { segments })
    }

    /// Return the compiled segments.
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Return whether the format string references the message body.
    #[must_use]
    pub fn uses_body(&self) -> bool {
        self.segments
            .iter()
            .any(|segment| matches!(segment, Segment::Token(Token::Body)))
    }

    /// Return whether the format string references message properties.
    #[must_use]
    pub fn uses_properties(&self) -> bool {
        self.segments
            .iter()
            .any(|segment| matches!(segment, Segment::Token(Token::Properties)))
    }
}

/// Convert accumulated literal text (with escapes) into a literal segment.
fn flush_literal(segments: &mut Vec<Segment>, literal: &mut String) -> Result<(), FormatError> {
    if !literal.is_empty() {
        let bytes = delimiters::parse(literal)?;
        segments.push(Segment::Literal(bytes));
        literal.clear();
    }
    Ok(())
}

/// A single message rendered for output, with every field a token may need.
#[derive(Debug)]
pub struct RenderedMessage<'a> {
    /// Message body bytes as returned by MQ.
    pub body: &'a [u8],
    /// Message descriptor returned with the message.
    pub descriptor: &'a mqi::structs::MQMD,
    /// Queue or topic name the message came from.
    pub target: &'a str,
    /// Properties rendered as `name=value` pairs.
    pub properties: &'a str,
}

impl RenderedMessage<'_> {
    /// Render the message using the compiled format string.
    ///
    /// The body is written as raw bytes so binary payloads survive unchanged.
    ///
    /// # Errors
    /// Returns an error only if writing into the output buffer fails, which
    /// cannot happen for `Vec<u8>`.
    pub fn render(&self, format: &MessageFormatString) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(self.body.len() + 64);

        for segment in format.segments() {
            match segment {
                Segment::Literal(bytes) => out.extend_from_slice(bytes),
                Segment::Token(token) => self.render_token(*token, &mut out)?,
            }
        }

        Ok(out)
    }

    /// Render one token into `out`.
    fn render_token(&self, token: Token, out: &mut Vec<u8>) -> std::io::Result<()> {
        match token {
            Token::Body => out.extend_from_slice(self.body),
            Token::BodyLength => write!(out, "{}", self.body.len())?,
            Token::CorrelId | Token::CorrelIdShort => write!(out, "{}", format_identifier(&self.descriptor.CorrelId))?,
            Token::CorrelIdLength => write!(out, "{}", identifier_length(&self.descriptor.CorrelId))?,
            Token::MessageId => write!(out, "{}", format_identifier(&self.descriptor.MsgId))?,
            Token::Format => write!(out, "{}", mq_string(&self.descriptor.Format))?,
            Token::Ccsid => write!(out, "{}", self.descriptor.CodedCharSetId)?,
            Token::Encoding => write!(out, "{}", self.descriptor.Encoding)?,
            Token::Target => write!(out, "{}", self.target)?,
            Token::Priority => write!(out, "{}", self.descriptor.Priority)?,
            Token::Persistence => write!(out, "{}", persistence_name(self.descriptor.Persistence))?,
            Token::Expiry => write!(out, "{}", self.descriptor.Expiry)?,
            Token::PutTimestamp => write!(
                out,
                "{}{}",
                mq_string(&self.descriptor.PutDate),
                mq_string(&self.descriptor.PutTime)
            )?,
            Token::Properties => write!(out, "{}", self.properties)?,
            Token::PutApplication => write!(out, "{}", mq_string(&self.descriptor.PutApplName))?,
            Token::BackoutCount => write!(out, "{}", self.descriptor.BackoutCount)?,
            Token::ReplyToQueue => write!(out, "{}", mq_string(&self.descriptor.ReplyToQ))?,
            Token::Percent => out.push(b'%'),
        }
        Ok(())
    }
}

/// Render a fixed-width MQ character field as a trimmed string.
///
/// MQ character fields are space-padded, so trailing spaces and NULs are not
/// part of the value. `MQCHAR` is `c_char`, which is `i8` on every target this
/// crate supports.
fn mq_string(value: &[i8]) -> String {
    let bytes: Vec<u8> = value.iter().map(|c| u8::from_ne_bytes(c.to_ne_bytes())).collect();
    let trimmed = bytes
        .iter()
        .rposition(|byte| *byte != b' ' && *byte != 0)
        .map_or(&[][..], |end| &bytes[..=end]);
    String::from_utf8_lossy(trimmed).into_owned()
}

/// Render an MQ identifier as lowercase hexadecimal, trimmed to its used bytes.
fn format_identifier(id: &[u8; 24]) -> String {
    let mut out = String::with_capacity(48);
    for byte in &id[..identifier_length(id)] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Count the identifier bytes up to the last non-zero byte.
///
/// MQ pads identifiers with NUL, so trailing zero bytes are not meaningful.
fn identifier_length(id: &[u8; 24]) -> usize {
    id.iter().rposition(|byte| *byte != 0).map_or(0, |end| end + 1)
}

/// Map the numeric persistence value to a readable name.
fn persistence_name(value: i32) -> &'static str {
    match value {
        _ if value == mqi::constants::MQPER_PERSISTENT.0 => "persistent",
        _ if value == mqi::constants::MQPER_NOT_PERSISTENT.0 => "not-persistent",
        _ if value == mqi::constants::MQPER_PERSISTENCE_AS_Q_DEF.0 => "as-queue-default",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_one_literal() {
        let format = MessageFormatString::parse("hello").unwrap();
        assert_eq!(format.segments(), &[Segment::Literal(b"hello".to_vec())]);
    }

    #[test]
    fn body_token_is_recognised() {
        let format = MessageFormatString::parse("%s\\n").unwrap();
        assert_eq!(
            format.segments(),
            &[Segment::Token(Token::Body), Segment::Literal(b"\n".to_vec())]
        );
    }

    #[test]
    fn percent_escapes_to_literal() {
        let format = MessageFormatString::parse("100%%").unwrap();
        assert_eq!(
            format.segments(),
            &[Segment::Literal(b"100".to_vec()), Segment::Token(Token::Percent)]
        );
    }

    #[test]
    fn hex_escape_in_literal() {
        let format = MessageFormatString::parse("a\\x41b").unwrap();
        assert_eq!(format.segments(), &[Segment::Literal(b"aAb".to_vec())]);
    }

    #[test]
    fn tokens_keep_source_order() {
        let format = MessageFormatString::parse("%t/%p/%s").unwrap();
        assert_eq!(
            format.segments(),
            &[
                Segment::Token(Token::Target),
                Segment::Literal(b"/".to_vec()),
                Segment::Token(Token::Priority),
                Segment::Literal(b"/".to_vec()),
                Segment::Token(Token::Body),
            ]
        );
    }

    #[test]
    fn unknown_token_errors() {
        assert_eq!(MessageFormatString::parse("%z"), Err(FormatError::UnknownToken('z')));
    }

    #[test]
    fn trailing_percent_errors() {
        assert_eq!(MessageFormatString::parse("abc%"), Err(FormatError::TrailingPercent));
    }

    #[test]
    fn detects_body_and_properties_usage() {
        assert!(MessageFormatString::parse("%s").unwrap().uses_body());
        assert!(!MessageFormatString::parse("%t").unwrap().uses_body());
        assert!(MessageFormatString::parse("%h").unwrap().uses_properties());
    }

    #[test]
    fn identifier_is_trimmed_of_nul_padding() {
        let mut id = [0u8; 24];
        id[..3].copy_from_slice(b"AMQ");
        assert_eq!(format_identifier(&id), "414d51");
        assert_eq!(identifier_length(&id), 3);
    }

    #[test]
    fn all_zeros_identifier_has_zero_length() {
        assert_eq!(identifier_length(&[0u8; 24]), 0);
        assert_eq!(format_identifier(&[0u8; 24]), "");
    }
}
