use std::borrow::Cow;
use std::fmt::{self, Display, Write};
use std::str::FromStr;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JsonStrMap {
    buf: String,
    first: bool,
}

impl JsonStrMap {
    pub(crate) fn new() -> JsonStrMap {
        JsonStrMap {
            buf: String::from('{'),
            first: true,
        }
    }

    pub(crate) fn field<D: Display + ?Sized>(mut self, key: &str, value: &D) -> JsonStrMap {
        if self.first {
            self.first = false;
        } else {
            self.buf.push_str(", ");
        }
        write_json_str(key, &mut self.buf).unwrap();
        self.buf.push_str(": ");
        write_json_str(&value.to_string(), &mut self.buf).unwrap();
        self
    }

    pub(crate) fn raw_field(mut self, key: &str, value: &str) -> JsonStrMap {
        if self.first {
            self.first = false;
        } else {
            self.buf.push_str(", ");
        }
        write_json_str(key, &mut self.buf).unwrap();
        self.buf.push_str(": ");
        self.buf.push_str(value);
        self
    }

    pub(crate) fn finish(mut self) -> String {
        self.buf.push('}');
        self.buf
    }
}

impl Default for JsonStrMap {
    fn default() -> JsonStrMap {
        JsonStrMap::new()
    }
}

fn write_json_str<W: Write>(s: &str, writer: &mut W) -> fmt::Result {
    writer.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => writer.write_str("\\\"")?,
            '\\' => writer.write_str(r"\\")?,
            '\x08' => writer.write_str("\\b")?,
            '\x0C' => writer.write_str("\\f")?,
            '\n' => writer.write_str("\\n")?,
            '\r' => writer.write_str("\\r")?,
            '\t' => writer.write_str("\\t")?,
            ' '..='~' => writer.write_char(c)?,
            c => {
                let mut buf = [0u16; 2];
                for b in c.encode_utf16(&mut buf) {
                    write!(writer, "\\u{:04x}", b)?;
                }
            }
        }
    }
    writer.write_char('"')?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum CharEncoding {
    Utf8,
    Utf8Latin1,
    Latin1,
}

impl CharEncoding {
    pub(crate) fn is_utf8(&self) -> bool {
        matches!(self, CharEncoding::Utf8 | CharEncoding::Utf8Latin1)
    }

    pub(crate) fn encode<'a>(&'a self, s: &'a str) -> Cow<'a, [u8]> {
        match self {
            CharEncoding::Utf8 | CharEncoding::Utf8Latin1 => Cow::from(s.as_bytes()),
            CharEncoding::Latin1 => Cow::from(
                s.chars()
                    .map(|c| u8::try_from(c).unwrap_or(0x3F))
                    .collect::<Vec<_>>(),
            ),
        }
    }

    pub(crate) fn decode(&self, bs: Vec<u8>) -> String {
        match self {
            CharEncoding::Utf8 => String::from_utf8_lossy(&bs).into_owned(),
            CharEncoding::Utf8Latin1 => match String::from_utf8(bs) {
                Ok(s) => s,
                Err(e) => decode_latin1(e.into_bytes()),
            },
            CharEncoding::Latin1 => decode_latin1(bs),
        }
    }
}

fn decode_latin1(bs: Vec<u8>) -> String {
    bs.into_iter().map(char::from).collect()
}

impl FromStr for CharEncoding {
    type Err = CharEncodingLookupError;

    fn from_str(s: &str) -> Result<CharEncoding, CharEncodingLookupError> {
        if s.eq_ignore_ascii_case("utf8") {
            Ok(CharEncoding::Utf8)
        } else if s.eq_ignore_ascii_case("utf8-latin1") {
            Ok(CharEncoding::Utf8Latin1)
        } else if s.eq_ignore_ascii_case("latin1") {
            Ok(CharEncoding::Latin1)
        } else {
            Err(CharEncodingLookupError)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CharEncodingLookupError;

impl Display for CharEncodingLookupError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "invalid character encoding name")
    }
}

impl std::error::Error for CharEncodingLookupError {}

pub(crate) fn chomp(s: &str) -> &str {
    let s = s.strip_suffix('\n').unwrap_or(s);
    let s = s.strip_suffix('\r').unwrap_or(s);
    s
}

#[cfg(test)]
mod test {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("foobar", r#""foobar""#)]
    #[case("foo / bar", r#""foo / bar""#)]
    #[case("foo\"bar", r#""foo\"bar""#)]
    #[case("foo\\bar", r#""foo\\bar""#)]
    #[case("foo\x08\x0C\n\r\tbar", r#""foo\b\f\n\r\tbar""#)]
    #[case("foo\x0B\x1B\x7Fbar", r#""foo\u000b\u001b\u007fbar""#)]
    #[case("foo—bar", r#""foo\u2014bar""#)]
    #[case("foo🐐bar", r#""foo\ud83d\udc10bar""#)]
    fn test_write_json_str(#[case] s: &str, #[case] json: String) {
        let mut buf = String::new();
        write_json_str(s, &mut buf).unwrap();
        assert_eq!(buf, json);
    }

    #[test]
    fn test_json_str_map_empty() {
        let s = JsonStrMap::new().finish();
        assert_eq!(s, "{}");
    }

    #[test]
    fn test_json_str_map_one_field() {
        let s = JsonStrMap::new().field("key", "value").finish();
        assert_eq!(s, r#"{"key": "value"}"#);
    }

    #[test]
    fn test_json_str_map_two_fields() {
        let s = JsonStrMap::new()
            .field("key", "value")
            .field("apple", "banana")
            .finish();
        assert_eq!(s, r#"{"key": "value", "apple": "banana"}"#);
    }

    #[rstest]
    #[case("foo", "foo")]
    #[case("foo\n", "foo")]
    #[case("foo\r", "foo")]
    #[case("foo\r\n", "foo")]
    #[case("foo\n\r", "foo\n")]
    #[case("foo\n\n", "foo\n")]
    #[case("foo\nbar", "foo\nbar")]
    #[case("\nbar", "\nbar")]
    fn test_chomp(#[case] s1: &str, #[case] s2: &str) {
        assert_eq!(chomp(s1), s2);
    }

    #[test]
    fn test_encode_latin1() {
        let s = "Snowémon: ☃!";
        assert_eq!(CharEncoding::Latin1.encode(s), &b"Snow\xE9mon: ?!"[..]);
    }

    #[test]
    fn test_decode_latin1() {
        let bs = b"Snow\xE9mon: \xE2\x98\x83!".to_vec();
        assert_eq!(CharEncoding::Latin1.decode(bs), "Snowémon: â\u{98}\u{83}!");
    }

    #[test]
    fn test_decode_utf8() {
        let bs = b"Snow\xC3\xA9mon: \xE2\x98!".to_vec();
        assert_eq!(CharEncoding::Utf8.decode(bs), "Snowémon: \u{fffd}!");
    }

    #[test]
    fn test_decode_utf8latin1_good() {
        let bs = b"Snow\xC3\xA9mon: \xE2\x98\x83!".to_vec();
        assert_eq!(CharEncoding::Utf8Latin1.decode(bs), "Snowémon: ☃!");
    }

    #[test]
    fn test_decode_utf8latin1_fallback() {
        let bs = b"Snow\xC3\xA9mon: \xE2\x98!".to_vec();
        assert_eq!(
            CharEncoding::Utf8Latin1.decode(bs),
            "Snow\u{c3}\u{a9}mon: \u{e2}\u{98}!"
        );
    }
}
