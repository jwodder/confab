use std::fmt::{Display, Result, Write};

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

fn write_json_str<W: Write>(s: &str, writer: &mut W) -> Result {
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
}
