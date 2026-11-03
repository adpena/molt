//! Runtime-known names have one generated identity and one owner per isolate.

use crate::{
    MoltObject, PyToken, TYPE_ID_STRING, alloc_string, inc_ref_bits, init_atomic_bits,
    obj_from_bits, object_type_id, runtime_state,
};

use std::sync::atomic::Ordering;

pub(crate) use super::names_generated::{NameId, RuntimeNames};

impl NameId {
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
