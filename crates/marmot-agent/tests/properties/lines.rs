//! The socket's line reader (D-033, S11): however the bytes a connector writes arrive, in parts
//! of any size, the lines read are the lines written, each whole, none over the limit, and no
//! more than the limit is ever held.

use marmot_agent::socket::lines::Splitter;
use proptest::prelude::*;

/// What the reader should give: every line a newline ends, without it, that is not over `max`.
fn reference(bytes: &[u8], max: usize) -> Vec<Vec<u8>> {
    let mut lines: Vec<Vec<u8>> = bytes.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect();
    lines.pop(); // what no newline ended yet
    lines.retain(|l| l.len() <= max);
    lines
}

fn bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(prop_oneof![3 => any::<u8>(), 1 => Just(b'\n')], 0..400)
}

proptest! {
    #[test]
    fn s11_lines_arrive_whole_in_parts_of_any_size(
        bytes in bytes(),
        max in 0..40usize,
        cuts in prop::collection::vec(1..64usize, 1..20),
    ) {
        let mut splitter = Splitter::new(max);
        let mut lines = Vec::new();
        let mut rest = &bytes[..];
        let mut cut = cuts.iter().cycle();
        while !rest.is_empty() {
            // A read gives a part of what is buffered; the reader takes from it until it is used.
            let size = (*cut.next().expect("cycles")).min(rest.len());
            let mut part = &rest[..size];
            while !part.is_empty() {
                let (used, line) = splitter.take(part);
                prop_assert!(used > 0 && used <= part.len(), "takes something, within the part");
                prop_assert!(splitter.held() <= max, "holds {} bytes, over {}", splitter.held(), max);
                if let Some(line) = line {
                    prop_assert!(line.len() <= max);
                    lines.push(line);
                }
                part = &part[used..];
            }
            rest = &rest[size..];
        }
        prop_assert_eq!(lines, reference(&bytes, max));
    }
}
