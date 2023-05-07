v0.1.0 (in development)
-----------------------
- MSRV increased to 1.65
- Added extended `--help` output for `--encoding` and `--max-line-length`
- Removed the `-M` short form of the `--max-line-length` option
- When `--encoding latin1` is in effect and the user inputs a line containing
  non-Latin-1 characters, the echo of the sent data — along with the transcript
  — will now show those characters as `?` so that they match the text actually
  sent to the server
- Use [`cargo-dist`](https://github.com/axodotdev/cargo-dist) for building
  release assets and installers
- Include third party licenses in release assets

v0.1.0-alpha (2022-12-04)
-------------------------
Alpha release
