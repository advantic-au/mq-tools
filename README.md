# mcat

Command-line tools for IBM MQ, built on the [`mqi`](https://crates.io/crates/mqi)
crate. The repository is a Cargo workspace so additional tools can be added
alongside the first one.

| Tool | Description |
|------|-------------|
| [`mqcat`](crates/mqcat) | [kcat](https://github.com/edenhill/kcat)-style producer and consumer for IBM MQ |

## Requirements

**Build time:** nothing but a Rust toolchain. The tools load the IBM MQ client
library at run time rather than linking against it, so no MQ installation is
needed to compile, and the same binary works with whatever client version is
installed on the host.

**Run time:** an IBM MQ client or server installation with its libraries on the
dynamic loader path. For a client redistributable unpacked into `/opt/mqc`:

```bash
export LD_LIBRARY_PATH="/opt/mqc/lib64:$LD_LIBRARY_PATH"
```

`MQ_HOME` is not required at run time; the client library is located through the
platform's standard loader search path (`LD_LIBRARY_PATH` on Linux,
`DYLD_LIBRARY_PATH` on macOS, `PATH` on Windows).

If the library cannot be found, the tool exits with code 1 and reports:

```
mqcat: unable to load the IBM MQ client library
  caused by: Could not open library: libmqm_r.so: cannot open shared object file: No such file or directory
```

## Build

```bash
cargo build --release
```

The resulting binary is `target/release/mqcat`. It does not depend on the MQ
client at load time, which you can confirm with `ldd target/release/mqcat`.

## Run

The examples below use the IBM MQ container's default developer
configuration: queue manager `QM1`, queue `DEV.QUEUE.1`, channel
`DEV.APP.SVRCONN`, and the `app` user.

```bash
export MQ_SERVER='DEV.APP.SVRCONN/TCP/localhost(1414)'

# Produce the contents of a file as one message
mqcat -P -t DEV.QUEUE.1 -b "$MQ_SERVER" -m QM1 -u app -a password message.json

# Produce stdin split on newlines
printf 'one\ntwo\n' | mqcat -P -t DEV.QUEUE.1 -b "$MQ_SERVER" -m QM1 -u app -a password

# Consume and print each body
mqcat -C -t DEV.QUEUE.1 -b "$MQ_SERVER" -m QM1 -u app -a password -c 2

# Browse without removing anything from the queue
mqcat -C --browse -t DEV.QUEUE.1 -b "$MQ_SERVER" -m QM1 -u app -a password

# Inspect queue depth
mqcat -L -t DEV.QUEUE.1 -b "$MQ_SERVER" -m QM1 -u app -a password
```

### Queue manager prerequisites

`mqcat` needs no special queue manager configuration beyond a working channel
and a user that may connect. Two settings are worth knowing about:

* **`MQOO_INQUIRE` is required.** The consumer sizes its get buffer from the
  queue's `MAXMSGL`, so the user needs inquire authority on the queue. Without
  it, MQGET fails with `MQRC_DATA_LENGTH_ERROR`.
* **Connection authentication.** If the queue manager uses `CONNAUTH` with
  `CHCKCLNT(REQUIRED)`, the supplied user and password are validated by the
  host's PAM stack. Minimal container images often ship without PAM modules or
  an `app` OS user, in which case authentication cannot succeed and the
  connection is refused with `MQRC_NOT_AUTHORIZED` or `MQRC_SECURITY_ERROR`.
  Either install PAM and create the user, or relax the check for development:

  ```bash
  # Development only: accept the supplied user without validating the password
  echo "ALTER AUTHINFO(DEV.AUTHINFO) AUTHTYPE(IDPWOS) CHCKCLNT(NONE) ADOPTCTX(YES)" | runmqsc QM1
  echo "SET CHLAUTH(DEV.APP.SVRCONN) TYPE(ADDRESSMAP) ADDRESS(*) USERSRC(CHANNEL) CHCKCLNT(ASQMGR) ACTION(REPLACE)" | runmqsc QM1
  ```

See [`crates/mqcat/README.md`](crates/mqcat/README.md) for the full option
reference and the format-string tokens.
