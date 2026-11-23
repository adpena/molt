//! Runtime-known names have one generated identity and one owner per isolate.

use crate::{
    MoltObject, PyToken, TYPE_ID_STRING, alloc_string, inc_ref_bits, init_atomic_bits,
    obj_from_bits, object_type_id, runtime_state,
};

use std::sync::atomic::Ordering;

use super::names_generated::{
    NAME_BUCKET_BITS, NAME_BUCKET_FACTOR, NAME_BUCKETS, NAME_COUNT_BITS, NAME_MAX_BYTES, NAME_ROWS,
};
pub(crate) use super::names_generated::{NameId, RuntimeNames};

impl NameId {
    /// Select a short bucket by sampling the byte window. Equality against
    /// the canonical spelling is the membership test, including collisions.
    #[inline]
    pub(crate) fn from_bytes(name: &[u8]) -> Option<Self> {
        let len = name.len();
        let sample = match name {
            [] => return None,
            [a] => u32::from(*a),
            [a, b] => u32::from(u16::from_le_bytes([*a, *b])),
            [a, b, c] => u32::from_le_bytes([*a, *b, *c, 0]),
            [a, b, c, d, ..] if len <= NAME_MAX_BYTES => {
                let middle = (len - 4) / 2;
                let center = u32::from_le_bytes(name[middle..middle + 4].try_into().unwrap());
                let suffix = u32::from_le_bytes(name[len - 4..].try_into().unwrap());
                u32::from_le_bytes([*a, *b, *c, *d])
                    ^ center.rotate_left(11)
                    ^ suffix.rotate_left(23)
            }
            _ => return None,
        };
        let hash = (sample ^ ((len as u32) << 24)).wrapping_mul(NAME_BUCKET_FACTOR);
        let bucket = (hash >> (32 - NAME_BUCKET_BITS)) as usize;
        let range = u64::from(NAME_BUCKETS[bucket]);
        let count = (range & ((1 << NAME_COUNT_BITS) - 1)) as usize;
        let start = (range >> NAME_COUNT_BITS) as usize;
        for &row in &NAME_ROWS[start..start + count] {
            let candidate = Self::ALL[usize::from(row)];
            if candidate.as_str().as_bytes() == name {
                return Some(candidate);
            }
        }
        None
    }

    /// Decode at the dynamic input boundary. Once observed, a canonical name
    /// is classified by pointer; the owning slot keeps that pointer live.
    pub(crate) fn from_bits(py: &PyToken<'_>, bits: u64) -> Option<Self> {
        let pointer = obj_from_bits(bits).as_ptr()?;
        unsafe {
            if object_type_id(pointer) != TYPE_ID_STRING {
                return None;
            }
        }
        let names = &runtime_state(py).names;
        {
            let index = names.index.lock().unwrap_or_else(|e| e.into_inner());
            if let Ok(position) = index.binary_search_by_key(&bits, |&(bits, _)| bits) {
                return Some(index[position].1);
            }
        }
        let name = unsafe {
            crate::object::ops_format::with_string_bytes(obj_from_bits(bits), Self::from_bytes)
                .flatten()
        }?;
        // Noncanonical input keeps its original identity and lifetime. Never
        // borrow an arbitrary caller's string in the reverse projection.
        if name.slot(names).load(Ordering::Acquire) == bits {
            let mut index = names.index.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(position) = index.binary_search_by_key(&bits, |&(bits, _)| bits) {
                index.insert(position, (bits, name));
            }
        }
        Some(name)
    }

    pub(crate) fn owned_bits(self, py: &PyToken<'_>) -> Option<u64> {
        let bits = self.bits(py);
        if bits == 0 {
            return None;
        }
        inc_ref_bits(py, bits);
        Some(bits)
    }

    /// Borrow the isolate's canonical name object, allocating only on first use.
    /// Allocation failure is never an absent name.
    #[inline]
    pub(crate) fn bits(self, py: &PyToken<'_>) -> u64 {
        init_atomic_bits(py, self.slot(&runtime_state(py).names), || {
            let pointer = alloc_string(py, self.as_str().as_bytes());
            if pointer.is_null() {
                0
            } else {
                MoltObject::from_ptr(pointer).bits()
            }
        })
    }
}

impl From<molt_obj_model::sequence_compare::RichCompareOp> for NameId {
    fn from(op: molt_obj_model::sequence_compare::RichCompareOp) -> Self {
        use molt_obj_model::sequence_compare::RichCompareOp;
        match op {
            RichCompareOp::Lt => NameId::DunderLt,
            RichCompareOp::Le => NameId::DunderLe,
            RichCompareOp::Eq => NameId::DunderEq,
            RichCompareOp::Ne => NameId::DunderNe,
            RichCompareOp::Gt => NameId::DunderGt,
            RichCompareOp::Ge => NameId::DunderGe,
        }
    }
}

impl From<molt_cpython_abi::api::typeobj::TypeAttributeField> for NameId {
    fn from(field: molt_cpython_abi::api::typeobj::TypeAttributeField) -> Self {
        use molt_cpython_abi::api::typeobj::TypeAttributeField as Field;
        match field {
            Field::Class => crate::NameId::DunderClass,
            Field::Name => crate::NameId::DunderName,
            Field::QualName => crate::NameId::DunderQualname,
            Field::Base => crate::NameId::DunderBase,
            Field::Bases => crate::NameId::DunderBases,
            Field::Mro => crate::NameId::DunderMro,
            Field::Dictionary => crate::NameId::DunderDict,
            Field::Annotations => crate::NameId::DunderAnnotations,
            Field::Annotate => crate::NameId::DunderAnnotate,
            Field::Doc => crate::NameId::DunderDoc,
            Field::TextSignature => crate::NameId::DunderTextSignature,
            Field::AbstractMethods => crate::NameId::DunderAbstractmethods,
            Field::Module => crate::NameId::DunderModule,
        }
    }
}

impl From<molt_obj_model::ExceptionTypedField> for NameId {
    fn from(field: molt_obj_model::ExceptionTypedField) -> Self {
        use molt_obj_model::ExceptionTypedField as Field;
        match field {
            Field::AttributeErrorName => crate::NameId::Name,
            Field::AttributeErrorObject => crate::NameId::Obj,
            Field::GroupExceptions => crate::NameId::Exceptions,
            Field::GroupMessage => crate::NameId::Message,
            Field::ImportMessage => crate::NameId::Msg,
            Field::ImportName => crate::NameId::Name,
            Field::ImportNameFrom => crate::NameId::NameFrom,
            Field::ImportPath => crate::NameId::Path,
            Field::NameErrorName => crate::NameId::Name,
            Field::OSErrorCharactersWritten => crate::NameId::CharactersWritten,
            Field::OSErrorErrno => crate::NameId::Errno,
            Field::OSErrorFilename => crate::NameId::Filename,
            Field::OSErrorFilename2 => crate::NameId::Filename2,
            Field::OSErrorStrError => crate::NameId::Strerror,
            Field::OSErrorWinError => crate::NameId::Winerror,
            Field::StopIterationValue => crate::NameId::Value,
            Field::SyntaxEndLineNumber => crate::NameId::EndLineno,
            Field::SyntaxEndOffset => crate::NameId::EndOffset,
            Field::SyntaxFilename => crate::NameId::Filename,
            Field::SyntaxLineNumber => crate::NameId::Lineno,
            Field::SyntaxMessage => crate::NameId::Msg,
            Field::SyntaxOffset => crate::NameId::Offset,
            Field::SyntaxPrintFileAndLine => crate::NameId::PrintFileAndLine,
            Field::SyntaxText => crate::NameId::Text,
            Field::SystemExitCode => crate::NameId::Code,
            Field::UnicodeEncoding => crate::NameId::Encoding,
            Field::UnicodeEnd => crate::NameId::End,
            Field::UnicodeObject => crate::NameId::Object,
            Field::UnicodeReason => crate::NameId::Reason,
            Field::UnicodeStart => crate::NameId::Start,
        }
    }
}

impl From<molt_obj_model::ExceptionAttributeField> for NameId {
    fn from(field: molt_obj_model::ExceptionAttributeField) -> Self {
        use molt_obj_model::ExceptionAttributeField as Field;
        match field {
            Field::Dictionary => crate::NameId::DunderDict,
            Field::Args => crate::NameId::Args,
            Field::Traceback => crate::NameId::DunderTraceback,
            Field::Context => crate::NameId::DunderContext,
            Field::Cause => crate::NameId::DunderCause,
            Field::SuppressContext => crate::NameId::DunderSuppressContext,
            Field::Typed(field) => field.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NameId;

    #[test]
    fn lookup_validates_every_name_and_rejects_nearby_byte_sequences() {
        assert_eq!(NameId::from_bytes(b""), None);
        for name in NameId::ALL {
            let spelling = name.as_str().as_bytes();
            assert_eq!(NameId::from_bytes(spelling), Some(name));
            let mut miss = spelling.to_vec();
            miss[0] = 0xff;
            assert_eq!(NameId::from_bytes(&miss), None);
        }
        // Length and all three sample windows match a real name, so the
        // candidate check must reject this miss using the unsampled bytes.
        assert_eq!(
            NameId::from_bytes(b"__getattribute__"),
            Some(NameId::DunderGetattribute)
        );
        assert_eq!(NameId::from_bytes(b"__geZattribute__"), None);
    }
}
