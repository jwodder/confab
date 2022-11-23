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
//! - Decoder: max_length now includes the terminating newline.
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
use std::{cmp, io};
use tokio_util::codec::{Decoder, Encoder};

/// A simple [`Decoder`] and [`Encoder`] implementation that splits up data into lines.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct ConfabCodec {
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

impl ConfabCodec {
    /// Returns a `ConfabCodec` for splitting up data into lines.
    ///
    /// # Note
    ///
    /// The returned `ConfabCodec` will not have an upper bound on the length
    /// of a buffered line. See the documentation for [`new_with_max_length`]
    /// for information on why this could be a potential security risk.
    pub(crate) fn new() -> ConfabCodec {
        ConfabCodec {
            next_index: 0,
            max_length: usize::MAX,
            encoding: CharEncoding::Utf8,
        }
    }

    /// Returns a `ConfabCodec` with a maximum line length limit.
    ///
    /// # Note
    ///
    /// Setting a length limit is highly recommended for any `ConfabCodec` which
    /// will be exposed to untrusted input. Otherwise, the size of the buffer
    /// that holds the line currently being read is unbounded. An attacker could
    /// exploit this unbounded buffer by sending an unbounded amount of input
    /// without any `\n` characters, causing unbounded memory consumption.
    pub(crate) fn new_with_max_length(max_length: usize) -> Self {
        ConfabCodec {
            max_length,
            ..ConfabCodec::new()
        }
    }

    pub(crate) fn encoding(self, encoding: CharEncoding) -> ConfabCodec {
        ConfabCodec { encoding, ..self }
    }
}

impl Decoder for ConfabCodec {
    type Item = String;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<String>, io::Error> {
        // Determine how far into the buffer we'll search for a newline. If
        // there's no max_length set, we'll read to the end of the buffer.
        let read_to = cmp::min(self.max_length, buf.len());
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
            None if buf.len() >= self.max_length => {
                self.next_index = 0;
                let i = if self.encoding.is_utf8() {
                    find_final_char_boundary(&buf[..self.max_length])
                } else {
                    self.max_length
                };
                let line = buf.split_to(i);
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

impl<T> Encoder<T> for ConfabCodec
where
    T: AsRef<str>,
{
    type Error = io::Error;

    fn encode(&mut self, line: T, buf: &mut BytesMut) -> Result<(), io::Error> {
        let line = self.encoding.encode(line.as_ref());
        buf.reserve(line.len());
        buf.put(&*line);
        Ok(())
    }
}

impl Default for ConfabCodec {
    fn default() -> Self {
        Self::new()
    }
}

/// If `buf` ends in an incomplete UTF-8 sequence (that is, a sequence that is
/// not a valid UTF-8 sequence but which could become one by appending
/// continuation bytes, ignoring the problem of overlong encodings), return the
/// index of the start of that sequence; otherwise, return the length of `buf`.
fn find_final_char_boundary(buf: &[u8]) -> usize {
    for (i, b) in buf.iter().enumerate().rev() {
        // Number of continuation bytes previously iterated over so far:
        let seen = buf.len() - i - 1;
        if (0x80..0xC0).contains(b) && seen < 3 {
            continue;
        } else if (0xC0..0xE0).contains(b) && seen < 1
            || (0xE0..0xF0).contains(b) && seen < 2
            || (0xF0..0xF8).contains(b) && seen < 3
        {
            return i;
        } else {
            return buf.len();
        }
    }
    buf.len()
}

#[cfg(test)]
mod test {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(b"", 0)]
    #[case(b"foo", 3)]
    #[case(b"foo\xE2\x98\x83", 6)]
    #[case(b"foo\xE2\x98", 3)]
    #[case(b"foo\xE2", 3)]
    #[case(b"foo\x98\x83", 5)]
    #[case(b"\x98\x83", 2)]
    #[case(b"\x80\x98\x83", 3)]
    #[case(b"\x80\x80\x98\x83", 4)]
    #[case(b"foo\xC0\x80", 5)]
    #[case(b"foo\xC0\x80\x80", 6)]
    #[case(b"foo\xC0", 3)]
    #[case(b"foo\xF0\x80\x80", 3)]
    #[case(b"foo\x80\x80\x80", 6)]
    #[case(b"foo\x80\x80\x80\x80", 7)]
    #[case(b"foo\xFF", 4)]
    #[case(b"foo\xFC", 4)]
    #[case(b"foo\xFC\x80\x80\x80", 7)]
    #[case(b"foo\xFC\x80\x80\x80\x80\x80", 9)]
    fn test_find_final_char_boundary(#[case] buf: &[u8], #[case] i: usize) {
        assert_eq!(find_final_char_boundary(buf), i);
    }

    #[test]
    fn test_decode_end_before_limit() {
        let mut codec = ConfabCodec::new_with_max_length(32);
        let mut buf = BytesMut::from("This is test text.\nAnd so is this.\n");
        assert_eq!(
            codec.decode(&mut buf).unwrap().unwrap(),
            "This is test text.\n"
        );
        assert_eq!(buf, "And so is this.\n");
    }

    #[test]
    fn test_decode_end_at_limit() {
        let mut codec = ConfabCodec::new_with_max_length(32);
        let mut buf = BytesMut::from("123456789.abcdefghi.123456789.a\nbcdef");
        assert_eq!(
            codec.decode(&mut buf).unwrap().unwrap(),
            "123456789.abcdefghi.123456789.a\n"
        );
        assert_eq!(buf, "bcdef");
    }

    #[test]
    fn test_decode_end_right_after_limit() {
        let mut codec = ConfabCodec::new_with_max_length(32);
        let mut buf = BytesMut::from("123456789.abcdefghi.123456789.ab\ncdef");
        assert_eq!(
            codec.decode(&mut buf).unwrap().unwrap(),
            "123456789.abcdefghi.123456789.ab"
        );
        assert_eq!(buf, "\ncdef");
    }

    #[test]
    fn test_decode_end_after_limit() {
        let mut codec = ConfabCodec::new_with_max_length(32);
        let mut buf = BytesMut::from("123456789.abcdefghi.123456789.abcdef\n");
        assert_eq!(
            codec.decode(&mut buf).unwrap().unwrap(),
            "123456789.abcdefghi.123456789.ab"
        );
        assert_eq!(buf, "cdef\n");
    }

    #[test]
    fn test_decode_max_length_no_end() {
        let mut codec = ConfabCodec::new_with_max_length(32);
        let mut buf = BytesMut::from("123456789.abcdefghi.123456789.ab");
        assert_eq!(
            codec.decode(&mut buf).unwrap().unwrap(),
            "123456789.abcdefghi.123456789.ab"
        );
        assert_eq!(buf, "");
    }

    #[test]
    fn test_decode_max_length_plus_1_no_end() {
        let mut codec = ConfabCodec::new_with_max_length(32);
        let mut buf = BytesMut::from("123456789.abcdefghi.123456789.abc");
        assert_eq!(
            codec.decode(&mut buf).unwrap().unwrap(),
            "123456789.abcdefghi.123456789.ab"
        );
        assert_eq!(buf, "c");
    }

    #[test]
    fn test_decode_max_length_minus_1_no_end() {
        let mut codec = ConfabCodec::new_with_max_length(32);
        let mut buf = BytesMut::from("123456789.abcdefghi.123456789.a");
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
        assert_eq!(buf, "123456789.abcdefghi.123456789.a");
        assert_eq!(codec.next_index, 31);
    }

    #[test]
    fn test_decode_over_max_length_straddling_utf8() {
        let mut codec = ConfabCodec::new_with_max_length(32);
        let mut buf = BytesMut::from(&b"123456789.abcdefghi.123456789.\xE2\x98\x83"[..]);
        assert_eq!(
            codec.decode(&mut buf).unwrap().unwrap(),
            "123456789.abcdefghi.123456789."
        );
        assert_eq!(buf, &b"\xE2\x98\x83"[..]);
    }

    #[test]
    fn test_decode_over_max_length_straddling_utf8_in_latin1() {
        let mut codec = ConfabCodec::new_with_max_length(32).encoding(CharEncoding::Latin1);
        let mut buf = BytesMut::from(&b"123456789.abcdefghi.123456789.\xE2\x98\x83"[..]);
        assert_eq!(
            codec.decode(&mut buf).unwrap().unwrap(),
            "123456789.abcdefghi.123456789.\u{e2}\u{98}"
        );
        assert_eq!(buf, &b"\x83"[..]);
    }
}
