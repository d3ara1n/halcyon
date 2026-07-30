use super::RandomGenerator;

pub struct LcGenerator {
    seed: usize,
}

impl LcGenerator {
    pub fn new(seed: usize) -> Self {
        Self { seed }
    }
}

impl RandomGenerator for LcGenerator {
    fn next(&mut self) -> usize {
        let next = (25214903917usize.wrapping_mul(self.seed).wrapping_add(11)) & ((1 << 48) - 1);
        self.seed = next;
        next
    }
}
