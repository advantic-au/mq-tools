//! Producer mode: read input and put messages on the target.

use std::io::{self, Write as _};

use mqi::{constants, prelude::*, types::MessageFormat};

use crate::{
    cli::{Cli, Persistence},
    connection::{self, Connection},
    error::MqCatError,
    target::Target,
};

/// Run producer mode.
///
/// Reads messages either as whole files or as stdin split on the configured
/// delimiter, then puts each one on the target. kcat's `-c` count limit and
/// `-T` tee behaviour are both honoured.
///
/// # Errors
/// Returns an error when the connection, open, or put fails, or when the input
/// cannot be read.
pub fn run(cli: &Cli, target: &Target) -> Result<(), MqCatError> {
    let delimiter = crate::delimiters::parse(&cli.delimiter).map_err(|source| MqCatError::InvalidDelimiter {
        value: cli.delimiter.clone(),
        source,
    })?;
    if delimiter.is_empty() {
        return Err(MqCatError::InvalidOption("--delimiter must not be empty in producer mode"));
    }

    let key_delimiter = cli
        .key_delimiter
        .as_deref()
        .map(crate::delimiters::parse)
        .transpose()
        .map_err(|source| MqCatError::InvalidDelimiter {
            value: cli.key_delimiter.clone().unwrap_or_default(),
            source,
        })?;

    let properties = parse_properties(cli)?;
    let message_format = parse_message_format(cli)?;
    let access = connection::access_options(constants::MQOO_OUTPUT);

    let connection = connection::connect(cli)?;
    let object = mqi::Object::open(connection, &connection::open_options(target, access))
        .warn_as_error()
        .map_err(|source| MqCatError::Open {
            target: target.display_name(),
            source,
        })?;

    let mut produced = 0usize;

    if cli.files.is_empty() {
        let mut input = io::stdin().lock();
        let mut reader = DelimitedReader::new(&mut input, &delimiter);

        while let Some(message) = reader.next_message().map_err(MqCatError::Read)? {
            if message.is_empty() {
                continue;
            }
            put(&object, cli, message, key_delimiter.as_deref(), &properties, message_format)?;
            produced += 1;
            if reached_limit(cli, produced) {
                return Ok(());
            }
        }
    } else {
        for path in &cli.files {
            let bytes = std::fs::read(path).map_err(|source| MqCatError::ReadFile {
                path: path.display().to_string(),
                source,
            })?;
            if bytes.is_empty() {
                continue;
            }
            put(&object, cli, &bytes, key_delimiter.as_deref(), &properties, message_format)?;
            produced += 1;
            if reached_limit(cli, produced) {
                return Ok(());
            }
        }
    }

    object.close().warn_as_error().map_err(|source| MqCatError::Mq {
        operation: "MQCLOSE",
        source,
    })?;

    Ok(())
}

/// Return whether the configured message count has been reached.
fn reached_limit(cli: &Cli, produced: usize) -> bool {
    cli.count.is_some_and(|limit| produced >= limit)
}

/// Split a message into its key and body when `-K` is configured.
///
/// The key maps to the MQ correlation identifier, matching how kcat treats a
/// message key as the value that identifies related messages.
fn split_key<'a>(message: &'a [u8], key_delimiter: Option<&[u8]>) -> (Option<&'a [u8]>, &'a [u8]) {
    let Some(delimiter) = key_delimiter.filter(|d| !d.is_empty()) else {
        return (None, message);
    };

    match find_subslice(message, delimiter) {
        Some(at) => (Some(&message[..at]), &message[at + delimiter.len()..]),
        None => (None, message),
    }
}

/// Find the first occurrence of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// Put a single message, applying the configured descriptor options.
fn put(
    object: &mqi::Object<Connection>,
    cli: &Cli,
    message: &[u8],
    key_delimiter: Option<&[u8]>,
    properties: &[(&str, String)],
    message_format: MessageFormat,
) -> Result<(), MqCatError> {
    let (key, body) = split_key(message, key_delimiter);

    // kcat treats the message key as the field that correlates related messages.
    // The closest MQ equivalent is the correlation identifier, so a key present in
    // the input becomes the CorrelId. Keys longer than 24 bytes cannot be
    // represented and are rejected rather than silently truncated.
    let correl_id = key.map(correl_id_from_key).transpose()?;

    let priority = cli.priority.map(mqi::types::MQPRI);
    let persistence = match cli.persistence {
        Persistence::Yes => Some(mqi::types::MQPER(constants::MQPER_PERSISTENT.0)),
        Persistence::No => Some(mqi::types::MQPER(constants::MQPER_NOT_PERSISTENT.0)),
        Persistence::AsQueueDefault => None,
    };
    let expiry = Some(mqi::types::MQEI(parse_expiry(cli)?));
    let reply_to = cli
        .reply_to
        .as_deref()
        .map(str::parse::<mqi::types::ReplyToQueueName>)
        .transpose()
        .map_err(|source| MqCatError::InvalidTargetName {
            name: cli.reply_to.clone().unwrap_or_default(),
            source,
        })?;

    // Message properties are applied through a message handle when present.
    if properties.is_empty() {
        object
            .put_message(&(priority, persistence, expiry, reply_to, correl_id), &(body, message_format))
            .warn_as_error()
            .map_err(|source| MqCatError::Mq {
                operation: "MQPUT",
                source,
            })?;
    } else {
        let mut handle =
            mqi::Properties::new(object.connection().connection_ref(), mqi::types::MQCMHO::default()).map_err(|source| {
                MqCatError::Mq {
                    operation: "MQCRTMH",
                    source,
                }
            })?;

        for (name, value) in properties {
            handle
                .set_property(*name, value.as_str(), constants::MQSMPO_SET_FIRST)
                .warn_as_error()
                .map_err(|source| MqCatError::Mq {
                    operation: "MQSETMP",
                    source,
                })?;
        }

        object
            .put_message(
                &(priority, persistence, expiry, reply_to, correl_id, &mut handle),
                &(body, message_format),
            )
            .warn_as_error()
            .map_err(|source| MqCatError::Mq {
                operation: "MQPUT",
                source,
            })?;

        handle.close().map_err(|source| MqCatError::Mq {
            operation: "MQDLTMH",
            source,
        })?;
    }

    if cli.tee {
        let mut stdout = io::stdout().lock();
        stdout.write_all(body).map_err(MqCatError::Write)?;
        stdout.write_all(&delimiter_bytes(cli)).map_err(MqCatError::Write)?;
        stdout.flush().map_err(MqCatError::Write)?;
    }

    Ok(())
}

/// Convert an input key into an MQ correlation identifier.
///
/// # Errors
/// Returns an error when the key is longer than the 24 bytes an MQ identifier
/// can hold, since truncating would silently change the correlation value.
fn correl_id_from_key(key: &[u8]) -> Result<mqi::types::CorrelationId, MqCatError> {
    let mut identifier = [0u8; 24];
    if key.len() > identifier.len() {
        return Err(MqCatError::InvalidOption(
            "message key is longer than the 24 bytes available for a correlation identifier",
        ));
    }
    identifier[..key.len()].copy_from_slice(key);
    Ok(mqi::types::CorrelationId(identifier))
}

/// Re-parse the output delimiter for tee output.
///
/// Validation already rejected an unparsable delimiter, so the fallback to a
/// newline is only a defensive measure.
fn delimiter_bytes(cli: &Cli) -> Vec<u8> {
    crate::delimiters::parse(&cli.delimiter).unwrap_or_else(|_| b"\n".to_vec())
}

/// Parse `--property name=value` arguments.
///
/// # Errors
/// Returns an error when an argument is not in `name=value` form.
fn parse_properties(cli: &Cli) -> Result<Vec<(&str, String)>, MqCatError> {
    cli.properties
        .iter()
        .map(|property| {
            property
                .split_once('=')
                .map(|(name, value)| (name, value.to_owned()))
                .filter(|(name, _)| !name.is_empty())
                .ok_or_else(|| MqCatError::InvalidProperty(property.clone()))
        })
        .collect()
}

/// Parse the message format name, defaulting to MQ string format.
///
/// # Errors
/// Returns an error when the supplied format name does not fit MQ's format field.
fn parse_message_format(cli: &Cli) -> Result<MessageFormat, MqCatError> {
    match cli.message_format.as_deref() {
        Some(name) => name.parse::<MessageFormat>().map_err(|source| MqCatError::InvalidFormatName {
            name: name.to_owned(),
            source,
        }),
        None => Ok(MessageFormat::from(mqi::MqStr::from(mqi::header::fmt::MQFMT_STRING))),
    }
}

/// Parse the expiry argument as a millisecond count, or unlimited.
///
/// # Errors
/// Returns an error when the value is not `unlimited` or a non-negative integer.
fn parse_expiry(cli: &Cli) -> Result<i32, MqCatError> {
    if cli.expiry.eq_ignore_ascii_case("unlimited") {
        return Ok(constants::MQEI_UNLIMITED.0);
    }
    cli.expiry
        .parse()
        .map_err(|_| MqCatError::InvalidOption("--expiry must be `unlimited` or a millisecond count"))
}

/// Reader that yields messages split on a byte delimiter.
///
/// Reads incrementally so arbitrarily large streams can be consumed without
/// buffering the whole input.
struct DelimitedReader<'a, R> {
    reader: &'a mut R,
    delimiter: &'a [u8],
    buffer: Vec<u8>,
    pending: Vec<u8>,
    eof: bool,
}

/// Size of each incremental read from the input stream.
const READ_CHUNK: usize = 8192;

impl<'a, R: io::Read + 'a> DelimitedReader<'a, R> {
    fn new(reader: &'a mut R, delimiter: &'a [u8]) -> Self {
        Self {
            reader,
            delimiter,
            buffer: Vec::new(),
            pending: Vec::new(),
            eof: false,
        }
    }

    /// Return the next delimited message, or `None` at end of input.
    ///
    /// # Errors
    /// Returns an I/O error from the underlying reader.
    fn next_message(&mut self) -> io::Result<Option<&[u8]>> {
        loop {
            if let Some(at) = find_subslice(&self.pending, self.delimiter) {
                let message = self.pending[..at].to_vec();
                self.pending.drain(..at + self.delimiter.len());
                self.buffer = message;
                return Ok(Some(&self.buffer));
            }

            if self.eof {
                if self.pending.is_empty() {
                    return Ok(None);
                }
                self.buffer = std::mem::take(&mut self.pending);
                return Ok(Some(&self.buffer));
            }

            let mut chunk = [0u8; READ_CHUNK];
            let read = self.reader.read(&mut chunk)?;
            if read == 0 {
                self.eof = true;
            } else {
                self.pending.extend_from_slice(&chunk[..read]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_key_from_body() {
        let (key, body) = split_key(b"key:value", Some(b":"));
        assert_eq!(key, Some(&b"key"[..]));
        assert_eq!(body, b"value");
    }

    #[test]
    fn missing_key_delimiter_keeps_whole_message() {
        let (key, body) = split_key(b"whole", Some(b":"));
        assert_eq!(key, None);
        assert_eq!(body, b"whole");
    }

    #[test]
    fn no_key_delimiter_configured() {
        let (key, body) = split_key(b"whole", None);
        assert_eq!(key, None);
        assert_eq!(body, b"whole");
    }

    #[test]
    fn delimited_reader_splits_on_delimiter() {
        let data = b"one\ntwo\nthree";
        let mut input: &[u8] = data;
        let mut reader = DelimitedReader::new(&mut input, b"\n");
        assert_eq!(reader.next_message().unwrap(), Some(&b"one"[..]));
        assert_eq!(reader.next_message().unwrap(), Some(&b"two"[..]));
        assert_eq!(reader.next_message().unwrap(), Some(&b"three"[..]));
        assert_eq!(reader.next_message().unwrap(), None);
    }

    #[test]
    fn delimited_reader_handles_trailing_delimiter() {
        let data = b"one\ntwo\n";
        let mut input: &[u8] = data;
        let mut reader = DelimitedReader::new(&mut input, b"\n");
        assert_eq!(reader.next_message().unwrap(), Some(&b"one"[..]));
        assert_eq!(reader.next_message().unwrap(), Some(&b"two"[..]));
        assert_eq!(reader.next_message().unwrap(), None);
    }

    #[test]
    fn key_becomes_correlation_identifier() {
        let correl = correl_id_from_key(b"abc").unwrap();
        assert_eq!(&correl.0[..3], b"abc");
        assert_eq!(&correl.0[3..], &[0u8; 21]);
    }

    #[test]
    fn overlong_key_is_rejected() {
        let long = vec![b'k'; 25];
        assert!(correl_id_from_key(&long).is_err());
    }

    #[test]
    fn key_of_exactly_24_bytes_is_accepted() {
        let key = vec![b'k'; 24];
        assert!(correl_id_from_key(&key).is_ok());
    }

    #[test]
    fn find_subslice_locates_needle() {
        assert_eq!(find_subslice(b"abcdef", b"cd"), Some(2));
        assert_eq!(find_subslice(b"abcdef", b"zz"), None);
        assert_eq!(find_subslice(b"abc", b""), None);
    }
}
