//! Exception rendering semantics shared by managed runtimes and physical C objects.
//!
//! Adapters own representation checks, Python calls and reference/error custody.
//! This module owns selective observation, callback order and formatting. It uses
//! static dispatch and fixed-size snapshots; no C type publication is required.

use crate::{
    ExceptionLayoutKind, ExceptionStrSlot, ExceptionTypedField, MAX_EXCEPTION_TYPED_FIELDS,
};
use std::mem::ManuallyDrop;

/// An adapter-owned reference with bulk cleanup under one error-preservation
/// boundary. Release every value in iterator order, retaining ordinary Drop for
/// individual owners and any values left behind by an unwinding cleanup.
pub trait ExceptionRenderOwned: Sized {
    fn release_all(values: impl Iterator<Item = Self>);
}

/// Fields are owned only when requested. Missing physical fields remain distinct
/// from Python None. Field order preserves cleanup of args, typed fields, class.
pub struct ExceptionRenderSnapshot<V: ExceptionRenderOwned> {
    pub args: Option<V>,
    pub fields: [Option<V>; MAX_EXCEPTION_TYPED_FIELDS],
    pub class: ManuallyDrop<V>,
    pub layout: ExceptionLayoutKind,
    pub start: isize,
    pub end: isize,
    pub unicode_object_present: bool,
}

impl<V: ExceptionRenderOwned> ExceptionRenderSnapshot<V> {
    fn field(&self, field: ExceptionTypedField) -> Option<&V> {
        self.layout
            .field_policies()
            .iter()
            .position(|policy| policy.field == field)
            .and_then(|index| self.fields[index].as_ref())
    }
}

impl<V: ExceptionRenderOwned> Drop for ExceptionRenderSnapshot<V> {
    fn drop(&mut self) {
        // The mandatory class owner moves exactly once into the cleanup
        // iterator. Its ordinary Drop remains available if cleanup unwinds.
        let class = unsafe { ManuallyDrop::take(&mut self.class) };
        V::release_all(
            self.args
                .take()
                .into_iter()
                .chain(self.fields.iter_mut().filter_map(Option::take))
                .chain(std::iter::once(class)),
        );
    }
}

/// The representation boundary, not a second rendering policy. Owned references
/// must preserve both raised-error channels when released, including after a
/// callback fails. Borrowed values remain valid through their snapshot owner.
pub trait ExceptionRenderContext {
    type Object: Copy;
    type Owned: ExceptionRenderOwned;

    fn borrow(value: &Self::Owned) -> Self::Object;
    fn capture(
        &self,
        object: Self::Object,
        required: Option<ExceptionLayoutKind>,
        capture_args: bool,
        fields: &[ExceptionTypedField],
    ) -> Option<ExceptionRenderSnapshot<Self::Owned>>;
    fn none(&self) -> Self::Object;
    fn retain(&self, value: Self::Object) -> Self::Owned;
    fn string_from_bytes(&self, bytes: &[u8]) -> Option<Self::Owned>;
    /// Missing physical values follow PyObject_Str/Repr(NULL): the text <NULL>,
    /// distinct from the Python None singleton.
    fn render(&self, value: Option<Self::Object>, repr: bool) -> Option<Self::Owned>;
    fn string_bytes(&self, value: Self::Object) -> Option<Vec<u8>>;
    fn is_string(&self, value: Self::Object, exact: bool) -> bool;
    fn type_short_name(&self, class: Self::Object) -> Option<Vec<u8>>;
    fn tuple_len(&self, value: Option<Self::Object>) -> Option<usize>;
    fn tuple_item(&self, value: Self::Object, index: usize) -> Option<Self::Object>;
    /// CPython's exact-int-only SyntaxError line read uses C long and renders -1
    /// on overflow without setting an error. Subclasses do not enter this path.
    fn syntax_line(&self, value: Self::Object) -> Option<i64>;
    /// Validate the input representation even when index is absent. Return a
    /// byte (decode) or Python code point, including a lone surrogate.
    fn unicode_unit(
        &self,
        value: Option<Self::Object>,
        index: Option<usize>,
        decode: bool,
    ) -> Option<Option<u32>>;
}

fn field<C: ExceptionRenderContext>(
    snapshot: &ExceptionRenderSnapshot<C::Owned>,
    field: ExceptionTypedField,
) -> Option<C::Object> {
    snapshot.field(field).map(C::borrow)
}

fn rendered_bytes<C: ExceptionRenderContext>(
    context: &C,
    value: C::Object,
    repr: bool,
) -> Option<Vec<u8>> {
    let result = context.render(Some(value), repr)?;
    context.string_bytes(C::borrow(&result))
}

fn argc<C: ExceptionRenderContext>(
    context: &C,
    snapshot: &ExceptionRenderSnapshot<C::Owned>,
) -> Option<usize> {
    snapshot
        .args
        .as_ref()
        .map_or(Some(0), |args| context.tuple_len(Some(C::borrow(args))))
}

fn base_str<C: ExceptionRenderContext>(
    context: &C,
    snapshot: &ExceptionRenderSnapshot<C::Owned>,
) -> Option<C::Owned> {
    match argc(context, snapshot)? {
        0 => context.string_from_bytes(b""),
        1 => context.render(
            Some(context.tuple_item(C::borrow(snapshot.args.as_ref()?), 0)?),
            false,
        ),
        _ => context.render(snapshot.args.as_ref().map(C::borrow), false),
    }
}

pub fn render_exception_repr<C: ExceptionRenderContext>(
    context: &C,
    object: C::Object,
) -> Option<C::Owned> {
    let snapshot = context.capture(object, None, true, &[])?;
    let mut out = context.type_short_name(C::borrow(&snapshot.class))?;
    let count = argc(context, &snapshot)?;
    if count == 0 {
        out.extend_from_slice(b"()");
    } else {
        let args = C::borrow(snapshot.args.as_ref()?);
        let value = if count == 1 {
            out.push(b'(');
            context.tuple_item(args, 0)?
        } else {
            args
        };
        out.extend_from_slice(&rendered_bytes(context, value, true)?);
        if count == 1 {
            out.push(b')');
        }
    }
    context.string_from_bytes(&out)
}

pub fn render_exception_str<C: ExceptionRenderContext>(
    context: &C,
    object: C::Object,
    slot: ExceptionStrSlot,
) -> Option<C::Owned> {
    match slot {
        ExceptionStrSlot::Base | ExceptionStrSlot::KeyError => {
            let snapshot = context.capture(object, None, true, &[])?;
            if slot == ExceptionStrSlot::KeyError && argc(context, &snapshot)? == 1 {
                context.render(
                    Some(context.tuple_item(C::borrow(snapshot.args.as_ref()?), 0)?),
                    true,
                )
            } else {
                base_str(context, &snapshot)
            }
        }
        ExceptionStrSlot::Group => group_str(context, object),
        ExceptionStrSlot::Import => import_str(context, object),
        ExceptionStrSlot::Syntax => syntax_str(context, object),
        ExceptionStrSlot::OSError => os_str(context, object),
        ExceptionStrSlot::UnicodeDecode
        | ExceptionStrSlot::UnicodeEncode
        | ExceptionStrSlot::UnicodeTranslate => unicode_str(context, object, slot),
    }
}

fn group_str<C: ExceptionRenderContext>(context: &C, object: C::Object) -> Option<C::Owned> {
    use ExceptionTypedField::{GroupExceptions, GroupMessage};
    let snapshot = context.capture(
        object,
        Some(ExceptionLayoutKind::Group),
        false,
        &[GroupMessage, GroupExceptions],
    )?;
    let count = context.tuple_len(field::<C>(&snapshot, GroupExceptions))?;
    let mut out = rendered_bytes(
        context,
        field::<C>(&snapshot, GroupMessage).unwrap_or_else(|| context.none()),
        false,
    )?;
    out.extend_from_slice(
        format!(
            " ({count} sub-exception{})",
            if count > 1 { "s" } else { "" }
        )
        .as_bytes(),
    );
    context.string_from_bytes(&out)
}

fn import_str<C: ExceptionRenderContext>(context: &C, object: C::Object) -> Option<C::Owned> {
    use ExceptionTypedField::ImportMessage;
    let snapshot = context.capture(
        object,
        Some(ExceptionLayoutKind::Import),
        true,
        &[ImportMessage],
    )?;
    if let Some(message) = field::<C>(&snapshot, ImportMessage)
        && context.is_string(message, true)
    {
        Some(context.retain(message))
    } else {
        base_str(context, &snapshot)
    }
}

fn syntax_str<C: ExceptionRenderContext>(context: &C, object: C::Object) -> Option<C::Owned> {
    use ExceptionTypedField::{SyntaxFilename, SyntaxLineNumber, SyntaxMessage};
    let snapshot = context.capture(
        object,
        Some(ExceptionLayoutKind::Syntax),
        false,
        &[SyntaxMessage, SyntaxFilename, SyntaxLineNumber],
    )?;
    let filename = if let Some(filename) = field::<C>(&snapshot, SyntaxFilename)
        && context.is_string(filename, false)
    {
        let bytes = context.string_bytes(filename)?;
        let separator = if cfg!(windows) { b'\\' } else { b'/' };
        let offset = bytes
            .iter()
            .rposition(|byte| *byte == separator)
            .map_or(0, |index| index + 1);
        Some(bytes[offset..].to_vec())
    } else {
        None
    };
    let line = field::<C>(&snapshot, SyntaxLineNumber).and_then(|line| context.syntax_line(line));
    let message = field::<C>(&snapshot, SyntaxMessage).unwrap_or_else(|| context.none());
    if filename.is_none() && line.is_none() {
        return context.render(Some(message), false);
    }
    let mut out = rendered_bytes(context, message, false)?;
    out.extend_from_slice(b" (");
    if let Some(filename) = &filename {
        out.extend_from_slice(filename);
        if line.is_some() {
            out.extend_from_slice(b", ");
        }
    }
    if let Some(line) = line {
        out.extend_from_slice(format!("line {line}").as_bytes());
    }
    out.push(b')');
    context.string_from_bytes(&out)
}

fn os_str<C: ExceptionRenderContext>(context: &C, object: C::Object) -> Option<C::Owned> {
    use ExceptionTypedField::{
        OSErrorErrno, OSErrorFilename, OSErrorFilename2, OSErrorStrError, OSErrorWinError,
    };
    let snapshot = context.capture(
        object,
        Some(ExceptionLayoutKind::OSError),
        true,
        &[
            OSErrorErrno,
            OSErrorStrError,
            OSErrorFilename,
            OSErrorFilename2,
            OSErrorWinError,
        ],
    )?;
    let filename = field::<C>(&snapshot, OSErrorFilename);
    let filename2 = field::<C>(&snapshot, OSErrorFilename2);
    let reason = field::<C>(&snapshot, OSErrorStrError);
    let errno = field::<C>(&snapshot, OSErrorErrno);
    #[cfg(windows)]
    let winerror = field::<C>(&snapshot, OSErrorWinError);
    #[cfg(windows)]
    let (label, code) = if winerror.is_some() && (filename.is_some() || reason.is_some()) {
        ("WinError", winerror)
    } else {
        ("Errno", errno)
    };
    #[cfg(not(windows))]
    let (label, code) = ("Errno", errno);
    if filename.is_none() && (code.is_none() || reason.is_none()) {
        return base_str(context, &snapshot);
    }
    let code = rendered_bytes(context, code.unwrap_or_else(|| context.none()), false)?;
    let reason = rendered_bytes(context, reason.unwrap_or_else(|| context.none()), false)?;
    let mut out = format!("[{label} ").into_bytes();
    out.extend_from_slice(&code);
    out.extend_from_slice(b"] ");
    out.extend_from_slice(&reason);
    if let Some(filename) = filename {
        out.extend_from_slice(b": ");
        out.extend_from_slice(&rendered_bytes(context, filename, true)?);
        if let Some(filename2) = filename2 {
            out.extend_from_slice(b" -> ");
            out.extend_from_slice(&rendered_bytes(context, filename2, true)?);
        }
    }
    context.string_from_bytes(&out)
}

/// Keep both callback results alive through final formatting and release in
/// evaluation order. The fixed pair does not allocate a reference-holder vector.
struct UnicodeResults<V: ExceptionRenderOwned> {
    reason: Option<V>,
    encoding: Option<V>,
}

impl<V: ExceptionRenderOwned> Drop for UnicodeResults<V> {
    fn drop(&mut self) {
        V::release_all(self.reason.take().into_iter().chain(self.encoding.take()));
    }
}

fn unicode_str<C: ExceptionRenderContext>(
    context: &C,
    object: C::Object,
    slot: ExceptionStrSlot,
) -> Option<C::Owned> {
    use ExceptionTypedField::{UnicodeEncoding, UnicodeObject, UnicodeReason};
    let mut strings = UnicodeResults {
        reason: None,
        encoding: None,
    };
    let reason = {
        let snapshot = context.capture(
            object,
            Some(ExceptionLayoutKind::Unicode),
            false,
            &[UnicodeReason],
        )?;
        if !snapshot.unicode_object_present {
            return context.string_from_bytes(b"");
        }
        strings.reason = Some(context.render(field::<C>(&snapshot, UnicodeReason), false)?);
        context.string_bytes(C::borrow(strings.reason.as_ref()?))?
    };
    let mut out = Vec::new();
    if slot != ExceptionStrSlot::UnicodeTranslate {
        let snapshot = context.capture(
            object,
            Some(ExceptionLayoutKind::Unicode),
            false,
            &[UnicodeEncoding],
        )?;
        strings.encoding = Some(context.render(field::<C>(&snapshot, UnicodeEncoding), false)?);
        let encoding = context.string_bytes(C::borrow(strings.encoding.as_ref()?))?;
        out.push(b'\'');
        out.extend_from_slice(&encoding);
        out.extend_from_slice(b"' codec ");
    }
    // Both callbacks precede input/position capture. Rebinding from either one
    // must affect these observations without retiring earlier callback results.
    let snapshot = context.capture(
        object,
        Some(ExceptionLayoutKind::Unicode),
        false,
        &[UnicodeObject],
    )?;
    let start = snapshot.start;
    let end = snapshot.end;
    let single = end.checked_sub(start) == Some(1);
    let index = if single {
        usize::try_from(start).ok()
    } else {
        None
    };
    let unit = context.unicode_unit(
        field::<C>(&snapshot, UnicodeObject),
        index,
        slot == ExceptionStrSlot::UnicodeDecode,
    )?;
    let operation = match slot {
        ExceptionStrSlot::UnicodeDecode => "decode",
        ExceptionStrSlot::UnicodeEncode => "encode",
        ExceptionStrSlot::UnicodeTranslate => "translate",
        _ => unreachable!("Unicode slot identity"),
    };
    let detail = match unit {
        Some(byte) if slot == ExceptionStrSlot::UnicodeDecode => {
            format!("can't decode byte 0x{byte:02x} in position {start}: ")
        }
        Some(code) => {
            let escaped = if code <= 0xff {
                format!("\\x{code:02x}")
            } else if code <= 0xffff {
                format!("\\u{code:04x}")
            } else {
                format!("\\U{code:08x}")
            };
            format!("can't {operation} character '{escaped}' in position {start}: ")
        }
        None => {
            let units = if slot == ExceptionStrSlot::UnicodeDecode {
                "bytes"
            } else {
                "characters"
            };
            format!(
                "can't {operation} {units} in position {start}-{}: ",
                end.wrapping_sub(1)
            )
        }
    };
    out.extend_from_slice(detail.as_bytes());
    out.extend_from_slice(&reason);
    context.string_from_bytes(&out)
}
