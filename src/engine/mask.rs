const WORDS: usize = 3;
pub const MAX_SERVICES: usize = WORDS * u64::BITS as usize;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ServiceMask([u64; WORDS]);

impl ServiceMask {
    pub fn insert(&mut self, index: usize) {
        debug_assert!(index < MAX_SERVICES);
        self.0[index / 64] |= 1_u64 << (index % 64);
    }

    pub fn contains(self, index: usize) -> bool {
        index < MAX_SERVICES && self.0[index / 64] & (1_u64 << (index % 64)) != 0
    }

    pub fn union(self, other: Self) -> Self {
        let mut result = self;
        for (target, source) in result.0.iter_mut().zip(other.0) {
            *target |= source;
        }
        result
    }

    pub fn difference(self, other: Self) -> Self {
        let mut result = self;
        for (target, source) in result.0.iter_mut().zip(other.0) {
            *target &= !source;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_mask_handles_word_boundaries_and_set_operations() {
        let mut left = ServiceMask::default();
        left.insert(0);
        left.insert(64);
        left.insert(191);
        let mut right = ServiceMask::default();
        right.insert(64);
        right.insert(130);

        let union = left.union(right);
        assert!(union.contains(0) && union.contains(64) && union.contains(130));
        assert!(union.contains(191));
        let difference = union.difference(right);
        assert!(difference.contains(0) && difference.contains(191));
        assert!(!difference.contains(64) && !difference.contains(130));
    }
}
