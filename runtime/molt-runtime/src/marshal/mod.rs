//! CPython's marshal wire format, without CPython bytecode execution.
//! Protocol provenance and the explicit scalar policy: docs/design/marshal.md.

use crate::*;
use molt_runtime_core::OwnedRuntimeValue;
use num_bigint::{BigInt, Sign};
use num_traits::{ToPrimitive, Zero};

pub(crate) mod c_api;
mod read;
#[cfg(test)]
mod tests;
mod write;

type Result<T> = std::result::Result<T, ()>;
type Value<'py> = OwnedRuntimeValue<'py>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RefId(usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum Tag {
    Null = b'0',
    None = b'N',
    False = b'F',
    True = b'T',
    StopIteration = b'S',
    Ellipsis = b'.',
    Int = b'i',
    Int64 = b'I',
    Long = b'l',
    Float = b'f',
    BinaryFloat = b'g',
    Complex = b'x',
    BinaryComplex = b'y',
    Bytes = b's',
    Unicode = b'u',
    Interned = b't',
    Ascii = b'a',
    AsciiInterned = b'A',
    ShortAscii = b'z',
    ShortAsciiInterned = b'Z',
    Tuple = b'(',
    SmallTuple = b')',
    List = b'[',
    Dict = b'{',
    Set = b'<',
    FrozenSet = b'>',
    Ref = b'r',
    Slice = b':',
    Code = b'c',
}

impl Tag {
    fn decode(byte: u8) -> Option<Self> {
        Some(match byte {
            b'0' => Self::Null,
            b'N' => Self::None,
            b'F' => Self::False,
            b'T' => Self::True,
            b'S' => Self::StopIteration,
            b'.' => Self::Ellipsis,
            b'i' => Self::Int,
            b'I' => Self::Int64,
            b'l' => Self::Long,
            b'f' => Self::Float,
            b'g' => Self::BinaryFloat,
            b'x' => Self::Complex,
            b'y' => Self::BinaryComplex,
            b's' => Self::Bytes,
            b'u' => Self::Unicode,
            b't' => Self::Interned,
            b'a' => Self::Ascii,
            b'A' => Self::AsciiInterned,
            b'z' => Self::ShortAscii,
            b'Z' => Self::ShortAsciiInterned,
            b'(' => Self::Tuple,
            b')' => Self::SmallTuple,
            b'[' => Self::List,
            b'{' => Self::Dict,
            b'<' => Self::Set,
            b'>' => Self::FrozenSet,
            b'r' => Self::Ref,
            b':' => Self::Slice,
            b'c' => Self::Code,
            _ => return None,
        })
    }
}

fn fail<T>(py: &PyToken<'_>, kind: BuiltinExc, message: &str) -> Result<T> {
    raise_exception::<()>(py, kind, message);
    Err(())
}

fn bad<T>(py: &PyToken<'_>, message: &str) -> Result<T> {
    fail(py, BuiltinExc::ValueError, message)
}

fn memory_error(py: &PyToken<'_>) {
    record_memory_error_without_allocation(py);
}

fn reserve<T>(py: &PyToken<'_>, values: &mut Vec<T>, additional: usize) -> Result<()> {
    values.try_reserve(additional).map_err(|_| memory_error(py))
}

fn owned<'a>(py: &'a PyToken<'_>, bits: u64) -> Result<Value<'a>> {
    let value = unsafe { Value::from_owned_bits(py.core_token(), bits) };
    if exception_pending(py) {
        Err(())
    } else {
        Ok(value)
    }
}

fn allocated<'a>(py: &'a PyToken<'_>, ptr: *mut u8) -> Result<Value<'a>> {
    if ptr.is_null() {
        return Err(());
    }
    owned(py, MoltObject::from_ptr(ptr).bits())
}

fn retained<'a>(py: &'a PyToken<'_>, bits: u64) -> Value<'a> {
    Value::retain(py.core_token(), bits)
}

fn target_minor(py: &PyToken<'_>) -> i64 {
    crate::object::ops_sys::runtime_target_python_info(runtime_state(py)).minor
}

fn depth_limit(py: &PyToken<'_>) -> usize {
    if cfg!(target_os = "windows") {
        1000
    } else if cfg!(target_os = "wasi")
        || (target_minor(py) >= 13
            && cfg!(any(
                target_os = "ios",
                target_os = "tvos",
                target_os = "watchos",
                target_os = "visionos"
            )))
    {
        1500
    } else {
        2000
    }
}

fn call_method1<'a>(
    py: &'a PyToken<'_>,
    receiver: u64,
    name: NameId,
    arg: u64,
) -> Result<Value<'a>> {
    let name_bits = name.owned_bits(py).ok_or(())?;
    let _name = owned(py, name_bits)?;
    let method = owned(
        py,
        molt_get_attr_name_attached_body(py, receiver, name_bits),
    )?;
    owned(py, unsafe { call_callable1(py, method.bits(), arg) })
}

fn version_argument(py: &PyToken<'_>, bits: u64) -> Result<i32> {
    let value = crate::builtins::numbers::index_bigint_from_obj(
        py,
        bits,
        &format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(py, obj_from_bits(bits))
        ),
    )
    .ok_or(())?;
    value.to_i32().ok_or_else(|| {
        raise_exception::<()>(
            py,
            BuiltinExc::OverflowError,
            "Python int too large to convert to C int",
        );
    })
}

fn dumps<'a>(py: &'a PyToken<'_>, bits: u64, version: i32) -> Result<Value<'a>> {
    let bytes = write::encode(py, bits, version)?;
    allocated(py, alloc_bytes(py, &bytes))
}

fn loads<'a>(py: &'a PyToken<'_>, bits: u64) -> Result<Value<'a>> {
    let input = crate::object::buffer_exports::ScopedBuffer::simple(py, bits).map_err(|error| {
        error.raise(
            py,
            &format!(
                "a bytes-like object is required, not '{}'",
                type_name(py, obj_from_bits(bits))
            ),
        );
    })?;
    read::decode(py, read::Source::memory(input.as_slice()))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_marshal_version() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        MoltObject::from_int(if target_minor(py) >= 14 { 5 } else { 4 }).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_marshal_dumps(bits: u64, version_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        version_argument(py, version_bits)
            .and_then(|version| dumps(py, bits, version))
            .map(Value::into_bits)
            .unwrap_or_else(|()| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_marshal_loads(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        loads(py, bits)
            .map(Value::into_bits)
            .unwrap_or_else(|()| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_marshal_dump(bits: u64, file: u64, version_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let result = version_argument(py, version_bits).and_then(|version| {
            let bytes = dumps(py, bits, version)?;
            call_method1(py, file, NameId::Write, bytes.bits())
        });
        result
            .map(Value::into_bits)
            .unwrap_or_else(|()| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_marshal_load(file: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let result = (|| {
            let check = call_method1(py, file, NameId::Read, MoltObject::from_int(0).bits())?;
            let bytes_result = isinstance_bits(py, check.bits(), builtin_classes(py).bytes);
            if exception_pending(py) {
                return Err(());
            }
            if !bytes_result {
                return fail(
                    py,
                    BuiltinExc::TypeError,
                    &format!(
                        "file.read() returned not bytes but {}",
                        type_name(py, obj_from_bits(check.bits()))
                    ),
                );
            }
            read::decode(py, read::Source::file(file))
        })();
        result
            .map(Value::into_bits)
            .unwrap_or_else(|()| MoltObject::none().bits())
    })
}
