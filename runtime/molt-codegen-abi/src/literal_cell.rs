//! Literal cells (call_anatomy.md 4.9, HF-856): one writable word per str,
//! bytes or big-integer literal of a compiled object. A zero cell is filled on
//! the literal's first evaluation by `molt_literal_cell_fill`, which publishes
//! the literal's immortal object; every later evaluation loads the cell.

/// The object a literal cell holds; the fill builds it from the literal's
/// bytes with the constructor of its kind.
#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralKind {
    Str = 0,
    Bytes = 1,
    BigInt = 2,
}

impl LiteralKind {
    /// The kind a fill call's kind word names, or `None` for a word no
    /// compiled code passes.
    pub const fn from_bits(bits: u64) -> Option<Self> {
        match bits {
            0 => Some(Self::Str),
            1 => Some(Self::Bytes),
            2 => Some(Self::BigInt),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LiteralKind;

    #[test]
    fn every_kind_round_trips_its_word() {
        for kind in [LiteralKind::Str, LiteralKind::Bytes, LiteralKind::BigInt] {
            assert_eq!(LiteralKind::from_bits(kind as u64), Some(kind));
        }
        assert_eq!(LiteralKind::from_bits(3), None);
    }
}
