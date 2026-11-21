//! The Marmot side's own keys: a group, an event, a person's public key. They never reach a
//! driver, which sees the core's ids instead (D-036), and they are never printed (D-071).

use serde::{Deserialize, Serialize};

macro_rules! key {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// The key as text, as MDK gives it.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(key: &str) -> Self {
                Self(key.to_string())
            }
        }

        /// Prints nothing of the key: it identifies a group or a person (D-071).
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($name), "(..)"))
            }
        }
    };
}

key!(
    /// A Marmot group, by MDK's group id.
    GroupKey
);
key!(
    /// An event in a group, by its id.
    EventKey
);
key!(
    /// A person, or the agent, by public key.
    PubKey
);

/// The hex form of a public key given in hex or as an npub (NIP-19), checksum checked; `None` when
/// the text is neither. The install scripts ask for it with `marmot-agent key`.
pub fn public_key_hex(text: &str) -> Option<String> {
    let text = text.trim();
    if text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(text.to_ascii_lowercase());
    }
    let checked = bech32::primitives::decode::CheckedHrpstring::new::<bech32::Bech32>(text).ok()?;
    if checked.hrp().to_lowercase() != "npub" {
        return None;
    }
    let bytes: Vec<u8> = checked.byte_iter().collect();
    (bytes.len() == 32).then(|| hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::public_key_hex;

    // NIP-19's own example.
    const NPUB: &str = "npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg";
    const HEX: &str = "7e7e9c42a91bfef19fa929e5fda1b72e0ebc1a4c1141673e2794234d86addf4e";

    #[test]
    fn reads_hex_and_npub() {
        assert_eq!(public_key_hex(NPUB).as_deref(), Some(HEX));
        assert_eq!(public_key_hex(&NPUB.to_uppercase()).as_deref(), Some(HEX));
        assert_eq!(public_key_hex(&HEX.to_uppercase()).as_deref(), Some(HEX));
    }

    #[test]
    fn refuses_what_is_not_a_public_key() {
        let mut bad_checksum = NPUB.to_string();
        bad_checksum.pop();
        bad_checksum.push('q');
        let nsec = "nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5";
        for text in ["", "abc", &"zz".repeat(32), &bad_checksum, nsec, &HEX[..62]] {
            assert_eq!(public_key_hex(text), None, "{text}");
        }
    }
}
