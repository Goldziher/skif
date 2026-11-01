//! A small seeded `SplitMix64` generator, so generated workspaces depend on nothing but the seed.

#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// An independent stream for `stream` (e.g. one package), derived from `seed`.
    pub fn for_stream(seed: u64, stream: u64) -> Self {
        let mut mixer = Self::new(seed ^ stream.wrapping_mul(0xD6E8_FEB8_6659_FD93));
        Self::new(mixer.next_u64())
    }

    pub const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..bound`; `bound == 0` yields `0`.
    pub const fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 { 0 } else { self.next_u64() % bound }
    }

    pub fn below_usize(&mut self, bound: usize) -> usize {
        usize::try_from(self.below(bound as u64)).unwrap_or(0)
    }

    /// Uniform in `lo..=hi`.
    pub fn range_usize(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below_usize(hi.saturating_sub(lo) + 1)
    }

    /// Uniform in `[0, 1)`.
    #[allow(clippy::cast_precision_loss, reason = "53 random bits fit an f64 mantissa exactly")]
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn chance(&mut self, probability: f64) -> bool {
        self.unit() < probability
    }
}

/// FNV-1a, used to derive stable per-name properties.
pub fn hash_str(value: &str) -> u64 {
    value.bytes().fold(0xCBF2_9CE4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01B3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        assert!((0..100).all(|_| a.next_u64() == b.next_u64()));
    }

    #[test]
    fn streams_differ() {
        assert_ne!(Rng::for_stream(1, 0).next_u64(), Rng::for_stream(1, 1).next_u64());
    }

    #[test]
    fn unit_is_in_range() {
        let mut rng = Rng::new(3);
        assert!((0..1000).all(|_| (0.0..1.0).contains(&rng.unit())));
    }

    #[test]
    fn below_respects_bound() {
        let mut rng = Rng::new(3);
        assert!((0..1000).all(|_| rng.below(10) < 10));
        assert_eq!(rng.below(0), 0);
    }
}
