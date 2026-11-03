//! Building `mqi` connection and open options from the command line.

use std::sync::Arc;

use mqi::{
    connection::{Binding, Ccdt, Credentials, MqServer, Tls},
    constants,
    prelude::*,
    types::{CipherSpec, KeyRepo, MQOO, QueueManagerName, QueueName},
};

use crate::{cli::Cli, error::MqCatError, target::Target};

// Brings `MqmContainer::load_mqm_default` into scope for [`load_library`].
use libmqm_sys::dlopen2::LoadMqmExt as _;

/// A handle to the IBM MQ client library, loaded at run time.
///
/// The MQ client is not linked into the binary. Instead the shared library is
/// opened when `mqcat` starts, so the same binary runs against whichever client
/// version is installed on the host. See [`load_library`].
///
/// The handle is shared through an [`Arc`], so a connection and the message
/// handles derived from it all keep the same loaded library alive. The library
/// is closed when the last handle is dropped.
///
/// This is a newtype rather than a bare `Arc` because `dlopen2`'s `Container`
/// implements neither `Debug` nor the traits `mqi` derives on `Connection`; the
/// wrapper supplies `Debug` so the rest of the crate can keep deriving it.
#[derive(Clone)]
pub struct MqLibrary(Arc<libmqm_sys::dlopen2::MqmContainer>);

impl MqLibrary {
    /// Wrap an already-loaded client library.
    #[must_use]
    pub fn new(container: libmqm_sys::dlopen2::MqmContainer) -> Self {
        Self(Arc::new(container))
    }
}

// `Container` cannot derive `Debug`, so render only what identifies the handle.
impl std::fmt::Debug for MqLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MqLibrary")
    }
}

impl mqi::Library for MqLibrary {
    // `MqmContainer` implements `Mqi` in `libmqm-sys` directly, so the wrapper
    // can expose it without going through `mqi`'s own `Library` impl. That impl
    // is gated on `mqi`'s `dlopen2` feature, which currently fails to compile
    // when enabled on its own (its test helper imports a `mqai`-only trait).
    type MQ = libmqm_sys::dlopen2::MqmContainer;

    fn lib(&self) -> &Self::MQ {
        &self.0
    }
}

/// The connection type returned by [`connect`].
///
/// `MqLibrary` is the runtime-loaded library handle. The alias keeps the rest of
/// the crate free of the long type name.
pub type Connection = mqi::Connection<MqLibrary, mqi::connection::ThreadNone>;

/// A borrowed view of a [`Connection`], used when a message handle needs one.
pub type ConnectionRef<'a> = mqi::ConnectionRef<'a, MqLibrary, mqi::connection::ThreadNone>;

/// Load the IBM MQ client library from the platform's normal search path.
///
/// The library name is platform dependent (`libmqm_r.so` on Unix-like systems,
/// `mqm.dll` on Windows) and is resolved by the dynamic loader, so the
/// installation directory must be on the loader path — `LD_LIBRARY_PATH` on
/// Linux, `DYLD_LIBRARY_PATH` on macOS, or `PATH` on Windows.
///
/// # Errors
/// Returns an error when the library cannot be found or opened, or when an
/// expected MQI symbol is missing from it.
///
/// Loading a dynamic library executes code the binary did not verify. The
/// function is safe to call because the library name is fixed rather than
/// user-supplied: it always resolves the platform's standard MQ client
/// (`libmqm_r.so` or `mqm.dll`) through the normal loader search path. The
/// residual risk is that a malicious library is placed earlier on that path,
/// which is the same trust assumption any dynamically linked program makes.
pub fn load_library() -> Result<MqLibrary, MqCatError> {
    // SAFETY: the library name is the fixed platform constant from libmqm-sys,
    // not attacker-controlled input. Loading it executes the MQ client's own
    // initialisers, which is the intended behaviour.
    let container = unsafe { libmqm_sys::dlopen2::MqmContainer::load_mqm_default() }.map_err(MqCatError::LoadLibrary)?;
    Ok(MqLibrary::new(container))
}

/// The putting application name reported in message descriptors.
pub const APPLICATION_NAME: mqi::types::ApplName = mqi::types::ApplName(mqi::mqstr!("mqcat"));

/// The cipher used when a TLS key repository is configured without an explicit
/// cipher. This TLS 1.3 cipher is supported by current MQ clients.
pub const DEFAULT_CIPHER: CipherSpec = CipherSpec(mqi::mqstr!("TLS_AES_128_GCM_SHA256"));

/// Connect to the queue manager described by the command line.
///
/// # Errors
/// Returns an error when the server or CCDT syntax is invalid, when a TLS option
/// cannot be represented in MQ's fixed-width fields, when the MQ client library
/// cannot be loaded, or when MQ refuses the connection.
pub fn connect(cli: &Cli) -> Result<Connection, MqCatError> {
    let server = cli
        .server
        .as_deref()
        .map(MqServer::try_from)
        .transpose()
        .map_err(|source| MqCatError::InvalidServer {
            value: cli.server.clone().unwrap_or_default(),
            source,
        })?;

    let ccdt = cli.ccdt.as_deref().map(Ccdt);
    let binding = if cli.local { Binding::Local } else { Binding::Default };
    let credentials = credentials(cli);

    // TLS options borrow their strings, so keep them in locals that outlive the
    // connect call. The connection does not retain these borrows after connect.
    let key_repo = cli
        .tls_key_repo
        .as_deref()
        .map(str::parse::<KeyRepo>)
        .transpose()
        .map_err(|source| MqCatError::InvalidTlsOption {
            option: "tls-key-repo",
            source,
        })?;

    let cipher = cli
        .tls_cipher_spec
        .as_deref()
        .map(str::parse::<CipherSpec>)
        .transpose()
        .map_err(|source| MqCatError::InvalidTlsOption {
            option: "tls-cipher-spec",
            source,
        })?
        .unwrap_or(DEFAULT_CIPHER);

    let cert_label = cli
        .tls_cert_label
        .as_deref()
        .map(str::parse::<mqi::types::CertificateLabel>)
        .transpose()
        .map_err(|source| MqCatError::InvalidTlsOption {
            option: "tls-cert-label",
            source,
        })?;

    let tls = key_repo.as_ref().map(|repo| Tls::new(repo, cert_label.as_ref(), &cipher));

    // The queue manager name is supplied on the command line so that a client
    // connection can be made without a CCDT entry.
    let queue_manager = cli
        .queue_manager
        .as_deref()
        .map(str::parse::<QueueManagerName>)
        .transpose()
        .map_err(|source| MqCatError::InvalidTargetName {
            name: cli.queue_manager.clone().unwrap_or_default(),
            source,
        })?;

    let connect_options = (APPLICATION_NAME, credentials, tls, server, ccdt, binding, queue_manager);

    // The library is opened per connection; `connect` is called once per
    // process, and the connection takes a shared reference to it.
    let library = load_library()?;
    mqi::connect_lib::<mqi::connection::ThreadNone, _>(library, &connect_options)
        .warn_as_error()
        .map_err(MqCatError::Connect)
}

/// Assemble credentials when a user name was supplied.
///
/// Returns `None` when no user name was given, letting MQ fall back to the
/// channel's own authentication configuration.
fn credentials<'a>(cli: &'a Cli) -> Option<Credentials<'a, &'a str>> {
    let username = cli.username.as_deref()?;
    Some(Credentials::User(
        username,
        cli.password.as_deref().unwrap_or_default().into(),
    ))
}

/// Build the open options for a target.
///
/// The target supplies the object name and type; `access` supplies the `MQOO_*`
/// access intent. Conditional elements use `Option` so the same tuple shape
/// works for queues and topics.
pub fn open_options<'a>(target: &'a Target, access: MQOO) -> impl mqi::open::OpenOption<'a, MQOO> {
    match target {
        Target::Queue { name, queue_manager } => (Some(*name), *queue_manager, None::<mqi::open::ObjectString<&'a str>>, access),
        Target::Topic { name } => (
            None::<QueueName>,
            None::<QueueManagerName>,
            Some(mqi::open::ObjectString(name.as_str())),
            access,
        ),
    }
}

/// Combine the base access intent with flags that should always be applied.
#[must_use]
pub fn access_options(base: MQOO) -> MQOO {
    base | constants::MQOO_FAIL_IF_QUIESCING
}
