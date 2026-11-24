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

/// Bounded proofs (`docs/design/verification.md`, "5. Bounded proofs with Kani"): two parts of up
/// to 3 bytes each, after any line held so far, with a limit of up to 3.
#[cfg(kani)]
mod proofs {
    use super::Splitter;

    /// S11, D-033: each take takes something of a part that has something, within it; ends a line
    /// at the first newline and only there, never over the limit; and holds no more than it.
    #[kani::proof]
    #[kani::unwind(8)]
    fn s11_a_take_ends_lines_at_newlines_and_holds_no_more_than_the_limit() {
        let max: usize = kani::any();
        kani::assume(max <= 3);
        let mut splitter = Splitter::new(max);
        for _ in 0..2 {
            let bytes: [u8; 3] = kani::any();
            let length: usize = kani::any();
            kani::assume(length <= 3);
            let part = &bytes[..length];
            let held = splitter.held();
            let (used, line) = splitter.take(part);
            assert!(splitter.held() <= max);
            if part.is_empty() {
                assert_eq!(used, 0);
                continue;
            }
            assert!(used >= 1 && used <= part.len());
            match part.iter().position(|&b| b == b'\n') {
                Some(at) => {
                    assert_eq!(used, at + 1);
                    assert_eq!(splitter.held(), 0);
                    if let Some(line) = line {
                        assert!(line.len() <= max);
                        assert_eq!(line.len(), held + at);
                        assert!(line[held..] == part[..at]);
                    }
                }
                None => {
                    assert_eq!(used, part.len());
                    assert!(line.is_none());
                }
            }
        }
    }
}
