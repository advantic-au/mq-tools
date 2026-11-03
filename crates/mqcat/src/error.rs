//! Error types and exit-code mapping for `mqcat`.
//!
//! kcat reserves exit code 0 for success and 1 for failures. Additional codes
//! distinguish usage errors from runtime failures so scripts can react without
//! parsing stderr.

use std::process::ExitCode;

/// Exit code returned when the command line could not be understood.
pub const EXIT_USAGE: u8 = 2;

/// Failure modes surfaced to the user.
#[derive(Debug, thiserror::Error)]
pub enum MqCatError {
    /// Neither `--queue` nor `--topic` was supplied.
    #[error("a target is required: pass --queue or --topic")]
    MissingTarget,

    /// Both `--queue` and `--topic` were supplied.
    #[error("--queue and --topic are mutually exclusive")]
    ConflictingTargets,

    /// A flag was combined with a mode that cannot use it.
    #[error("{0}")]
    InvalidOption(&'static str),

    /// A queue or topic name is not valid for MQ.
    #[error("invalid name `{name}`")]
    InvalidTargetName {
        /// The name as supplied on the command line.
        name: String,
        /// The underlying length or encoding error.
        #[source]
        source: mqi::string::MqStrError,
    },

    /// A channel server string was not in `CHANNEL/TRANSPORT/HOST(PORT)` form.
    #[error("invalid server `{value}`")]
    InvalidServer {
        /// The server argument as supplied.
        value: String,
        /// The MQ server syntax error.
        #[source]
        source: mqi::connection::MqServerSyntaxError,
    },

    /// A TLS option could not be represented in MQ's fixed-width fields.
    #[error("invalid value for --{option}")]
    InvalidTlsOption {
        /// Name of the offending option.
        option: &'static str,
        /// The underlying length or encoding error.
        #[source]
        source: mqi::string::MqStrError,
    },

    /// A message format name could not be represented in MQ's format field.
    #[error("invalid message format `{name}`")]
    InvalidFormatName {
        /// The format name as supplied.
        name: String,
        /// The underlying length or encoding error.
        #[source]
        source: mqi::string::MqStrError,
    },

    /// An argument containing escape sequences could not be parsed.
    #[error("invalid delimiter `{value}`")]
    InvalidDelimiter {
        /// The delimiter as supplied on the command line.
        value: String,
        /// The offending escape sequence.
        #[source]
        source: crate::delimiters::EscapeError,
    },

    /// An identifier argument was not valid hexadecimal.
    #[error("invalid hexadecimal identifier `{value}`")]
    InvalidIdentifier {
        /// The identifier as supplied on the command line.
        value: String,
        /// The decode error.
        #[source]
        source: IdentifierError,
    },

    /// A property argument was not in `name=value` form.
    #[error("invalid property `{0}`: expected `name=value`")]
    InvalidProperty(String),

    /// The format string could not be interpreted.
    #[error("invalid format string")]
    InvalidFormat {
        /// The parse failure.
        #[source]
        source: crate::format::FormatError,
    },

    /// The IBM MQ client library could not be loaded at run time.
    #[error("unable to load the IBM MQ client library")]
    LoadLibrary(#[source] dlopen2::Error),

    /// The connection could not be established.
    #[error("unable to connect to the queue manager")]
    Connect(#[source] mqi::result::Error),

    /// The object could not be opened.
    #[error("unable to open {target}")]
    Open {
        /// The target being opened.
        target: String,
        /// The underlying MQ error.
        #[source]
        source: mqi::result::Error,
    },

    /// An MQ operation failed after the object was open.
    #[error("{operation} failed")]
    Mq {
        /// Human-readable description of the failed operation.
        operation: &'static str,
        /// The underlying MQ error.
        #[source]
        source: mqi::result::Error,
    },

    /// Reading the input stream failed.
    #[error("unable to read input: {0}")]
    Read(#[source] std::io::Error),

    /// Writing output failed.
    #[error("unable to write output: {0}")]
    Write(#[source] std::io::Error),

    /// A file named on the command line could not be read.
    #[error("unable to read file `{path}`: {source}")]
    ReadFile {
        /// The file that could not be read.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

impl MqCatError {
    /// Map the failure to a process exit code.
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::MissingTarget | Self::ConflictingTargets | Self::InvalidOption(_) => ExitCode::from(EXIT_USAGE),
            Self::InvalidTargetName { .. }
            | Self::InvalidServer { .. }
            | Self::InvalidTlsOption { .. }
            | Self::InvalidFormatName { .. }
            | Self::InvalidDelimiter { .. }
            | Self::InvalidIdentifier { .. }
            | Self::InvalidProperty(_)
            | Self::InvalidFormat { .. } => ExitCode::from(EXIT_USAGE),
            Self::LoadLibrary(_)
            | Self::Connect(_)
            | Self::Open { .. }
            | Self::Mq { .. }
            | Self::Read(_)
            | Self::Write(_)
            | Self::ReadFile { .. } => ExitCode::FAILURE,
        }
    }
}

/// Failure while decoding a hexadecimal message identifier.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdentifierError {
    /// The identifier contained an odd number of digits.
    #[error("expected an even number of hexadecimal digits")]
    OddLength,
    /// The identifier contained a character that is not a hexadecimal digit.
    #[error("`{0}` is not a valid hexadecimal digit")]
    NotHex(char),
    /// The identifier was longer than the 24 bytes available in the message descriptor.
    #[error("identifier is {0} bytes but at most 24 are allowed")]
    TooLong(usize),
}

/// Decode a hexadecimal identifier into the fixed-width message-descriptor form.
///
/// # Errors
/// Returns an error when the input is not an even-length hexadecimal string of
/// at most 24 bytes.
pub fn parse_identifier(value: &str) -> Result<[u8; 24], IdentifierError> {
    let digits: Vec<char> = value.chars().filter(|c| !c.is_whitespace() && *c != ':').collect();
    if !digits.len().is_multiple_of(2) {
        return Err(IdentifierError::OddLength);
    }

    let mut bytes = [0u8; 24];
    let count = digits.len() / 2;
    if count > 24 {
        return Err(IdentifierError::TooLong(count));
    }

    for (index, pair) in digits.as_chunks::<2>().0.iter().enumerate() {
        let high = pair[0].to_digit(16).ok_or(IdentifierError::NotHex(pair[0]))?;
        let low = pair[1].to_digit(16).ok_or(IdentifierError::NotHex(pair[1]))?;
        // Both nibbles are < 16, so the cast cannot lose information.
        #[expect(clippy::cast_possible_truncation, reason = "nibble values are at most 15")]
        {
            bytes[index] = ((high << 4) | low) as u8;
        }
    }

    Ok(bytes)
}

/// Print an error and its causes to stderr in a compact single-block form.
pub fn report(err: &MqCatError) {
    use std::error::Error as _;

    eprintln!("mqcat: {err}");
    let mut source = err.source();
    while let Some(cause) = source {
        eprintln!("  caused by: {cause}");
        source = cause.source();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_decodes_hex() {
        let id = parse_identifier("414d5120").unwrap();
        assert_eq!(&id[..4], b"AMQ ");
        assert_eq!(&id[4..], &[0u8; 20]);
    }

    #[test]
    fn identifier_ignores_separators() {
        assert_eq!(parse_identifier("41:4d 51").unwrap(), parse_identifier("414d51").unwrap());
    }

    #[test]
    fn identifier_rejects_odd_length() {
        assert_eq!(parse_identifier("414"), Err(IdentifierError::OddLength));
    }

    #[test]
    fn identifier_rejects_non_hex() {
        assert_eq!(parse_identifier("41zz"), Err(IdentifierError::NotHex('z')));
    }

    #[test]
    fn identifier_rejects_overlong() {
        let long = "41".repeat(25);
        assert_eq!(parse_identifier(&long), Err(IdentifierError::TooLong(25)));
    }

    #[test]
    fn usage_errors_exit_with_two() {
        assert_eq!(MqCatError::MissingTarget.exit_code(), ExitCode::from(2));
    }
}
