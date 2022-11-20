//! This is a derivative of tokio-util's `LinesCodec` (obtained from
//! [`lines_codec.rs`][1]) adjusted as follows:
//!
//! - Decoder: If the buffer exceeds the maximum line length and does not
//!   contain a newline, a break is inserted at the max line length (adjusted
//!   backwards if necessary so as not to break up any UTF-8 sequences) rather
//!   than returning an error.  As a result, the error type is now
//!   `std::io::Error` (unused) instead of the custom `LinesCodecError`.
//! - The Decoder does not strip line endings from returned values.
//! - The caller must append the line ending before passing the value to the
//!   Encoder.
//! - Conversion between bytes & strings is handled by CharEncoding.
//!
//! [1]: https://github.com/tokio-rs/tokio/blob/a03e0420249d1740668f608a5a16f1fa614be2c7/tokio-util/src/codec/lines_codec.rs

// Copyright (c) 2022 Tokio Contributors
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use crate::util::CharEncoding;
use bytes::{BufMut, BytesMut};
use std::borrow::Borrow;
use std::{cmp, io};
use tokio_util::codec::{Decoder, Encoder};

/// A simple [`Decoder`] and [`Encoder`] implementation that splits up data into lines.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct NetlionCodec {
    // Stored index of the next index to examine for a `\n` character.
    // This is used to optimize searching.
    // For example, if `decode` was called with `abc`, it would hold `3`,
    // because that is the next index to examine.
    // The next time `decode` is called with `abcde\n`, the method will
    // only look at `de\n` before returning.
    next_index: usize,

    /// The maximum length for a given line. If `usize::MAX`, lines will be
    /// read until a `\n` character is reached.
    max_length: usize,

    /// Character encoding for converting between strings and bytes
    encoding: CharEncoding,
}

impl NetlionCodec {
    /// Returns a `NetlionCodec` for splitting up data into lines.
    ///
    /// # Note
    ///
    /// The returned `NetlionCodec` will not have an upper bound on the length
    /// of a buffered line. See the documentation for [`new_with_max_length`]
    /// for information on why this could be a potential security risk.
    pub(crate) fn new() -> NetlionCodec {
        NetlionCodec {
            next_index: 0,
            max_length: usize::MAX,
            encoding: CharEncoding::Utf8,
        }
    }

    /// Returns a `NetlionCodec` with a maximum line length limit.
    ///
    /// # Note
    ///
    /// Setting a length limit is highly recommended for any `NetlionCodec` which
    /// will be exposed to untrusted input. Otherwise, the size of the buffer
    /// that holds the line currently being read is unbounded. An attacker could
    /// exploit this unbounded buffer by sending an unbounded amount of input
    /// without any `\n` characters, causing unbounded memory consumption.
    pub(crate) fn new_with_max_length(max_length: usize) -> Self {
        NetlionCodec {
            max_length,
            ..NetlionCodec::new()
        }
    }

    pub(crate) fn encoding(self, encoding: CharEncoding) -> NetlionCodec {
        NetlionCodec { encoding, ..self }
    }
}

impl Decoder for NetlionCodec {
    type Item = String;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<String>, io::Error> {
        // Determine how far into the buffer we'll search for a newline. If
        // there's no max_length set, we'll read to the end of the buffer.
        let read_to = cmp::min(self.max_length.saturating_add(1), buf.len());
        let newline_offset = buf[self.next_index..read_to]
            .iter()
            .position(|b| *b == b'\n');
        match newline_offset {
            Some(offset) => {
                // Found a line!
                let newline_index = offset + self.next_index;
                self.next_index = 0;
                let line = buf.split_to(newline_index + 1);
                let line = self.encoding.decode(line.into());
                Ok(Some(line))
            }
            None if buf.len() > self.max_length => {
                // TODO: Strip off trailing UTF-8 fragment!
                self.next_index = 0;
                let line = buf.split_to(self.max_length);
                let line = self.encoding.decode(line.into());
                Ok(Some(line))
            }
            None => {
                // We didn't find a line or reach the length limit, so the next
                // call will resume searching at the current offset.
                self.next_index = read_to;
                Ok(None)
            }
        }
    }

    fn decode_eof(&mut self, buf: &mut BytesMut) -> Result<Option<String>, io::Error> {
        Ok(match self.decode(buf)? {
            Some(frame) => Some(frame),
            None => {
                // No terminating newline - return remaining data, if any
                if buf.is_empty() {
                    None
                } else {
                    let line = buf.split_to(buf.len());
                    let line = self.encoding.decode(line.into());
                    self.next_index = 0;
                    Some(line)
                }
            }
        })
    }
}

impl<T> Encoder<T> for NetlionCodec
where
    T: AsRef<str>,
{
    type Error = io::Error;

    fn encode(&mut self, line: T, buf: &mut BytesMut) -> Result<(), io::Error> {
        let line = self.encoding.encode(line.as_ref());
        let lineref: &[u8] = line.borrow();
        buf.reserve(lineref.len());
        buf.put(lineref);
        Ok(())
    }
}

impl Default for NetlionCodec {
    fn default() -> Self {
        Self::new()
    }
}
