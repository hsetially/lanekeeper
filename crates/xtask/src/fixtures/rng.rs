//! A small seeded generator (`SplitMix64`). Fixtures must be reproducible on every machine and every release of every
//! dependency, so the generator is part of this crate instead of an external PRNG whose output could change.

/// Deterministic pseudo-random numbers. Not for anything security related.
#[derive(Debug, Clone)]
pub struct Rng {
    /// Identifies the stream; `fork` derives children from it, so a child never depends on how much the parent drew.
    key: u64,
    state: u64,
}

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            key: seed,
            state: seed,
        }
    }

    /// An independent stream named by `label`. The same parent and label always give the same stream.
    #[must_use]
    pub fn fork(&self, label: &str) -> Self {
        // FNV-1a over the label, mixed into the parent's key.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in label.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        let mut child = Self {
            key: 0,
            state: self.key ^ h.rotate_left(29),
        };
        let key = child.next_u64();
        child.key = key;
        child.state = key;
        child
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n`. `n` must be at least 1 (0 gives 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n <= 1 {
            return 0;
        }
        // The multiply-shift maps 64 random bits onto `0..n` with a bias far below anything a fixture can notice.
        let wide = u128::from(self.next_u64()) * (n as u128);
        #[allow(clippy::cast_possible_truncation)]
        let v = (wide >> 64) as usize;
        v
    }

    /// A number in `lo..=hi`.
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi.saturating_sub(lo) + 1)
    }

    /// True with probability `percent` / 100.
    pub fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// Fisher-Yates.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(n + 8);
        while out.len() < n {
            out.extend_from_slice(&self.next_u64().to_le_bytes());
        }
        out.truncate(n);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn same_seed_same_stream() {
        let (mut a, mut b) = (Rng::new(1), Rng::new(1));
        assert!((0..100).all(|_| a.next_u64() == b.next_u64()));
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
    }

    #[test]
    fn fork_does_not_depend_on_parent_draws() {
        let mut parent = Rng::new(9);
        let before = parent.fork("x").next_u64();
        for _ in 0..10 {
            parent.next_u64();
        }
        assert_eq!(before, parent.fork("x").next_u64());
        assert_ne!(parent.fork("x").next_u64(), parent.fork("y").next_u64());
    }

    #[test]
    fn below_stays_in_range_and_shuffle_is_a_permutation() {
        let mut r = Rng::new(3);
        assert!((0..1000).all(|_| r.below(7) < 7));
        let mut v: Vec<usize> = (0..50).collect();
        r.shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
        assert_ne!(v, sorted);
    }
}
