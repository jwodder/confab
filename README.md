[![Project Status: Active – The project has reached a stable, usable state and is being actively developed.](https://www.repostatus.org/badges/latest/active.svg)](https://www.repostatus.org/#active)
[![CI Status](https://github.com/jwodder/confab/actions/workflows/test.yml/badge.svg)](https://github.com/jwodder/confab/actions/workflows/test.yml)
[![codecov.io](https://codecov.io/gh/jwodder/confab/branch/master/graph/badge.svg)](https://codecov.io/gh/jwodder/confab)
[![Minimum Supported Rust Version](https://img.shields.io/badge/MSRV-1.86-orange)](https://www.rust-lang.org)
[![MIT License](https://img.shields.io/github/license/jwodder/confab.svg)](https://opensource.org/licenses/MIT)

[GitHub](https://github.com/jwodder/confab) | [crates.io](https://crates.io/crates/confab) | [Issues](https://github.com/jwodder/confab/issues) | [Changelog](https://github.com/jwodder/confab/blob/master/CHANGELOG.md)

`confab` is an asynchronous line-oriented interactive TCP client with TLS
support.  Use it to connect to a TCP server, and you'll be able to send
messages line by line while lines received from the remote server are printed
above the prompt.

![Recording of an example session](https://github.com/jwodder/confab/raw/master/example.gif)

Installation
============

Release Assets
--------------

Prebuilt binaries for the most common platforms are available as GitHub release
assets.  [The page for the latest
release](https://github.com/jwodder/confab/releases/latest) lists these under
"Assets", along with installer scripts for both Unix-like systems and Windows.

As an alternative to the installer scripts, if you have
[`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall) on your
system, you can use it to download & install the appropriate release asset for
your system for the latest version of `confab` by running `cargo binstall
confab`.

Cargo
-----

If you have [Rust and Cargo
installed](https://www.rust-lang.org/tools/install), you can build the latest
release of `confab` and install it in `~/.cargo/bin` by running:

    cargo install confab

`confab` has the following Cargo features, selectable via the `--features
<LIST>` option to `cargo install`:

- `rustls` — Use [`rustls`](https://github.com/rustls/rustls) for TLS support.

    - This feature is enabled by default, and it overrides any other features;
      hence, in order to enable the `native` or `vendored-openssl` feature, the
      `--no-default-features` option must be passed to `cargo install` in
      addition to the `--features ...` option.

    - The release assets are built with this feature enabled.

- `native` — Use [`native-tls`](https://github.com/sfackler/rust-native-tls)
  for TLS support.

- `vendored-openssl` — Compile a vendored copy of OpenSSL into `confab` instead
  of using the platform's copy at runtime.  This makes it possible to build
  `confab` on one system and run it on another system that has a different
  version of OpenSSL.

    - This feature implies the `native` feature.

    - This option is not meaningful on macOS or Windows, on which `confab` does
      not use OpenSSL for TLS connections.


Usage
=====

    confab [<options>] <host> <port>

Open a TCP connection to the given host and port.  Lines entered by the user at
the `confab` prompt are sent to the remote server and echoed locally with a
"`>`" prefix, while lines received from the remote server are printed out above
the prompt with a "`<`" prefix.  Communication stops when the remote server
closes the connection or when the user presses Ctrl-D.

`confab` relies on
[`rustyline-async`](https://github.com/zyansheep/rustyline-async) for its
readline-like capabilities; see there for the supported control sequences.

Options
-------

- `--build-info` — Display a summary of the program's build information &
  dependencies and exit

- `--crlf` — Append CR LF (`"\r\n"`) to each line sent to the remote server
  instead of just LF (`"\n"`)

- `-E <encoding>`, `--encoding <encoding>` — Set the text encoding for the
  connection.  The available options are:

    - `utf8` *(default)* — Use UTF-8.  If a line received from the remote
      server contains an invalid UTF-8 sequence, the sequence is replaced with
      U+FFFD REPLACEMENT CHARACTER (`�`).

    - `utf8-latin1` — Use UTF-8.  If a line received from the remote server
      contains an invalid UTF-8 sequence, the entire line is instead decoded as
      Latin-1.  (Useful for IRC!)

    - `latin1` — Use Latin-1 (a.k.a. ISO-8859-1).  If a line sent to the remote
      server contains non-Latin-1 characters, they are replaced with question
      marks (`?`).

- `-h`, `--help` — Display a summary of the command-line options and exit

- `--max-line-length <LIMIT>` — Set the maximum length in bytes of each line
  read from the remote server (including the terminating newline).  If the
  server sends a line longer than this, the first `<LIMIT>` bytes will be split
  off and treated as a whole line, with the remaining bytes treated as the
  start of a new line.  [default value: 65535]

- `--servername <DOMAIN>` — (with `--tls`) Use the given domain name for SNI
  and certificate hostname validation; defaults to the remote host name

- `--startup-wait-ms <INT>` — Specify the time to wait in milliseconds
  before sending each line of the startup script [default value: 500]

- `-S <FILE>`, `--startup-script <FILE>` — On startup, read lines from the
  given file and send them to the server one at a time.  The user will not be
  prompted for input until after the end of the file is reached.

- `-t`, `--show-times` — Prepend a timestamp of the form `[HH:MM:SS]` to each
  line printed to the terminal

- `--tls` — Connect using SSL/TLS

- `-T <FILE>`, `--transcript <FILE>` — Append a transcript of events to the
  given file.  See [Transcript Format](#transcript-format) below for more
  information.

- `-V`, `--version` — Show the program version and exit


Transcript Format
=================

The session transcripts produced by the `--transcript` option take the form of
JSON Lines (a.k.a. newline-delimited JSON), that is, a series of lines with one
JSON object per line.  Each JSON object represents an event such as a line
sent, a line received, or the start or end of the connection.

Each object contains, at minimum, a `"timestamp"` field containing a timestamp
for the event in the form `"YYYY-MM-DDTHH:MM:SS.ssssss+HH:MM"` and an `"event"`
field identifying the type of event.  The possible values for the `"event"`
field, along with any accompanying further fields, are as follows:

- `"connection-start"` — Emitted just before starting to connect to the remote
  server.  The event object also contains `"host"` and `"port"` fields listing
  the remote host & port specified on the command line.

- `"connection-complete"` — Emitted after connecting successfully (but before
  negotiating TLS, if applicable).  The event object also contains a
  `"peer_ip"` field listing the remote IP address that the connection was made
  to.

- `"tls-start"` — Emitted before starting the TLS handshake.  The event object
  has no additional fields.

- `"tls-complete"` — Emitted after completing the TLS handshake.  The event
  object has no additional fields.

- `"recv"` — Emitted whenever a line is received from the remote server.  The
  event object also contains a `"data"` field giving the line received,
  including trailing newline (if any).

- `"send"` — Emitted whenever a line is send to the remote server.  The event
  object also contains a `"data"` field giving the line sent, including
  trailing newline (if any).

- `"disconnect"` — Emitted when the connection is closed normally.  The event
  object has no additional fields.

- `"error"` — Emitted when a fatal error occurs.  The event object also
  contains a `"data"` field giving a human-readable error message.
