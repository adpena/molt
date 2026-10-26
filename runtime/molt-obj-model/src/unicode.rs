//! Pinned target-Python Unicode facts shared by runtime and C ABI.
//!
//! Select one borrowed descriptor at the operation boundary. Lookup uses two
//! integer indexes and one shared record, with no allocation, lock or host query.
//! ASCII SIMD kernels remain with their existing callers.

use crate::text_storage::{code_points, push_code_point};

mod generated {
    include!("unicode_generated.rs");
}

#[derive(Clone, Copy)]
pub struct UnicodeData {
    pub version: &'static str,
    index: &'static [u16],
}

#[derive(Clone, Copy)]
struct Record {
    flags: u16,
    case: [u16; 4],
    numeric: u16,
    decimal: i8,
    digit: i8,
    category: u8,
    bidi: u8,
    combining: u8,
    mirrored: u8,
    width: u8,
}

#[derive(Clone, Copy)]
struct CaseSequence {
    deltas: [i32; 3],
    len: u8,
}

#[derive(Clone, Copy)]
pub enum CaseMapping {
    Lower,
    Upper,
    Title,
    Fold,
}

#[derive(Clone, Copy)]
pub enum CaseOperation {
    Lower,
    Upper,
    Title,
    Fold,
    Swapcase,
    Capitalize,
}

impl UnicodeData {
    /// Unknown target versions have no admitted Unicode facts.
    pub fn for_version(target: (i64, i64)) -> Option<&'static Self> {
        generated::for_version(target)
    }

    pub fn for_target_comparison(
        mut at_least: impl FnMut(i64, i64) -> bool,
    ) -> Option<&'static Self> {
        for version in crate::python_version_generated::SUPPORTED_TARGET_PYTHON_VERSIONS
            .iter()
            .rev()
        {
            let (major, minor) = version.split_once('.')?;
            let (major, minor) = (major.parse::<i64>().ok()?, minor.parse::<i64>().ok()?);
            if at_least(major, minor) {
                if at_least(major, minor.checked_add(1)?) {
                    return None;
                }
                return Self::for_version((major, minor));
            }
        }
        None
    }

    /// Generation proves every admitted table has identical ASCII records.
    /// Runtime byte operations may use these facts without querying the target
    /// when their entire input is ASCII. C-API target admission remains separate.
    #[inline]
    pub fn ascii() -> &'static Self {
        Self::for_version({
            let version = crate::python_version_generated::DEFAULT_TARGET_PYTHON_VERSION;
            (version.0, version.1)
        })
        .expect("generated default target has Unicode facts")
    }

    pub fn is_identifier(&self, bytes: &[u8]) -> bool {
        let mut codes = code_points(bytes);
        codes
            .next()
            .is_some_and(|code| self.is_identifier_start(code))
            && codes.all(|code| self.is_identifier_continue(code))
    }

    #[inline]
    fn record(&self, code: u32) -> &'static Record {
        assert!(code < 0x110000, "Unicode code point exceeds U+10FFFF");
        let page = usize::from(self.index[(code >> generated::SHIFT) as usize]);
        let offset = (page << generated::SHIFT) + (code as usize & ((1 << generated::SHIFT) - 1));
        &generated::RECORDS[usize::from(generated::PAGES[offset])]
    }

    #[inline]
    pub fn is_alpha(&self, code: u32) -> bool {
        self.record(code).flags & 1 != 0
    }
    #[inline]
    pub fn is_decimal(&self, code: u32) -> bool {
        self.record(code).flags & 2 != 0
    }
    #[inline]
    pub fn is_digit(&self, code: u32) -> bool {
        self.record(code).flags & 4 != 0
    }
    #[inline]
    pub fn is_lower(&self, code: u32) -> bool {
        self.record(code).flags & 8 != 0
    }
    #[inline]
    pub fn is_linebreak(&self, code: u32) -> bool {
        self.record(code).flags & 16 != 0
    }
    #[inline]
    pub fn is_space(&self, code: u32) -> bool {
        self.record(code).flags & 32 != 0
    }
    #[inline]
    pub fn is_title(&self, code: u32) -> bool {
        self.record(code).flags & 64 != 0
    }
    #[inline]
    pub fn is_upper(&self, code: u32) -> bool {
        self.record(code).flags & 128 != 0
    }
    #[inline]
    pub fn is_identifier_start(&self, code: u32) -> bool {
        code == 95 || self.record(code).flags & 256 != 0
    }
    #[inline]
    pub fn is_identifier_continue(&self, code: u32) -> bool {
        self.record(code).flags & 512 != 0
    }
    #[inline]
    pub fn is_printable(&self, code: u32) -> bool {
        self.record(code).flags & 1024 != 0
    }
    #[inline]
    pub fn is_numeric(&self, code: u32) -> bool {
        self.record(code).flags & 2048 != 0
    }
    #[inline]
    pub fn is_case_ignorable(&self, code: u32) -> bool {
        self.record(code).flags & 4096 != 0
    }
    #[inline]
    pub fn is_cased(&self, code: u32) -> bool {
        self.record(code).flags & 8192 != 0
    }
    #[inline]
    pub fn decimal(&self, code: u32) -> Option<u8> {
        u8::try_from(self.record(code).decimal).ok()
    }
    #[inline]
    pub fn digit(&self, code: u32) -> Option<u8> {
        u8::try_from(self.record(code).digit).ok()
    }
    #[inline]
    pub fn numeric(&self, code: u32) -> Option<f64> {
        let row = self.record(code);
        (row.flags & 2048 != 0).then(|| generated::NUMERIC[usize::from(row.numeric)])
    }
    #[inline]
    pub fn category(&self, code: u32) -> &'static str {
        generated::CATEGORIES[usize::from(self.record(code).category)]
    }
    #[inline]
    pub fn bidirectional(&self, code: u32) -> &'static str {
        generated::BIDIRECTIONAL[usize::from(self.record(code).bidi)]
    }
    #[inline]
    pub fn combining(&self, code: u32) -> u8 {
        self.record(code).combining
    }
    #[inline]
    pub fn mirrored(&self, code: u32) -> bool {
        self.record(code).mirrored != 0
    }
    #[inline]
    pub fn east_asian_width(&self, code: u32) -> &'static str {
        generated::WIDTHS[usize::from(self.record(code).width)]
    }

    #[inline]
    pub fn mapping(&self, code: u32, mapping: CaseMapping) -> impl Iterator<Item = u32> {
        let index = match mapping {
            CaseMapping::Lower => 0,
            CaseMapping::Upper => 1,
            CaseMapping::Title => 2,
            CaseMapping::Fold => 3,
        };
        let sequence = &generated::CASE[usize::from(self.record(code).case[index])];
        sequence.deltas[..usize::from(sequence.len)]
            .iter()
            .map(move |delta| (i64::from(code) + i64::from(*delta)) as u32)
    }

    /// Full Python case conversion over admitted surrogatepass storage.
    /// Sigma's look-ahead stops at the next non-ignorable character; these
    /// spans cannot overlap across sigmas, so total work remains linear.
    pub fn transform(&self, bytes: &[u8], operation: CaseOperation) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        let mut codes = code_points(bytes);
        let mut previous_cased = false;
        let mut previous_word_cased = false;
        let mut first = true;
        while let Some(code) = codes.next() {
            let mapping = match operation {
                CaseOperation::Lower => Some(CaseMapping::Lower),
                CaseOperation::Upper => Some(CaseMapping::Upper),
                CaseOperation::Fold => Some(CaseMapping::Fold),
                CaseOperation::Capitalize => Some(if first {
                    CaseMapping::Title
                } else {
                    CaseMapping::Lower
                }),
                CaseOperation::Title => Some(if previous_word_cased {
                    CaseMapping::Lower
                } else {
                    CaseMapping::Title
                }),
                CaseOperation::Swapcase if self.is_upper(code) => Some(CaseMapping::Lower),
                CaseOperation::Swapcase if self.is_lower(code) => Some(CaseMapping::Upper),
                CaseOperation::Swapcase => None,
            };
            if matches!(mapping, Some(CaseMapping::Lower))
                && code == 0x03a3
                && previous_cased
                && !codes
                    .clone()
                    .find(|next| !self.is_case_ignorable(*next))
                    .is_some_and(|next| self.is_cased(next))
            {
                push_code_point(&mut out, 0x03c2);
            } else if let Some(mapping) = mapping {
                for mapped in self.mapping(code, mapping) {
                    push_code_point(&mut out, mapped);
                }
            } else {
                push_code_point(&mut out, code);
            }
            if !self.is_case_ignorable(code) {
                previous_cased = self.is_cased(code);
            }
            previous_word_cased = self.is_cased(code);
            first = false;
        }
        out
    }
}

/// Numeric syntax admits ASCII TAB..CR/SPACE, excluding string separators
/// U+001C..1F. Bytes never gain Unicode whitespace from UTF-8 decoding.
pub fn numeric_whitespace(data: Option<&UnicodeData>, ch: char) -> bool {
    if ch.is_ascii() {
        matches!(ch, '\t'..='\r' | ' ')
    } else {
        data.is_some_and(|data| data.is_space(ch as u32))
    }
}

#[cfg(test)]
mod tests;
