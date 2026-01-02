//! Core object representation for Molt.
//! Uses NaN-boxing to represent primitives and heap pointers in 64 bits.

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(transparent)]
pub struct MoltObject(u64);

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_NONE: u64 = 0x0003_0000_0000_0000;
const TAG_PENDING: u64 = 0x0004_0000_0000_0000; // New variant
const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

impl MoltObject {
    pub fn from_float(f: f64) -> Self {
        Self(f.to_bits())
    }

    pub fn from_int(i: i64) -> Self {
        // Simple 47-bit integer for MVP
        let val = (i as u64) & POINTER_MASK;
        Self(QNAN | TAG_INT | val)
    }

    pub fn from_bool(b: bool) -> Self {
        let val = if b { 1 } else { 0 };
        Self(QNAN | TAG_BOOL | val)
    }

    pub fn none() -> Self {
        Self(QNAN | TAG_NONE)
    }

    pub fn pending() -> Self {
        Self(QNAN | TAG_PENDING)
    }

    pub fn from_ptr(ptr: *mut u8) -> Self {
        let addr = ptr as u64;
        assert!(addr <= POINTER_MASK, "Pointer exceeds 48 bits");
        Self(addr) // Pointers have 0 in the top 16 bits (canonical)
    }

    pub fn is_float(&self) -> bool {
        (self.0 & QNAN) != QNAN
    }

    pub fn is_int(&self) -> bool {
        (self.0 & (QNAN | TAG_INT)) == (QNAN | TAG_INT)
    }

    pub fn is_pending(&self) -> bool {
        (self.0 & (QNAN | TAG_PENDING)) == (QNAN | TAG_PENDING)
    }

    pub fn as_int(&self) -> Option<i64> {
        if self.is_int() {
            let val = self.0 & POINTER_MASK;
            // Sign-extend if needed (assuming 47-bit signed)
            Some(val as i64)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_float() {
        let obj = MoltObject::from_float(std::f64::consts::PI);
        assert!(obj.is_float());
    }

    #[test]
    fn test_int() {
        let obj = MoltObject::from_int(42);
        assert!(obj.is_int());
        assert_eq!(obj.as_int(), Some(42));
    }
}