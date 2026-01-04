//! Core object representation for Molt.
//! Uses NaN-boxing to represent primitives and heap pointers in 64 bits.

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(transparent)]
pub struct MoltObject(u64);

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_NONE: u64 = 0x0003_0000_0000_0000;
const TAG_PTR: u64 = 0x0004_0000_0000_0000;
const TAG_PENDING: u64 = 0x0005_0000_0000_0000;
const TAG_MASK: u64 = 0x0007_0000_0000_0000;
const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const INT_SIGN_BIT: u64 = 1 << 46;
const INT_WIDTH: u64 = 47;
const INT_MASK: u64 = (1u64 << INT_WIDTH) - 1;

impl MoltObject {
    pub fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub fn bits(self) -> u64 {
        self.0
    }

    pub fn from_float(f: f64) -> Self {
        Self(f.to_bits())
    }

    pub fn from_int(i: i64) -> Self {
        // Simple 47-bit integer for MVP
        let val = (i as u64) & INT_MASK;
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
        Self(QNAN | TAG_PTR | addr)
    }

    pub fn is_float(&self) -> bool {
        (self.0 & QNAN) != QNAN
    }

    pub fn as_float(&self) -> Option<f64> {
        if self.is_float() {
            Some(f64::from_bits(self.0))
        } else {
            None
        }
    }

    pub fn is_int(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_INT)
    }

    pub fn is_bool(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_BOOL)
    }

    pub fn as_bool(&self) -> Option<bool> {
        if self.is_bool() {
            Some((self.0 & 0x1) == 1)
        } else {
            None
        }
    }

    pub fn is_none(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_NONE)
    }

    pub fn is_pending(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_PENDING)
    }

    pub fn is_ptr(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR)
    }

    pub fn as_ptr(&self) -> Option<*mut u8> {
        if self.is_ptr() {
            Some((self.0 & POINTER_MASK) as *mut u8)
        } else {
            None
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        if self.is_int() {
            let val = self.0 & INT_MASK;
            // Sign-extend if needed (assuming 47-bit signed)
            if (val & INT_SIGN_BIT) != 0 {
                Some((val as i64) - ((1u64 << INT_WIDTH) as i64))
            } else {
                Some(val as i64)
            }
        } else {
            None
        }
    }

    pub fn as_int_unchecked(&self) -> i64 {
        let val = self.0 & INT_MASK;
        if (val & INT_SIGN_BIT) != 0 {
            (val as i64) - ((1u64 << INT_WIDTH) as i64)
        } else {
            val as i64
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

    #[test]
    fn test_negative_int() {
        let obj = MoltObject::from_int(-1);
        assert!(obj.is_int());
        assert_eq!(obj.as_int(), Some(-1));
    }
}
