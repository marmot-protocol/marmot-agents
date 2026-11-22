//! The simulator's own random numbers: SplitMix64, written here so a schedule made from a seed
//! never changes with a dependency's version.

/// A small generator, the same on every machine.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator started from `seed`.
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next number.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `n`, which must not be 0.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    /// True once in `n` times.
    pub fn one_in(&mut self, n: u64) -> bool {
        self.below(n) == 0
    }

    /// One of `choices`, each as likely as its weight. The weights must not all be 0.
    pub fn weighted(&mut self, weights: &[u32]) -> usize {
        let total: u64 = weights.iter().map(|w| u64::from(*w)).sum();
        let mut at = self.below(total);
        for (i, w) in weights.iter().enumerate() {
            let w = u64::from(*w);
            if at < w {
                return i;
            }
            at -= w;
        }
        weights.len() - 1
    }

    /// One of `items`.
    pub fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

/// Mixes the parts of a run's name into one seed.
pub fn mix(parts: &[u64]) -> u64 {
    let mut rng = Rng::new(0x6d61_726d_6f74);
    let mut seed = 0;
    for part in parts {
        rng = Rng::new(rng.next_u64() ^ part);
        seed = rng.next_u64();
    }
    seed
}

/// The bytes of a name, as a number, to mix into a seed.
pub fn hash(text: &str) -> u64 {
    // FNV-1a.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
