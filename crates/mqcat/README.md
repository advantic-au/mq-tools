# mqcat

A [kcat](https://github.com/edenhill/kcat)-style command-line producer and
consumer for IBM MQ.

kcat is a small, scriptable client for Apache Kafka: it reads messages from
stdin or files and produces them to a topic, or consumes from a topic and prints
each message through a format string. `mqcat` keeps that command-line shape and
format-string syntax, replacing Kafka concepts with their IBM MQ equivalents.

## How kcat concepts map to IBM MQ

| kcat concept | IBM MQ equivalent used by `mqcat` |
|---|---|
| Broker (`-b`) | Channel connection, written `CHANNEL/TRANSPORT/HOST(PORT)` |
| Topic | MQ topic string, or a queue name |
| Partition | No equivalent; there is no per-partition offset |
| Message offset | No equivalent; the message identifier fills this role |
| Message key | Message correlation identifier (`CorrelId`) |
| Message headers | Message properties |
| Producer transactions | MQ syncpoint (`--syncpoint`) |
| `-o beginning/end/stored` | Not applicable; MQ delivers in queue order |

Because MQ has no partitions or offsets, the format tokens differ from kcat's.
The table below lists every token and, where one exists, the kcat token it
replaces.

## How the MQ client library is loaded

`mqcat` does not link against the IBM MQ client library when it is built.
Instead it opens the client at run time, which has two practical consequences:

* Building requires only a Rust toolchain. No MQ installation, no `MQ_HOME`, and
  no headers are needed to compile.
* One binary runs against any installed MQ client version. To upgrade the client,
  replace the library on the host; the binary is unchanged.

The library is resolved through the platform's normal dynamic-loader search
path, so the client's `lib64` directory must be on it. `MQ_HOME` is not
consulted.

The trade-off is that a missing or wrong-version client is discovered when the
tool starts rather than when it is built. That surfaces as a clear message and
exit code 1:

```
mqcat: unable to load the IBM MQ client library
  caused by: Could not open library: libmqm_r.so: cannot open shared object file: No such file or directory
```

This differs from kcat, which links `librdkafka` at build time. If you prefer a
self-contained binary over a portable one, linking at build time is also
possible: enable `mqi`'s `link` feature and build with `MQ_HOME` set.

## Format string tokens

| Token | Meaning | kcat equivalent |
|---|---|---|
| `%s` | Message body | `%s` |
| `%S` | Body length in bytes | `%S` |
| `%k` | Correlation identifier, as hex | `%k` (message key) |
| `%c` | Correlation identifier, as hex | — |
| `%K` | Correlation identifier length | `%K` |
| `%i` | Message identifier, as hex | — |
| `%f` | Message format name | — |
| `%C` | Coded character set identifier (CCSID) | — |
| `%x` | Numeric encoding of the body | — |
| `%t` | Queue or topic name | `%t` |
| `%p` | Message priority | `%p` |
| `%P` | Message persistence | — |
| `%e` | Message expiry | — |
| `%T` | Put date and time (`YYYYMMDDHHMMSS`) | `%T` (timestamp) |
| `%h` | Message properties as `name=value` CSV | `%h` (headers) |
| `%o` | Putting application name | `%o` (offset) |
| `%b` | Backout count | — |
| `%r` | Reply-to queue name | — |
| `%%` | Literal `%` | `%%` |

Escapes `\n`, `\r`, `\t`, `\0`, `\\`, `\xNN`, and octal `\NNN` are interpreted in
format strings and in delimiter arguments.

### Note on `%o` and `%k`

kcat's `%o` is a partition offset and `%k` a message key. Neither has a direct MQ
counterpart, so `mqcat` repurposes `%o` for the putting application name and `%k`
as a short alias for the correlation identifier. Scripts that depend on numeric
offsets need to be updated; the correlation identifier is the closest analogue to
kcat's key because it is what MQ applications use to correlate request and reply
messages.

### Default format

When `-f` is not supplied, `mqcat` prints the message body followed by the output
delimiter, matching kcat's default consumer output.

## Options

### Mode

| Option | Meaning |
|---|---|
| `-P`, `--produce` | Producer mode |
| `-C`, `--consume` | Consumer mode |
| `-L`, `--list` | List metadata about the target |

Exactly one mode applies. With no mode flag, consumer mode is used.

### Target

| Option | Meaning |
|---|---|
| `-t`, `--queue <QUEUE>` | Queue to produce to or consume from |
| `--topic <TOPIC>` | Topic string to produce to or subscribe to |
| `-m`, `--queue-manager <QMGR>` | Queue manager name |

`--queue` and `--topic` are mutually exclusive and one is required.

### Connection

| Option | Meaning |
|---|---|
| `-b`, `--server <CHANNEL/TRANSPORT/HOST(PORT)>` | Client channel connection |
| `--ccdt <URL>` | Client channel definition table |
| `--local` | Local binding instead of a client connection |
| `-u`, `--username <USER>` | User name |
| `-a`, `--password <PASSWORD>` | Password (requires `--username`) |
| `--tls-key-repo <KEYREPO>` | TLS key repository stem |
| `--tls-cipher-spec <CIPHER>` | TLS cipher (requires `--tls-key-repo`) |
| `--tls-cert-label <LABEL>` | Certificate label (requires `--tls-key-repo`) |

### Producing

| Option | Meaning |
|---|---|
| `-D`, `--delimiter <DELIM>` | Delimiter splitting input into messages (default `\n`) |
| `-K`, `--key-delimiter <DELIM>` | Split input key from message body |
| `-c`, `--count <COUNT>` | Exit after producing this many messages |
| `-p`, `--priority <PRIORITY>` | Message priority |
| `--persistence <yes\|no\|as-queue-default>` | Message persistence |
| `--expiry <unlimited\|MILLIS>` | Message expiry |
| `--message-format <FORMAT>` | MQ format name, for example `MQFMT_STRING` |
| `--ccsid <CCSID>` | CCSID to declare for produced messages; disables encoding detection |
| `--reply-to <QUEUE>` | Reply-to queue |
| `-H`, `--property <NAME=VALUE>` | Set a message property; repeatable |
| `-T`, `--tee` | Echo each input record to stdout as well as putting it |
| `FILE...` | Files to send as whole messages |

### Character encoding of produced messages

Input typed at an interactive terminal is in whatever encoding the console uses,
which is not necessarily UTF-8. `mqcat` detects that encoding and handles it two
ways, so the queue manager and consumers see correctly decoded text:

| Host encoding | Action |
|---|---|
| UTF-8 | Bytes sent unchanged; CCSID is left to the queue manager |
| ANSI/Windows and CJK pages (`encoding_rs` supported) | Decoded to UTF-8; message declared CCSID `1208` |
| OEM/DOS code pages | Bytes sent unchanged; message declared the matching OEM CCSID for MQ to convert |

Piped input and `FILE` arguments are never altered, because they are byte-exact
payloads. `--ccsid` overrides detection entirely and declares the given CCSID
without converting the bytes; use it when the input is already in a known
character set. Invalid input for a detected encoding is rejected rather than
substituted, so a message is never silently corrupted.

### Consuming

| Option | Meaning |
|---|---|
| `-f`, `--format <FORMAT>` | Output format string |
| `-D`, `--delimiter <DELIM>` | Delimiter written after each message (default `\n`) |
| `-c`, `--count <COUNT>` | Exit after consuming this many messages |
| `-e`, `--exit-on-empty` | Exit as soon as no message is immediately available |
| `-w`, `--wait <SECONDS>` | Seconds to wait for a message (default 5) |
| `--browse` | Read messages without removing them |
| `--shared` | Open for shared rather than exclusive input |
| `--syncpoint` | Commit each message using MQ syncpoint |
| `--correl-id <HEX>` | Match only this correlation identifier |
| `--msg-id <HEX>` | Match only this message identifier |

### General

| Option | Meaning |
|---|---|
| `-v`, `--verbose` | Increase logging verbosity; repeatable |
| `-q`, `--quiet` | Suppress informational logging |
| `-V`, `--version` | Print version |
| `-h`, `--help` | Print help |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | A runtime failure: connection, open, get, put, or I/O |
| `2` | A usage error: invalid arguments or argument combinations |

## Examples

```bash
# Consume and show the queue name and priority alongside each body
mqcat -C -t DEV.QUEUE.1 -b DEV.APP.SVRCONN/TCP/localhost\(1414\) -m QM1 \
      -f '%t [%p] %s\n'

# Consume 10 messages as JSON-ish key/value output with properties
mqcat -C -t DEV.QUEUE.1 -b DEV.APP.SVRCONN/TCP/localhost\(1414\) -m QM1 \
      -c 10 -f '{"id":"%i","correl":"%k","props":"%h","body":"%s"}\n'

# Produce a file with a property and a priority
mqcat -P -t DEV.QUEUE.1 -b DEV.APP.SVRCONN/TCP/localhost\(1414\) -m QM1 \
      -H source=mqcat -p 7 message.json

# Produce stdin records separated by a custom delimiter
printf 'alpha|beta|gamma' | mqcat -P -t DEV.QUEUE.1 \
      -b DEV.APP.SVRCONN/TCP/localhost\(1414\) -m QM1 -D '|'

# Declare the character set of input that is already in Latin-1
mqcat -P -t DEV.QUEUE.1 -b DEV.APP.SVRCONN/TCP/localhost\(1414\) -m QM1 \
      --ccsid 819 latin1.txt

# Browse without removing, exiting as soon as the queue is drained
mqcat -C -t DEV.QUEUE.1 -b DEV.APP.SVRCONN/TCP/localhost\(1414\) -m QM1 \
      --browse -e
```

## Differences from kcat worth knowing

* **No partitions or offsets.** MQ delivers messages in queue order and does not
  expose a numeric offset. The message identifier and correlation identifier are
  the identifying fields available.
* **Consuming from topics.** MQ rejects a get handle opened directly on a topic,
  so consuming from a topic string creates a managed, non-durable subscription
  and reads from the temporary queue MQ allocates for it. The subscription ends
  when `mqcat` exits, so messages published while nothing is subscribed are not
  retained. Use `mqi`'s `Subscription` API for durable subscriptions, which keep
  messages across restarts.
* **Key handling.** `-K` splits input into a key and a body. The key is written
  into the message's correlation identifier, which is how MQ applications
  correlate request and reply messages. A key longer than 24 bytes cannot be
  represented as an MQ identifier and is rejected rather than truncated.
* **No consumer groups.** MQ uses queue sharing and `--shared` input handles for
  competing consumers instead.
