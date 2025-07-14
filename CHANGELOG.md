v0.4.0 (in development)
-----------------------
- Increased MSRV to 1.86

v0.3.2 (2025-05-17)
-------------------
- Remove unintended "openssl" feature
- Increased MSRV to 1.80
- Linux release artifacts are now built on Ubuntu 22.04 (up from Ubuntu 20.04),
  which may result in a more recent glibc being required
- The sending halves of TCP connections are now explicitly shut down when
  disconnecting
- Provide release assets for aarch64-unknown-linux-gnu

v0.3.1 (2023-12-13)
-------------------
- A "Disconnect" event is no longer emitted if an I/O error occurs

v0.3.0 (2023-12-12)
-------------------
- Increased MSRV to 1.70
- Added `--startup-script` and `--startup-wait-ms` options

v0.2.0 (2023-06-03)
-------------------
- `confab` is now built using [`rustls`](https://github.com/rustls/rustls)
  instead of [`native-tls`](https://github.com/sfackler/rust-native-tls) by
  default; `native-tls` can still be used via feature flags

v0.1.1 (2023-05-28)
-------------------
- Fix release workflow

v0.1.0 (2023-05-28)
-------------------
- Increased MSRV to 1.65
- Added extended `--help` output for `--encoding` and `--max-line-length`
- Removed the `-M` short form of the `--max-line-length` option
- When `--encoding latin1` is in effect and the user inputs a line containing
  non-Latin-1 characters, the echo of the sent data — along with the transcript
  — will now show those characters as `?` so that they match the text actually
  sent to the server
- Use [`cargo-dist`](https://github.com/axodotdev/cargo-dist) for building
  release assets and installers
- Include third party licenses in release assets
- Added a `--build-info` option for displaying build information & dependencies

v0.1.0-alpha (2022-12-04)
-------------------------
Alpha release
