#[derive(Clone, Debug)]
pub(crate) struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }

    pub(crate) fn usize(&mut self, upper_exclusive: usize) -> usize {
        if upper_exclusive <= 1 {
            return 0;
        }
        (self.next_u64() % upper_exclusive as u64) as usize
    }

    pub(crate) fn chance(&mut self, probability: f32) -> bool {
        let threshold = (probability.clamp(0.0, 1.0) * u32::MAX as f32) as u32;
        self.next_u64() as u32 <= threshold
    }

    pub(crate) fn shuffle<T>(&mut self, values: &mut [T]) {
        for index in (1..values.len()).rev() {
            let target = self.usize(index + 1);
            values.swap(index, target);
        }
    }

    /// Uniformly samples a shuffled prefix without permuting the unused tail.
    /// Structural trials without library searches only observe their opening
    /// hand and bounded turn-horizon draws, so a full-library Fisher-Yates pass
    /// would spend most of its work on cards the trial can never inspect.
    pub(crate) fn shuffle_prefix<T>(&mut self, values: &mut [T], prefix_len: usize) {
        let prefix_len = prefix_len.min(values.len());
        for index in 0..prefix_len {
            let target = index + self.usize(values.len() - index);
            values.swap(index, target);
        }
    }
}
