//! Code-owned locations, encoded as CPython's documented co_linetable bytes.
//!
//! Format: CPython Objects/locations.md (3.12). This decoder needs no source
//! file, interpreter, cache or second position registry.

use crate::{TYPE_ID_BYTES, code_firstlineno, code_linetable_bits, obj_from_bits, object_type_id};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CodeLocation {
    pub(crate) line: i64,
    pub(crate) end_line: i64,
    pub(crate) column: i64,
    pub(crate) end_column: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LocationId(u64);

impl LocationId {
    pub(crate) const ABSENT: Self = Self(u64::MAX);

    pub(crate) fn from_word(word: u64) -> Self {
        Self(word)
    }

    pub(crate) const fn word(self) -> u64 {
        self.0
    }

    pub(crate) fn lasti(self) -> Option<i64> {
        if self == Self::ABSENT {
            Some(-1)
        } else {
            i64::try_from(self.0).ok()?.checked_mul(2)
        }
    }
}

pub(crate) struct LocationRows<'a> {
    bytes: &'a [u8],
    line: i64,
    repeat: u8,
    current: CodeLocation,
}

impl<'a> LocationRows<'a> {
    pub(crate) fn new(bytes: &'a [u8], first_line: i64) -> Self {
        Self {
            bytes,
            line: first_line,
            repeat: 0,
            current: CodeLocation {
                line: -1,
                end_line: -1,
                column: -1,
                end_column: -1,
            },
        }
    }

    fn byte(&mut self) -> Option<u8> {
        let (&byte, rest) = self.bytes.split_first()?;
        self.bytes = rest;
        Some(byte)
    }

    fn varint(&mut self) -> Option<i64> {
        let mut value = 0u64;
        for shift in (0..60).step_by(6) {
            let byte = self.byte()?;
            if byte & 128 != 0 {
                return None;
            }
            value |= u64::from(byte & 63) << shift;
            if byte & 64 == 0 {
                return i64::try_from(value).ok();
            }
        }
        None
    }

    fn signed(&mut self) -> Option<i64> {
        let n = self.varint()?;
        Some(if n & 1 != 0 { -(n >> 1) } else { n >> 1 })
    }

    fn row(&mut self, header: u8) -> Option<CodeLocation> {
        let kind = (header >> 3) & 15;
        if header & 128 == 0 {
            return None;
        }
        let (end_line, column, end_column) = match kind {
            0..=9 => {
                let byte = self.byte()?;
                if byte & 128 != 0 {
                    return None;
                }
                let col = i64::from(kind) * 8 + i64::from((byte >> 4) & 7);
                (self.line, col, col + i64::from(byte & 15))
            }
            10..=12 => {
                self.line = self.line.checked_add(i64::from(kind - 10))?;
                (self.line, i64::from(self.byte()?), i64::from(self.byte()?))
            }
            13 => {
                self.line = self.line.checked_add(self.signed()?)?;
                (self.line, -1, -1)
            }
            14 => {
                self.line = self.line.checked_add(self.signed()?)?;
                (
                    self.line.checked_add(self.varint()?)?,
                    self.varint()? - 1,
                    self.varint()? - 1,
                )
            }
            15 => {
                return Some(CodeLocation {
                    line: -1,
                    end_line: -1,
                    column: -1,
                    end_column: -1,
                });
            }
            _ => unreachable!(),
        };
        Some(CodeLocation {
            line: self.line,
            end_line,
            column,
            end_column,
        })
    }
}

impl Iterator for LocationRows<'_> {
    type Item = Result<CodeLocation, ()>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.repeat != 0 {
            self.repeat -= 1;
            return Some(Ok(self.current));
        }
        let header = self.byte()?;
        match self.row(header) {
            Some(row) => {
                self.repeat = header & 7;
                self.current = row;
                Some(Ok(row))
            }
            None => {
                self.bytes = &[];
                Some(Err(()))
            }
        }
    }
}

/// Borrow immutable code metadata; no Python callbacks occur while decoding it.
pub(crate) unsafe fn code_location(code: *mut u8, id: LocationId) -> Option<CodeLocation> {
    if id == LocationId::ABSENT {
        return None;
    }
    unsafe {
        let table = obj_from_bits(code_linetable_bits(code)).as_ptr()?;
        if object_type_id(table) != TYPE_ID_BYTES {
            return None;
        }
        let bytes = std::slice::from_raw_parts(crate::bytes_data(table), crate::bytes_len(table));
        LocationRows::new(bytes, code_firstlineno(code))
            .nth(usize::try_from(id.word()).ok()?)?
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpython_multiline_and_cache_unit_positions_decode_exactly() {
        // Independent CPython 3.12.15 co_linetable/co_positions capture:
        // def golden(flag):
        //     if flag:
        //         return (
        //             flag +
        //             1
        //         )
        //     return None
        let bytes = [
            128, 0, 217, 7, 11, 224, 12, 16, 216, 12, 13, 241, 3, 1, 13, 14, 240, 3, 3, 9, 10, 240,
            8, 0, 12, 16,
        ];
        let expected = [
            (1, 1, 0, 0),
            (2, 2, 7, 11),
            (2, 2, 7, 11),
            (4, 4, 12, 16),
            (5, 5, 12, 13),
            (4, 5, 12, 13),
            (4, 5, 12, 13),
            (3, 6, 8, 9),
            (7, 7, 11, 15),
        ];
        let actual = LocationRows::new(&bytes, 1)
            .map(|row| {
                let row = row.unwrap();
                (row.line, row.end_line, row.column, row.end_column)
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn malformed_locations_are_errors_instead_of_invented_source_positions() {
        for bytes in [&[0][..], &[0xf0][..], &[0xe8, 0x40][..], &[0xf0, 0x80][..]] {
            assert!(
                LocationRows::new(bytes, 1).next().unwrap().is_err(),
                "{bytes:?}"
            );
        }
        let missing = LocationRows::new(&[0xf8], 1).next().unwrap().unwrap();
        assert_eq!(
            missing,
            CodeLocation {
                line: -1,
                end_line: -1,
                column: -1,
                end_column: -1
            }
        );
    }
}
