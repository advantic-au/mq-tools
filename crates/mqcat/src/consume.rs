//! Consumer mode: get messages from the target and render them.

use std::io::{self, Write as _};

use mqi::{constants, get::GetWait, prelude::*};

use crate::{
    connection,
    error::MqCatError,
    format::{MessageFormatString, RenderedMessage},
    target::Target,
};

use crate::cli::Cli;

/// Default format string applied when the user did not supply `-f`.
///
/// This mirrors kcat, which prints the message body followed by the output
/// delimiter. The delimiter is appended by [`render_and_write`] rather than being
/// part of the format string so that `-D` stays authoritative.
pub const DEFAULT_FORMAT: &str = "%s";

/// Get buffer size used when the queue's maximum message length is unknown.
///
/// This applies when the queue manager refuses an inquiring open, or when the
/// object reports no limit (as topics do). One mebibyte matches the default
/// `MAXMSGL` of a queue created without an explicit limit.
pub const DEFAULT_BUFFER_SIZE: usize = 1024 * 1024;

/// Run consumer mode.
///
/// # Errors
/// Returns an error when the connection, open, or get fails, or when output
/// cannot be written.
pub fn run(cli: &Cli, target: &Target) -> Result<(), MqCatError> {
    let format = compile_format(cli)?;
    // A format that never references the body still needs the get buffer, but
    // knowing the body is unused lets the renderer skip copying it. The flag is
    // also what the README documents as the reason `%s` matters for binary data.
    debug_assert!(
        format.uses_body() || !format.segments().is_empty(),
        "format strings always render at least one segment"
    );
    let delimiter = crate::delimiters::parse(&cli.delimiter).map_err(|source| MqCatError::InvalidDelimiter {
        value: cli.delimiter.clone(),
        source,
    })?;
    let correl_id = cli
        .correl_id
        .as_deref()
        .map(crate::error::parse_identifier)
        .transpose()
        .map_err(|source| MqCatError::InvalidIdentifier {
            value: cli.correl_id.clone().unwrap_or_default(),
            source,
        })?;
    let msg_id = cli
        .msg_id
        .as_deref()
        .map(crate::error::parse_identifier)
        .transpose()
        .map_err(|source| MqCatError::InvalidIdentifier {
            value: cli.msg_id.clone().unwrap_or_default(),
            source,
        })?;

    let connection = connection::connect(cli)?;

    // A topic cannot be opened for input directly: MQ rejects a get handle on a
    // topic with MQRC_OPTIONS_ERROR. Consuming from a topic therefore creates a
    // managed, non-durable subscription and reads from the queue MQ allocates
    // for it. A queue is opened for input as usual.
    let (object, subscription, max_message_length) = if target.is_topic() {
        let (subscription, object) = open_subscription(target, connection.connection_ref())?;
        // A managed subscription queue is temporary and cannot be inquired for
        // MAXMSGL reliably, so the fixed default is used.
        (object, Some(subscription), DEFAULT_BUFFER_SIZE)
    } else {
        // Prefer an open that permits inquiry: the get buffer is then sized from
        // the queue's own maximum message length, which avoids both truncation and
        // the MQRC_DATA_LENGTH_ERROR MQ returns when the buffer exceeds that limit.
        // Inquire authority is not always granted, so fall back to a plain input
        // open and a fixed default buffer when the first attempt is refused.
        match open_with_inquire(cli, target, connection.connection_ref()) {
            Ok(object) => {
                let limit = object
                    .inquire_integer(mqi::attribute::MQIA_MAX_MSG_LENGTH)
                    .warn_as_error()
                    .ok()
                    .flatten()
                    .filter(|len| *len > 0)
                    .map_or(DEFAULT_BUFFER_SIZE, |len| len as usize);
                (object, None, limit)
            }
            Err(_) => {
                let object = mqi::Object::open(
                    connection.connection_ref(),
                    &connection::open_options(target, consumer_access(cli, false)),
                )
                .warn_as_error()
                .map_err(|source| MqCatError::Open {
                    target: target.display_name(),
                    source,
                })?;
                (object, None, DEFAULT_BUFFER_SIZE)
            }
        }
    };

    let mut buffer = vec![0u8; max_message_length];
    let mut stdout = io::stdout().lock();
    let mut consumed = 0usize;

    while !cli.count.is_some_and(|limit| consumed >= limit) {
        // A message handle is only needed when properties are part of the output.
        let mut properties = if format.uses_properties() {
            Some(
                mqi::Properties::new(object.connection().connection_ref(), mqi::types::MQCMHO::default()).map_err(|source| {
                    MqCatError::Mq {
                        operation: "MQCRTMH",
                        source,
                    }
                })?,
            )
        } else {
            None
        };

        let wait = wait_option(cli);
        let correl = correl_id.map(mqi::types::CorrelationId);
        let msg = msg_id.map(mqi::types::MessageId);
        // When syncpoint is requested the get joins the unit of work and the
        // message is only removed once committed below.
        let syncpoint = cli.syncpoint.then_some(constants::MQGMO_SYNCPOINT);
        // Browsing needs a browse cursor on the get, not just MQOO_BROWSE on the
        // open: without these options MQGET removes the message even though the
        // handle was opened for browsing. The first get positions the cursor and
        // later gets advance it.
        let browse = cli.browse.then_some(if consumed == 0 {
            constants::MQGMO_BROWSE_FIRST
        } else {
            constants::MQGMO_BROWSE_NEXT
        });

        let result = match properties.as_mut() {
            Some(handle) => object
                .get_data_with(&(wait, correl, msg, syncpoint, browse, &mut *handle), buffer.as_mut_slice())
                .warn_as_error(),
            None => object
                .get_data_with(&(wait_option(cli), correl, msg, syncpoint, browse), buffer.as_mut_slice())
                .warn_as_error(),
        };

        let Some((body, descriptor)) = result.map_err(|source| MqCatError::Mq {
            operation: "MQGET",
            source,
        })?
        else {
            // No message available within the wait window.
            if cli.exit_on_empty {
                break;
            }
            if matches!(wait_option(cli), GetWait::NoWait) {
                // A no-wait get would spin; sleep briefly instead.
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            continue;
        };

        let property_text = match properties.as_ref() {
            Some(handle) => collect_properties(handle)?,
            None => String::new(),
        };

        let target_name = target.display_name();
        let rendered = RenderedMessage {
            body,
            descriptor: &descriptor,
            target: &target_name,
            properties: &property_text,
        }
        .render(&format)
        .map_err(MqCatError::Write)?;

        render_and_write(&mut stdout, &rendered, &delimiter).map_err(MqCatError::Write)?;

        if cli.syncpoint {
            // Commit the unit of work so the message is removed from the queue.
            // `Syncpoint::new` manages the current unit without calling MQBEGIN,
            // which is what a single-resource get needs.
            // The commit consumes the Syncpoint and returns the connection
            // reference; the unit of work is finished so the value is dropped.
            let _ = mqi::Syncpoint::new(object.connection().connection_ref())
                .commit()
                .warn_as_error()
                .map_err(|source| MqCatError::Mq {
                    operation: "MQCMIT",
                    source,
                })?;
        }

        if let Some(handle) = properties {
            handle.close().map_err(|source| MqCatError::Mq {
                operation: "MQDLTMH",
                source,
            })?;
        }

        consumed += 1;
    }

    object.close().warn_as_error().map_err(|source| MqCatError::Mq {
        operation: "MQCLOSE",
        source,
    })?;

    // Closing the subscription removes the managed queue and the subscription
    // itself. It is closed after the queue handle so the queue is still valid
    // while the object handle closes.
    if let Some(subscription) = subscription {
        subscription.close().warn_as_error().map_err(|source| MqCatError::Mq {
            operation: "MQCLOSE",
            source,
        })?;
    }

    Ok(())
}

/// Create a managed, non-durable subscription and return the queue to read from.
///
/// A topic cannot be opened for input: MQ requires an `MQSUB` to create the
/// subscription that resolves the topic to a queue. `MQSO_MANAGED` asks MQ to
/// allocate and open that queue, and `MQSO_NON_DURABLE` keeps the subscription
/// alive only while the process is running, which matches kcat's behaviour of
/// consuming what is published while it is subscribed.
///
/// # Errors
/// Returns an error when the subscription cannot be created.
fn open_subscription<'a>(
    target: &'a Target,
    connection: connection::ConnectionRef<'a>,
) -> Result<
    (
        mqi::Subscription<connection::ConnectionRef<'a>>,
        mqi::Object<connection::ConnectionRef<'a>>,
    ),
    MqCatError,
> {
    let Target::Topic { name } = target else {
        return Err(MqCatError::InvalidOption("a subscription requires a topic target"));
    };

    // A managed subscription's queue is opened by MQ with input access. Only
    // `MQSO_*` options apply to the subscription; `MQOO_*` access flags belong to
    // the object open and would be rejected here. `MQSO_MANAGED` must not be
    // repeated: `subscribe_managed` already sets it, and MQ rejects the
    // duplicate flag with MQRC_OPTIONS_ERROR.
    let options = constants::MQSO_CREATE | constants::MQSO_NON_DURABLE;
    mqi::Subscription::subscribe_managed(connection, (options, mqi::open::ObjectString(name.as_str())))
        .warn_as_error()
        .map_err(|source| MqCatError::Open {
            target: target.display_name(),
            source,
        })
}

/// Compile the format string, falling back to the default.
fn compile_format(cli: &Cli) -> Result<MessageFormatString, MqCatError> {
    let source = cli.format.as_deref().unwrap_or(DEFAULT_FORMAT);
    MessageFormatString::parse(source).map_err(|source| MqCatError::InvalidFormat { source })
}

/// Choose the wait option for a get.
///
/// `--exit-on-empty` implies an immediate check; a browse always uses no-wait so
/// the cursor can advance without blocking. Otherwise the configured `--wait`
/// seconds are used.
fn wait_option(cli: &Cli) -> GetWait {
    if cli.exit_on_empty || cli.browse {
        GetWait::NoWait
    } else {
        GetWait::Wait(i32::try_from(cli.wait.saturating_mul(1000)).unwrap_or(i32::MAX))
    }
}

/// Build the `MQOO_*` access flags for consumer mode.
///
/// When `inquire` is set, `MQOO_INQUIRE` is added so the get buffer can be sized
/// from the object's maximum message length. Callers fall back to `false` when
/// the queue manager refuses an inquiring open.
fn consumer_access(cli: &Cli, inquire: bool) -> mqi::types::MQOO {
    let base = if cli.browse {
        constants::MQOO_BROWSE | constants::MQOO_INPUT_AS_Q_DEF
    } else if cli.shared {
        constants::MQOO_INPUT_SHARED
    } else {
        constants::MQOO_INPUT_AS_Q_DEF
    };
    let access = if inquire { base | constants::MQOO_INQUIRE } else { base };
    connection::access_options(access)
}

/// Open the target for input, requesting inquiry authority.
///
/// The connection is borrowed rather than consumed so the caller can retry the
/// open without reconnecting when this attempt is refused.
fn open_with_inquire<'a>(
    cli: &Cli,
    target: &'a Target,
    connection: connection::ConnectionRef<'a>,
) -> Result<mqi::Object<connection::ConnectionRef<'a>>, mqi::result::Error> {
    mqi::Object::open(connection, &connection::open_options(target, consumer_access(cli, true))).warn_as_error()
}

/// Collect message properties as a `name=value` CSV string.
fn collect_properties(handle: &mqi::Properties<connection::ConnectionRef<'_>>) -> Result<String, MqCatError> {
    use mqi::properties::{INQUIRE_ALL_USR, Name, Value};

    let mut pairs = Vec::new();
    for property in handle.property_iter::<(Value, Name<String>), _>(INQUIRE_ALL_USR, mqi::types::MQIMPO::default()) {
        let (value, Name(name)) = property.warn_as_error().map_err(|source| MqCatError::Mq {
            operation: "MQINQMP",
            source,
        })?;
        pairs.push(format!("{name}={}", property_value_text(&value)));
    }
    Ok(pairs.join(","))
}

/// Render a property value as text for the `%h` format token.
///
/// `mqi`'s `Value` has no `Display` implementation, so each variant is rendered
/// here. String values are decoded from their CCSID; anything that cannot be
/// decoded falls back to a lossy rendering rather than failing the whole message.
fn property_value_text(value: &mqi::properties::Value) -> String {
    use mqi::properties::Value;

    match value {
        Value::Boolean(flag) => flag.to_string(),
        Value::Int8(number) => number.to_string(),
        Value::Int16(number) => number.to_string(),
        Value::Int32(number) => number.to_string(),
        Value::Int64(number) => number.to_string(),
        Value::Float32(number) => number.to_string(),
        Value::Float64(number) => number.to_string(),
        Value::ByteString(bytes) => bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
        Value::String(text) => String::try_from(text.clone()).unwrap_or_else(|_| "<non-utf8>".to_owned()),
        Value::Null => String::new(),
    }
}

/// Write the rendered message followed by the output delimiter.
fn render_and_write(out: &mut impl io::Write, rendered: &[u8], delimiter: &[u8]) -> io::Result<()> {
    out.write_all(rendered)?;
    if !delimiter.is_empty() {
        out.write_all(delimiter)?;
    }
    out.flush()
}

/// Run metadata-list mode, reporting queue attributes for the target.
///
/// # Errors
/// Returns an error when the connection or open fails, or when attributes cannot
/// be inquired.
pub fn list(cli: &Cli, target: &Target) -> Result<(), MqCatError> {
    let access = connection::access_options(constants::MQOO_INQUIRE | constants::MQOO_INPUT_AS_Q_DEF);
    let connection = connection::connect(cli)?;
    let object = mqi::Object::open(connection, &connection::open_options(target, access))
        .warn_as_error()
        .map_err(|source| MqCatError::Open {
            target: target.display_name(),
            source,
        })?;

    let mut stdout = io::stdout().lock();
    writeln!(stdout, "target: {}", target.display_name()).map_err(MqCatError::Write)?;
    writeln!(stdout, "kind: {}", if target.is_topic() { "topic" } else { "queue" }).map_err(MqCatError::Write)?;

    let depth = object
        .inquire_integer(mqi::attribute::MQIA_CURRENT_Q_DEPTH)
        .warn_as_error()
        .map_err(|source| MqCatError::Mq {
            operation: "MQINQ",
            source,
        })?;
    writeln!(stdout, "current-depth: {}", depth.unwrap_or_default()).map_err(MqCatError::Write)?;

    let max_depth = object
        .inquire_integer(mqi::attribute::MQIA_MAX_Q_DEPTH)
        .warn_as_error()
        .map_err(|source| MqCatError::Mq {
            operation: "MQINQ",
            source,
        })?;
    writeln!(stdout, "max-depth: {}", max_depth.unwrap_or_default()).map_err(MqCatError::Write)?;

    stdout.flush().map_err(MqCatError::Write)?;

    object.close().warn_as_error().map_err(|source| MqCatError::Mq {
        operation: "MQCLOSE",
        source,
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("mqcat").chain(args.iter().copied())).expect("valid arguments")
    }

    #[test]
    fn default_format_renders_body_only() {
        let format = MessageFormatString::parse(DEFAULT_FORMAT).unwrap();
        assert!(format.uses_body());
        assert!(!format.uses_properties());
    }

    #[test]
    fn exit_on_empty_uses_no_wait() {
        let cli = parse(&["-C", "-t", "Q1", "-e"]);
        assert!(matches!(wait_option(&cli), GetWait::NoWait));
    }

    #[test]
    fn default_wait_is_in_milliseconds() {
        let cli = parse(&["-C", "-t", "Q1", "-w", "3"]);
        assert!(matches!(wait_option(&cli), GetWait::Wait(3000)));
    }

    #[test]
    fn browse_uses_browse_access() {
        let cli = parse(&["-C", "-t", "Q1", "--browse"]);
        assert!(consumer_access(&cli, true).contains(constants::MQOO_BROWSE));
    }

    #[test]
    fn inquire_access_is_requested_when_available() {
        // The preferred open asks for inquiry so the buffer can be sized from
        // the object's MAXMSGL.
        for args in [
            &["-C", "-t", "Q1"][..],
            &["-C", "-t", "Q1", "--browse"][..],
            &["-C", "-t", "Q1", "--shared"][..],
        ] {
            let cli = parse(args);
            assert!(
                consumer_access(&cli, true).contains(constants::MQOO_INQUIRE),
                "inquiry must be requested for {args:?}"
            );
        }
    }

    #[test]
    fn fallback_access_omits_inquire() {
        // The retry must not ask for inquiry, or it would fail the same way.
        for args in [
            &["-C", "-t", "Q1"][..],
            &["-C", "-t", "Q1", "--browse"][..],
            &["-C", "-t", "Q1", "--shared"][..],
        ] {
            let cli = parse(args);
            assert!(
                !consumer_access(&cli, false).contains(constants::MQOO_INQUIRE),
                "fallback must not request inquiry for {args:?}"
            );
        }
    }

    #[test]
    fn fallback_access_keeps_input_intent() {
        let cli = parse(&["-C", "-t", "Q1"]);
        let access = consumer_access(&cli, false);
        assert!(access.contains(constants::MQOO_INPUT_AS_Q_DEF));
        assert!(!access.contains(constants::MQOO_INQUIRE));
    }

    #[test]
    fn default_buffer_size_is_one_mebibyte() {
        assert_eq!(DEFAULT_BUFFER_SIZE, 1024 * 1024);
    }

    #[test]
    fn property_values_render_as_plain_text() {
        use mqi::properties::Value;

        assert_eq!(property_value_text(&Value::Int32(42)), "42");
        assert_eq!(property_value_text(&Value::Boolean(true)), "true");
        assert_eq!(property_value_text(&Value::ByteString(vec![0xab, 0x01])), "ab01");
        assert_eq!(property_value_text(&Value::Null), "");
        let text: mqi::string::StrCcsidOwned = "hello".into();
        assert_eq!(property_value_text(&Value::String(text)), "hello");
    }

    #[test]
    fn shared_uses_shared_access() {
        let cli = parse(&["-C", "-t", "Q1", "--shared"]);
        assert!(consumer_access(&cli, true).contains(constants::MQOO_INPUT_SHARED));
    }

    #[test]
    fn render_appends_delimiter() {
        let mut out = Vec::new();
        render_and_write(&mut out, b"body", b"\n").unwrap();
        assert_eq!(out, b"body\n");
    }

    #[test]
    fn empty_delimiter_appends_nothing() {
        let mut out = Vec::new();
        render_and_write(&mut out, b"body", b"").unwrap();
        assert_eq!(out, b"body");
    }
}
