//! Command-line argument definitions for `mqcat`.
//!
//! The flags mirror kcat's naming where an IBM MQ equivalent exists, so that
//! existing kcat habits carry over. Flags that have no MQ meaning (partitions,
//! consumer groups, broker lists) are replaced by MQ-specific ones.

use std::path::PathBuf;

use clap::{ArgAction, Parser, ValueEnum};

/// Mode of operation, mirroring kcat's `-P` / `-C` / `-L`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    /// Produce messages to a queue or topic.
    Produce,
    /// Consume messages from a queue or topic.
    Consume,
    /// List metadata about the queue or topic.
    List,
}

/// kcat-style producer and consumer for IBM MQ.
#[derive(Debug, Parser)]
#[command(
    name = "mqcat",
    version,
    about = "kcat-style producer and consumer for IBM MQ",
    long_about = None,
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    /// Producer mode: read messages from input and put them on the target.
    #[arg(short = 'P', long, action = ArgAction::SetTrue, group = "mode")]
    pub produce: bool,

    /// Consumer mode: get messages from the target and print them.
    #[arg(short = 'C', long, action = ArgAction::SetTrue, group = "mode")]
    pub consume: bool,

    /// List metadata about the target (queue depth, usage, and so on).
    #[arg(short = 'L', long, action = ArgAction::SetTrue, group = "mode")]
    pub list: bool,

    /// Queue to produce to or consume from.
    #[arg(short = 't', long, value_name = "QUEUE", group = "target")]
    pub queue: Option<String>,

    /// Topic string to produce to or subscribe to.
    #[arg(long, value_name = "TOPIC", group = "target")]
    pub topic: Option<String>,

    /// Queue manager to connect to. Defaults to the MQ client configuration.
    #[arg(short = 'm', long, value_name = "QMGR")]
    pub queue_manager: Option<String>,

    /// Channel connection in `CHANNEL/TRANSPORT/HOST(PORT)` form.
    ///
    /// For example `DEV.APP.SVRCONN/TCP/localhost(1414)`.
    #[arg(short = 'b', long, value_name = "CHANNEL/TRANSPORT/HOST(PORT)")]
    pub server: Option<String>,

    /// Connect using a client channel definition table (CCDT) URL.
    #[arg(long, value_name = "URL", conflicts_with = "server")]
    pub ccdt: Option<String>,

    /// Connect with a local binding instead of a client connection.
    #[arg(long, conflicts_with_all = ["server", "ccdt"])]
    pub local: bool,

    /// User name for MQ authentication.
    #[arg(short = 'u', long, value_name = "USER")]
    pub username: Option<String>,

    /// Password for MQ authentication. Requires `--username`.
    #[arg(short = 'a', long, value_name = "PASSWORD", requires = "username")]
    pub password: Option<String>,

    /// TLS key repository stem (without the `.kdb` suffix).
    #[arg(long, value_name = "KEYREPO")]
    pub tls_key_repo: Option<String>,

    /// TLS cipher specification. Requires `--tls-key-repo`.
    #[arg(long, value_name = "CIPHER", requires = "tls_key_repo")]
    pub tls_cipher_spec: Option<String>,

    /// TLS certificate label. Requires `--tls-key-repo`.
    #[arg(long, value_name = "LABEL", requires = "tls_key_repo")]
    pub tls_cert_label: Option<String>,

    /// Format string used to render each consumed message.
    ///
    /// See the README for the supported tokens. Defaults to the message body
    /// followed by the output delimiter.
    #[arg(short = 'f', long, value_name = "FORMAT")]
    pub format: Option<String>,

    /// Delimiter written after each consumed message and used to split input.
    ///
    /// Accepts escape sequences such as `\n`, `\r`, `\t`, and `\xNN`.
    #[arg(short = 'D', long, value_name = "DELIM", default_value = "\\n")]
    pub delimiter: String,

    /// Delimiter that separates a message key from its body in produced input.
    ///
    /// In IBM MQ the key is mapped to the message correlation identifier.
    #[arg(short = 'K', long, value_name = "DELIM")]
    pub key_delimiter: Option<String>,

    /// Exit successfully once this many messages have been processed.
    #[arg(short = 'c', long, value_name = "COUNT")]
    pub count: Option<usize>,

    /// Exit as soon as no message is immediately available (consumer only).
    ///
    /// Without this flag the consumer waits indefinitely for messages.
    #[arg(short = 'e', long, action = ArgAction::SetTrue)]
    pub exit_on_empty: bool,

    /// Seconds to wait for a message before giving up (consumer only).
    #[arg(short = 'w', long, value_name = "SECONDS", default_value_t = 5)]
    pub wait: u64,

    /// Browse messages without removing them from the queue (consumer only).
    #[arg(long, action = ArgAction::SetTrue)]
    pub browse: bool,

    /// Open the queue for shared input rather than exclusive input.
    #[arg(long, action = ArgAction::SetTrue)]
    pub shared: bool,

    /// Commit each message using MQ syncpoint (consumer only).
    #[arg(long, action = ArgAction::SetTrue)]
    pub syncpoint: bool,

    /// Match only messages with this correlation identifier (hex, consumer only).
    #[arg(long, value_name = "HEX")]
    pub correl_id: Option<String>,

    /// Match only messages with this message identifier (hex, consumer only).
    #[arg(long, value_name = "HEX")]
    pub msg_id: Option<String>,

    /// Message priority to set when producing.
    #[arg(short = 'p', long, value_name = "PRIORITY")]
    pub priority: Option<i32>,

    /// Message persistence: `yes`, `no`, or `as-queue-default`.
    #[arg(long, value_name = "PERSISTENCE", default_value = "as-queue-default")]
    pub persistence: Persistence,

    /// Message expiry in milliseconds, or `unlimited`.
    #[arg(long, value_name = "EXPIRY", default_value = "unlimited")]
    pub expiry: String,

    /// Message format name to use when producing (for example `MQFMT_STRING`).
    #[arg(long, value_name = "FORMAT")]
    pub message_format: Option<String>,

    /// Reply-to queue to set on produced messages.
    #[arg(long, value_name = "QUEUE")]
    pub reply_to: Option<String>,

    /// Property to set on produced messages, as `name=value`. Repeatable.
    #[arg(short = 'H', long = "property", value_name = "NAME=VALUE", action = ArgAction::Append)]
    pub properties: Vec<String>,

    /// Print produced message bodies to stdout as well as putting them.
    #[arg(short = 'T', long, action = ArgAction::SetTrue)]
    pub tee: bool,

    /// Files to read as complete messages when producing.
    ///
    /// With no files, input is read from stdin and split on the delimiter.
    #[arg(value_name = "FILE")]
    pub files: Vec<PathBuf>,

    /// Increase logging verbosity. Repeat for more detail.
    #[arg(short = 'v', long, action = ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress informational logging.
    #[arg(short = 'q', long, action = ArgAction::SetTrue, global = true)]
    pub quiet: bool,
}

/// Message persistence selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Persistence {
    /// Persistent message delivery.
    Yes,
    /// Non-persistent message delivery.
    No,
    /// Use the queue's default persistence.
    #[value(name = "as-queue-default")]
    AsQueueDefault,
}

impl Cli {
    /// Resolve the selected mode from the mutually exclusive flags.
    #[must_use]
    pub const fn mode(&self) -> Mode {
        if self.produce {
            Mode::Produce
        } else if self.list {
            Mode::List
        } else {
            Mode::Consume
        }
    }

    /// Reject argument combinations that cannot be honoured.
    ///
    /// # Errors
    /// Returns an error when no target is named or when flags are combined in a
    /// way that has no meaningful IBM MQ equivalent.
    pub fn validate(&self) -> Result<(), crate::error::MqCatError> {
        use crate::error::MqCatError;

        if self.queue.is_none() && self.topic.is_none() {
            return Err(MqCatError::MissingTarget);
        }
        if self.queue.is_some() && self.topic.is_some() {
            return Err(MqCatError::ConflictingTargets);
        }
        if self.mode() != Mode::Consume && self.browse {
            return Err(MqCatError::InvalidOption("--browse requires consumer mode"));
        }
        if self.mode() != Mode::Produce && !self.properties.is_empty() {
            return Err(MqCatError::InvalidOption("--property requires producer mode"));
        }
        if self.mode() != Mode::Produce && self.tee {
            return Err(MqCatError::InvalidOption("--tee requires producer mode"));
        }
        Ok(())
    }

    /// Build the resolved target description.
    ///
    /// # Errors
    /// Returns an error if the queue or topic name cannot be represented by MQ.
    pub fn target(&self) -> Result<crate::target::Target, crate::error::MqCatError> {
        crate::target::Target::new(self.queue.as_deref(), self.topic.as_deref(), self.queue_manager.as_deref())
    }
}
