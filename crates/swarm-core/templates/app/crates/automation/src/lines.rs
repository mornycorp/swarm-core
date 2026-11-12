//! Lines in, lines out, with the cap on the way in. A reader that grows a buffer until a newline
//! lets one hostile peer take all the memory, so [`read_line`] stops keeping bytes at the cap and
//! discards the rest of the line.

use std::io::{self, BufRead, Write};

use crate::limits::MAX_REQUEST;

/// One line of a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// The line without its `\n` (and a `\r` before it).
    Text(String),
    /// Longer than the cap: the rest of it has been discarded, up to the newline.
    TooLarge,
    /// Not UTF-8.
    NotUtf8,
    /// The stream ended.
    Eof,
}

/// The next line of `reader`, at most `max` bytes of it kept. The last line of a stream needs no
/// newline.
pub fn read_line(reader: &mut impl BufRead, max: usize) -> io::Result<Line> {
    let mut kept: Vec<u8> = Vec::new();
    let (mut over, mut seen) = (false, false);
    loop {
        let (take, ended) = match chunk(reader, &mut kept, &mut over, max) {
            Ok(Some(step)) => step,
            Ok(None) => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        seen = true;
        reader.consume(take);
        if ended {
            break;
        }
    }
    Ok(finish(kept, over, seen))
}

/// Looks at the reader's buffer: keeps what belongs to the line (unless it is over `max`), and
/// says how many bytes to consume and whether the line ended. `None`: the stream ended.
fn chunk(reader: &mut impl BufRead, kept: &mut Vec<u8>, over: &mut bool, max: usize) -> io::Result<Option<(usize, bool)>> {
    let buf = reader.fill_buf()?;
    if buf.is_empty() {
        return Ok(None);
    }
    let newline = buf.iter().position(|b| *b == b'\n');
    let content = newline.unwrap_or(buf.len());
    if !*over {
        if kept.len().saturating_add(content) > max {
            *over = true;
            kept.clear();
        } else {
            kept.extend_from_slice(buf.get(..content).unwrap_or_default());
        }
    }
    Ok(Some((newline.map_or(content, |at| at + 1), newline.is_some())))
}

fn finish(mut kept: Vec<u8>, over: bool, seen: bool) -> Line {
    if !seen {
        return Line::Eof;
    }
    if over {
        return Line::TooLarge;
    }
    if kept.last() == Some(&b'\r') {
        kept.pop();
    }
    String::from_utf8(kept).map_or(Line::NotUtf8, Line::Text)
}

/// `text` and a newline, flushed.
pub fn write_line(out: &mut impl Write, text: &str) -> io::Result<()> {
    out.write_all(text.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

/// The loop of every line transport: read a line (capped at [`MAX_REQUEST`]), let `answer` say what
/// to write for it (nothing, for a blank line or a notification), write it. Ends at the end of
/// the input.
pub fn exchange(mut input: impl BufRead, mut output: impl Write, mut answer: impl FnMut(Line) -> Option<String>) -> io::Result<()> {
    loop {
        let line = read_line(&mut input, MAX_REQUEST)?;
        if line == Line::Eof {
            return Ok(());
        }
        if let Some(reply) = answer(line) {
            write_line(&mut output, &reply)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn lines(bytes: &[u8], max: usize) -> Vec<Line> {
        let mut reader = Cursor::new(bytes.to_vec());
        let mut out = Vec::new();
        loop {
            match read_line(&mut reader, max).unwrap() {
                Line::Eof => return out,
                line => out.push(line),
            }
        }
    }

    fn text(s: &str) -> Line {
        Line::Text(s.to_owned())
    }

    #[test]
    fn lines_split_on_newlines_and_the_last_needs_none() {
        assert_eq!(lines(b"a\nbb\r\n\nlast", 10), [text("a"), text("bb"), text(""), text("last")]);
        assert!(lines(b"", 10).is_empty());
        assert_eq!(lines(b"\n", 10), [text("")]);
    }

    #[test]
    fn a_line_over_the_cap_is_discarded_to_its_newline_and_the_next_is_read() {
        assert_eq!(lines(b"0123456789\nab\n", 10), [text("0123456789"), text("ab")], "exactly the cap is fine");
        assert_eq!(lines(b"01234567890\nab\n", 10), [Line::TooLarge, text("ab")]);
        assert_eq!(lines(b"01234567890", 10), [Line::TooLarge], "no newline at the end");
        let big = vec![b'x'; 5 * 1024 * 1024];
        let mut input = big.clone();
        input.extend_from_slice(b"\nok\n");
        assert_eq!(lines(&input, MAX_REQUEST), [Line::TooLarge, text("ok")]);
    }

    #[test]
    fn a_line_that_is_not_utf8_is_reported_and_the_stream_goes_on() {
        assert_eq!(lines(b"\xff\xfe\nok\n", 10), [Line::NotUtf8, text("ok")]);
    }

    /// A reader that hands out one byte at a time, as a slow pipe may.
    struct Trickle(Cursor<Vec<u8>>);
    impl io::Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let one = buf.len().min(1);
            self.0.read(buf.get_mut(..one).unwrap_or_default())
        }
    }

    #[test]
    fn a_slow_reader_gives_the_same_lines() {
        let mut reader = io::BufReader::with_capacity(1, Trickle(Cursor::new(b"ab\ncd".to_vec())));
        assert_eq!(read_line(&mut reader, 10).unwrap(), text("ab"));
        assert_eq!(read_line(&mut reader, 10).unwrap(), text("cd"));
        assert_eq!(read_line(&mut reader, 10).unwrap(), Line::Eof);
    }

    #[test]
    fn exchange_writes_one_line_per_answer() {
        let mut out = Vec::new();
        let answer = |line: Line| match line {
            Line::Text(t) if t.is_empty() => None,
            Line::Text(t) => Some(t.to_uppercase()),
            other => Some(format!("{other:?}")),
        };
        exchange(Cursor::new(b"a\n\nb\n".to_vec()), &mut out, answer).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "A\nB\n");
    }
}
