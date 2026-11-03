//! Escape-sequence parsing for delimiter and separator arguments.
//!
//! kcat interprets escapes in `-K`/`-D` style arguments so that callers can
//! supply control characters without embedding them literally in a shell
//! argument. This module reproduces that behaviour, supporting the sequences
//! `\n`, `\r`, `\t`, `\0`, `\\`, `\xNN`, and `\NNN`.

/// Parse escape sequences in `input` into their byte representation.
///
/// A trailing lone backslash is preserved literally rather than being dropped,
/// so malformed input round-trips instead of silently losing a character.
///
/// # Errors
/// Returns an error describing the offending sequence when a hexadecimal or
/// octal escape is malformed or out of range.
pub fn parse(input: &str) -> Result<Vec<u8>, EscapeError> {
    let mut out = Vec::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            continue;
        }

        let Some(escaped) = chars.next() else {
            // A trailing backslash has nothing to escape; keep it verbatim.
            out.push(b'\\');
            break;
        };

        match escaped {
            'n' => out.push(b'\n'),
            'r' => out.push(b'\r'),
            't' => out.push(b'\t'),
            '0' => out.push(0),
            '\\' => out.push(b'\\'),
            'x' | 'X' => {
                let digits: String = std::iter::from_fn(|| chars.next_if(char::is_ascii_hexdigit))
                    .take(2)
                    .collect();
                if digits.is_empty() {
                    return Err(EscapeError::MissingHexDigits);
                }
                let value = u8::from_str_radix(&digits, 16).map_err(|_| EscapeError::OutOfRange(digits.clone()))?;
                out.push(value);
            }
            '0'..='7' => {
                let mut digits = String::from(escaped);
                digits.extend(std::iter::from_fn(|| chars.next_if(|c| ('0'..='7').contains(c))).take(2));
                let value = u8::from_str_radix(&digits, 8).map_err(|_| EscapeError::OutOfRange(digits.clone()))?;
                out.push(value);
            }
            other => {
                // Unknown escapes are treated as the literal character, matching
                // kcat's permissive handling.
                let mut buf = [0u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
        }
    }

    Ok(out)
}

/// Failure while interpreting an escape sequence.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EscapeError {
    /// A `\x` escape was not followed by any hexadecimal digit.
    #[error("`\\x` escape requires at least one hexadecimal digit")]
    MissingHexDigits,
    /// A hexadecimal or octal escape exceeded the range of a byte.
    #[error("escape value `{0}` is outside the range of a single byte")]
    OutOfRange(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_copied() {
        assert_eq!(parse("hello").unwrap(), b"hello");
    }

    #[test]
    fn control_escapes_are_interpreted() {
        assert_eq!(parse(r"a\nb\tc\rd").unwrap(), b"a\nb\tc\rd");
    }

    #[test]
    fn hex_escapes_are_interpreted() {
        assert_eq!(parse(r"\x41\x42").unwrap(), b"AB");
        assert_eq!(parse(r"\x0a").unwrap(), b"\n");
    }

    #[test]
    fn octal_escapes_are_interpreted() {
        assert_eq!(parse(r"\101\102").unwrap(), b"AB");
    }

    #[test]
    fn backslash_escapes_itself() {
        assert_eq!(parse(r"a\\b").unwrap(), br"a\b");
    }

    #[test]
    fn trailing_backslash_is_preserved() {
        assert_eq!(parse("a\\").unwrap(), br"a\");
    }

    #[test]
    fn unknown_escape_is_literal() {
        assert_eq!(parse(r"a\qb").unwrap(), b"aqb");
    }

    #[test]
    fn hex_without_digits_errors() {
        assert_eq!(parse(r"\xzz"), Err(EscapeError::MissingHexDigits));
    }

    #[test]
    fn multibyte_characters_are_utf8_encoded() {
        assert_eq!(parse("é").unwrap(), "é".as_bytes());
    }
}
