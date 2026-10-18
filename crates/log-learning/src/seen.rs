/// Fixed 2 MiB Bloom filter for old local training receipts. False positives
/// only withhold training; false negatives never re-count a compacted receipt.
pub struct SeenFilter {
    bits: Vec<u8>,
}
impl Default for SeenFilter {
    fn default() -> Self {
        Self {
            bits: vec![0; 2 * 1024 * 1024],
        }
    }
}
impl SeenFilter {
    pub fn from_bytes(bits: Vec<u8>) -> Result<Self, String> {
        if bits.len() != 2 * 1024 * 1024 {
            return Err("Filtro local de observações incompatível".into());
        }
        Ok(Self { bits })
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bits
    }
    fn positions(&self, key: &str) -> Option<[usize; 7]> {
        if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let a = u64::from_str_radix(&key[..16], 16).ok()?;
        let b = u64::from_str_radix(&key[16..32], 16).ok()? | 1;
        Some(std::array::from_fn(|i| {
            a.wrapping_add((i as u64).wrapping_mul(b)) as usize % (self.bits.len() * 8)
        }))
    }
    pub fn contains(&self, key: &str) -> bool {
        self.positions(key)
            .is_some_and(|p| p.iter().all(|i| self.bits[i / 8] & (1 << (i % 8)) != 0))
    }
    pub fn insert(&mut self, key: &str) -> Result<(), String> {
        let positions = self
            .positions(key)
            .ok_or("Recibo de treinamento inválido")?;
        for i in positions {
            self.bits[i / 8] |= 1 << (i % 8);
        }
        Ok(())
    }
    pub fn estimated_false_positive_rate(&self) -> f64 {
        let occupied = self.bits.iter().map(|b| b.count_ones() as u64).sum::<u64>();
        (occupied as f64 / (self.bits.len() * 8) as f64).powi(7)
    }
}
