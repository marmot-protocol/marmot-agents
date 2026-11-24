//! How the bytes a connector writes are split into lines, none over a limit (D-033), with nothing
//! of the connection in it, so it can be checked on its own: by property tests, by the fuzzer, and
//! by Kani's bounded proofs.

/// Lines from bytes that arrive in parts of any size. A line over `max` bytes is passed over as it
/// arrives, never held whole: at most `max` bytes are held at any time. The bytes after the last
/// newline wait for the next part.
#[derive(Debug)]
pub struct Splitter {
    max: usize,
    line: Vec<u8>,
    too_long: bool,
}

impl Splitter {
    /// A splitter that passes over every line longer than `max` bytes.
    pub fn new(max: usize) -> Self {
        Self {
            max,
            line: Vec::new(),
            too_long: false,
        }
    }

    /// Takes what it can of `part`: up to and with its first newline, or all of it. How many bytes
    /// it took, and the line that newline ended, without the newline, unless that line was too
    /// long. The caller hands the rest of `part` again.
    pub fn take(&mut self, part: &[u8]) -> (usize, Option<Vec<u8>>) {
        let (head, end) = match part.iter().position(|&b| b == b'\n') {
            Some(at) => (&part[..at], Some(at + 1)),
            None => (part, None),
        };
        if !self.too_long {
            if self.line.len() + head.len() > self.max {
                self.too_long = true;
                self.line = Vec::new();
            } else {
                self.line.extend_from_slice(head);
            }
        }
        let Some(end) = end else {
            return (part.len(), None);
        };
        let too_long = std::mem::replace(&mut self.too_long, false);
        let line = std::mem::take(&mut self.line);
        (end, (!too_long).then_some(line))
    }

    /// How many bytes it holds now, of a line not yet ended.
    pub fn held(&self) -> usize {
        self.line.len()
    }
}
