use super::*;
use std::collections::HashSet;

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;

pub(super) enum Source<'input> {
    Memory {
        input: &'input [u8],
        position: usize,
    },
    File {
        file: u64,
        buffer: Vec<u8>,
    },
}

impl<'input> Source<'input> {
    pub(super) fn memory(input: &'input [u8]) -> Self {
        Self::Memory { input, position: 0 }
    }
    pub(super) fn file(file: u64) -> Self {
        Self::File {
            file,
            buffer: Vec::new(),
        }
    }

    fn take(&mut self, py: &PyToken<'_>, count: usize) -> Result<&[u8]> {
        match self {
            Self::Memory { input, position } => {
                let end = position
                    .checked_add(count)
                    .filter(|&end| end <= input.len());
                let Some(end) = end else {
                    return fail(py, BuiltinExc::EOFError, "marshal data too short");
                };
                let bytes = &input[*position..end];
                *position = end;
                Ok(bytes)
            }
            Self::File { file, buffer } => {
                if count > buffer.len() {
                    reserve(py, buffer, count - buffer.len())?;
                    buffer.resize(count, 0);
                }
                let address = buffer.as_mut_ptr();
                let owner = ();
                let view = unsafe {
                    crate::object::buffer_exports::CallbackMemoryView::borrow(
                        py,
                        &owner,
                        address,
                        count,
                        crate::object::buffer_exports::CallbackMemoryViewAccess::Writable,
                    )
                }
                .map_err(|_| ())?;
                let result = call_method1(py, *file, NameId::Readinto, view.bits())?;
                let number = crate::builtins::numbers::index_bigint_from_obj(
                    py,
                    result.bits(),
                    &format!(
                        "'{}' object cannot be interpreted as an integer",
                        type_name(py, obj_from_bits(result.bits()))
                    ),
                )
                .ok_or(())?;
                let Some(read) = number.to_isize() else {
                    return bad(py, "cannot fit 'int' into an index-sized integer");
                };
                if read != count as isize {
                    if read > count as isize {
                        return bad(
                            py,
                            &format!(
                                "read() returned too much data: {count} bytes requested, {read} returned"
                            ),
                        );
                    }
                    return fail(py, BuiltinExc::EOFError, "EOF read where not expected");
                }
                drop(view);
                Ok(&buffer[..count])
            }
        }
    }

    fn byte(&mut self, py: &PyToken<'_>, object: bool) -> Result<u8> {
        if let Self::Memory { input, position } = self {
            if *position == input.len() {
                return fail(
                    py,
                    BuiltinExc::EOFError,
                    if object {
                        "EOF read where object expected"
                    } else {
                        "EOF read where not expected"
                    },
                );
            }
        }
        match self.take(py, 1) {
            Ok(bytes) => Ok(bytes[0]),
            Err(()) => {
                if object {
                    let exception = molt_exception_last_pending();
                    let matches = crate::builtins::exceptions::exception_matches_kind(
                        py,
                        exception,
                        BuiltinExc::EOFError,
                    );
                    dec_ref_bits(py, exception);
                    if matches {
                        clear_exception(py);
                        return fail(py, BuiltinExc::EOFError, "EOF read where object expected");
                    }
                }
                Err(())
            }
        }
    }

    fn i32(&mut self, py: &PyToken<'_>) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(py, 4)?.try_into().unwrap()))
    }

    fn length(&mut self, py: &PyToken<'_>, kind: &str) -> Result<usize> {
        let count = self.i32(py)?;
        if count < 0 {
            return bad(py, &format!("bad marshal data ({kind} size out of range)"));
        }
        Ok(count as usize)
    }

    fn float(&mut self, py: &PyToken<'_>, binary: bool) -> Result<f64> {
        if binary {
            return Ok(f64::from_le_bytes(self.take(py, 8)?.try_into().unwrap()));
        }
        let size = self.byte(py, false)? as usize;
        let bytes = self.take(py, size)?;
        // marshal.c copies the length-prefixed bytes into a NUL-terminated C
        // buffer before PyOS_string_to_double. Bytes following an interior NUL
        // are consumed from the stream but do not participate in parsing.
        let bytes = &bytes[..bytes
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(bytes.len())];
        // Marshal's decimal records use PyOS_string_to_double, not float()'s
        // whitespace/underscore acceptance. Rust's complete decimal parser has
        // the same grammar for the finite spellings the wire writer produces.
        let text = std::str::from_utf8(bytes).ok();
        match text.and_then(|text| text.parse::<f64>().ok()) {
            Some(value)
                if !text
                    .unwrap()
                    .bytes()
                    .any(|b| b.is_ascii_whitespace() || b == b'_') =>
            {
                Ok(value)
            }
            _ => {
                let text = String::from_utf8_lossy(bytes);
                let value = allocated(py, alloc_string(py, text.as_bytes()))?;
                let rendered = format_obj(py, obj_from_bits(value.bits()));
                fail(
                    py,
                    BuiltinExc::ValueError,
                    &format!("could not convert string to float: {rendered}"),
                )
            }
        }
    }
}

enum Reference<'py> {
    Reserved,
    Ready(Value<'py>),
}
enum Record<'py> {
    Null,
    Value(Value<'py>),
}

enum KeyVisit {
    Enter(u64),
    Leave(u64),
}

enum FrameKind<'py> {
    Tuple {
        index: usize,
        length: usize,
    },
    List {
        remaining: usize,
    },
    Dict {
        key: Option<Value<'py>>,
    },
    Set {
        remaining: usize,
    },
    FrozenSet {
        remaining: usize,
        children: Vec<Value<'py>>,
    },
    Slice {
        children: Vec<Value<'py>>,
    },
}

struct Frame<'py> {
    kind: FrameKind<'py>,
    owner: Option<Value<'py>>,
    reference: Option<RefId>,
}

struct Reader<'a, 'py, 'input> {
    py: &'a PyToken<'py>,
    source: Source<'input>,
    references: Vec<Reference<'a>>,
    frames: Vec<Frame<'a>>,
    constructing_tuples: HashSet<u64>,
}

impl<'a, 'py, 'input> Reader<'a, 'py, 'input> {
    fn register(&mut self, flag: bool, value: Option<u64>) -> Result<Option<RefId>> {
        if !flag {
            return Ok(None);
        }
        if self.references.len() >= 0x7fff_fffe {
            return bad(self.py, "bad marshal data (index list too large)");
        }
        reserve(self.py, &mut self.references, 1)?;
        let id = RefId(self.references.len());
        self.references.push(match value {
            Some(bits) => Reference::Ready(retained(self.py, bits)),
            None => Reference::Reserved,
        });
        Ok(Some(id))
    }

    fn atom(&mut self, tag: Tag, flag: bool) -> Result<Record<'a>> {
        let py = self.py;
        let value = match tag {
            Tag::Null => return Ok(Record::Null),
            Tag::None => return Ok(Record::Value(retained(py, MoltObject::none().bits()))),
            Tag::False | Tag::True => {
                return Ok(Record::Value(retained(
                    py,
                    MoltObject::from_bool(tag == Tag::True).bits(),
                )));
            }
            Tag::Ellipsis => return owned(py, molt_ellipsis()).map(Record::Value),
            Tag::StopIteration => {
                return Ok(Record::Value(retained(
                    py,
                    crate::builtins::exceptions::exception_class_bits(
                        py,
                        BuiltinExc::StopIteration,
                    ),
                )));
            }
            Tag::Code => {
                crate::builtins::compatibility_error::CompatibilityError::MarshalCodeObject
                    .raise::<()>(py);
                return Err(());
            }
            Tag::Ref => {
                let id = self.source.i32(py)?;
                let Some(Reference::Ready(value)) = usize::try_from(id)
                    .ok()
                    .and_then(|id| self.references.get(id))
                else {
                    return bad(py, "bad marshal data (invalid reference)");
                };
                return Ok(Record::Value(retained(py, value.bits())));
            }
            Tag::Int => owned(
                py,
                crate::builtins::numbers::int_bits_from_i64(py, self.source.i32(py)? as i64),
            )?,
            Tag::Int64 => {
                let value = i64::from_le_bytes(self.source.take(py, 8)?.try_into().unwrap());
                owned(py, crate::builtins::numbers::int_bits_from_i64(py, value))?
            }
            Tag::Long => {
                let length = self.source.i32(py)?;
                if length == i32::MIN {
                    return bad(py, "bad marshal data (long size out of range)");
                }
                let count = length.unsigned_abs() as usize;
                let mut value = BigInt::zero();
                // Read before building each digit: truncated enormous lengths
                // do not cause an enormous speculative allocation.
                for index in 0..count {
                    let digit = i16::from_le_bytes(self.source.take(py, 2)?.try_into().unwrap());
                    if digit < 0 {
                        return bad(py, "bad marshal data (digit out of range in long)");
                    }
                    if index + 1 == count && digit == 0 {
                        return bad(py, "bad marshal data (unnormalized long data)");
                    }
                    value += BigInt::from(digit) << (index * 15);
                }
                if length < 0 {
                    value = -value;
                }
                owned(
                    py,
                    crate::builtins::numbers::int_bits_from_bigint(py, value),
                )?
            }
            Tag::Float | Tag::BinaryFloat => {
                let number = self.source.float(py, tag == Tag::BinaryFloat)?;
                owned(py, float_result_bits(py, number))?
            }
            Tag::Complex | Tag::BinaryComplex => {
                let real = self.source.float(py, tag == Tag::BinaryComplex)?;
                let imag = self.source.float(py, tag == Tag::BinaryComplex)?;
                owned(py, crate::builtins::numbers::complex_bits(py, real, imag))?
            }
            Tag::Bytes => {
                let size = self.source.length(py, "bytes object")?;
                allocated(py, alloc_bytes(py, self.source.take(py, size)?))?
            }
            Tag::Unicode
            | Tag::Interned
            | Tag::Ascii
            | Tag::AsciiInterned
            | Tag::ShortAscii
            | Tag::ShortAsciiInterned => {
                let short = matches!(tag, Tag::ShortAscii | Tag::ShortAsciiInterned);
                let ascii = !matches!(tag, Tag::Unicode | Tag::Interned);
                let size = if short {
                    self.source.byte(py, false)? as usize
                } else {
                    self.source.length(py, "string")?
                };
                let bytes = if size == 0 && !ascii {
                    &[][..]
                } else {
                    self.source.take(py, size)?
                };
                let decoded = crate::object::ops_encoding::decode_bytes_text(
                    if ascii { "latin1" } else { "utf8" },
                    "surrogatepass",
                    bytes,
                )
                .map_err(|error| {
                    error.raise::<()>(py, bytes);
                })?
                .0;
                let interned = matches!(
                    tag,
                    Tag::Interned | Tag::AsciiInterned | Tag::ShortAsciiInterned
                );
                allocated(
                    py,
                    if interned {
                        crate::object::builders::alloc_interned_string(py, &decoded)
                    } else {
                        alloc_string(py, &decoded)
                    },
                )?
            }
            _ => unreachable!("container tags are handled by the frame machine"),
        };
        self.register(flag, Some(value.bits()))?;
        Ok(Record::Value(value))
    }

    fn begin(&mut self, tag: Tag, flag: bool) -> Result<Option<Record<'a>>> {
        let py = self.py;
        let (kind, owner, complete) = match tag {
            Tag::Tuple | Tag::SmallTuple => {
                let length = if tag == Tag::SmallTuple {
                    self.source.byte(py, false)? as usize
                } else {
                    self.source.length(py, "tuple")?
                };
                let owner = allocated(
                    py,
                    crate::object::builders::alloc_tuple_uninitialized(py, length),
                )?;
                if length > 0 {
                    self.constructing_tuples
                        .try_reserve(1)
                        .map_err(|_| memory_error(py))?;
                    self.constructing_tuples.insert(owner.bits());
                }
                (
                    FrameKind::Tuple { index: 0, length },
                    Some(owner),
                    length == 0,
                )
            }
            Tag::List => {
                let length = self.source.length(py, "list")?;
                // Grow with decoded items rather than trusting a hostile count.
                let owner = allocated(py, alloc_list(py, &[]))?;
                (
                    FrameKind::List { remaining: length },
                    Some(owner),
                    length == 0,
                )
            }
            Tag::Dict => (
                FrameKind::Dict { key: None },
                Some(allocated(py, alloc_dict_with_pairs(py, &[]))?),
                false,
            ),
            Tag::Set | Tag::FrozenSet => {
                let length = self.source.length(py, "set")?;
                if tag == Tag::Set {
                    (
                        FrameKind::Set { remaining: length },
                        Some(allocated(
                            py,
                            crate::object::builders::alloc_set_with_entries(py, &[]),
                        )?),
                        length == 0,
                    )
                } else if length == 0 {
                    (
                        FrameKind::FrozenSet {
                            remaining: 0,
                            children: Vec::new(),
                        },
                        Some(allocated(
                            py,
                            crate::object::builders::alloc_set_like_with_entries(
                                py,
                                &[],
                                TYPE_ID_FROZENSET,
                            ),
                        )?),
                        true,
                    )
                } else {
                    (
                        FrameKind::FrozenSet {
                            remaining: length,
                            children: Vec::new(),
                        },
                        None,
                        false,
                    )
                }
            }
            Tag::Slice if target_minor(py) >= 14 => (
                FrameKind::Slice {
                    children: Vec::new(),
                },
                None,
                false,
            ),
            Tag::Slice => return bad(py, "bad marshal data (unknown type code)"),
            _ => return self.atom(tag, flag).map(Some),
        };
        let reference = self.register(flag, owner.as_ref().map(Value::bits))?;
        if complete {
            return Ok(Some(Record::Value(owner.unwrap())));
        }
        reserve(py, &mut self.frames, 1)?;
        self.frames.push(Frame {
            kind,
            owner,
            reference,
        });
        Ok(None)
    }

    fn key_ready(&self, bits: u64) -> Result<()> {
        let py = self.py;
        let Some(pointer) = obj_from_bits(bits).as_ptr() else {
            return Ok(());
        };
        if !matches!(
            unsafe { object_type_id(pointer) },
            TYPE_ID_TUPLE | TYPE_ID_FROZENSET | TYPE_ID_SLICE
        ) {
            return Ok(());
        }
        let mut work = Vec::new();
        reserve(py, &mut work, 1)?;
        work.push(KeyVisit::Enter(bits));
        let mut seen = HashSet::new();
        let mut active = HashSet::new();
        while let Some(visit) = work.pop() {
            let bits = match visit {
                KeyVisit::Leave(bits) => {
                    active.remove(&bits);
                    continue;
                }
                KeyVisit::Enter(bits) => bits,
            };
            let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                continue;
            };
            if self.constructing_tuples.contains(&bits) || active.contains(&bits) {
                return bad(py, "bad marshal data (invalid reference)");
            }
            seen.try_reserve(1).map_err(|_| memory_error(py))?;
            if !seen.insert(bits) {
                continue;
            }
            let kind = unsafe { object_type_id(ptr) };
            if matches!(kind, TYPE_ID_TUPLE | TYPE_ID_FROZENSET | TYPE_ID_SLICE) {
                active.try_reserve(1).map_err(|_| memory_error(py))?;
                active.insert(bits);
                reserve(py, &mut work, 1)?;
                work.push(KeyVisit::Leave(bits));
            }
            if kind == TYPE_ID_TUPLE {
                let values = unsafe {
                    crate::object::seq_access::snapshot(py, ptr, "marshal key allocation failed")
                }
                .ok_or(())?;
                reserve(py, &mut work, values.len())?;
                work.extend(values.iter().copied().map(KeyVisit::Enter));
            } else if kind == TYPE_ID_FROZENSET {
                let values = unsafe { crate::object::ops_set::set_snapshot(py, ptr) }.ok_or(())?;
                reserve(py, &mut work, values.len())?;
                work.extend(values.iter().copied().map(KeyVisit::Enter));
            } else if kind == TYPE_ID_SLICE && target_minor(py) >= 12 {
                reserve(py, &mut work, 3)?;
                work.extend(
                    unsafe {
                        [
                            slice_start_bits(ptr),
                            slice_stop_bits(ptr),
                            slice_step_bits(ptr),
                        ]
                    }
                    .into_iter()
                    .map(KeyVisit::Enter),
                );
            }
        }
        Ok(())
    }

    fn accept(&mut self, record: Record<'a>) -> Result<Option<Record<'a>>> {
        let py = self.py;
        let mut frame = self.frames.pop().unwrap();
        let Record::Value(value) = record else {
            match frame.kind {
                FrameKind::Dict { .. } => {
                    return Ok(Some(Record::Value(frame.owner.take().unwrap())));
                }
                FrameKind::Slice { .. } => return Ok(Some(Record::Null)),
                FrameKind::Tuple { .. } => {
                    return fail(
                        py,
                        BuiltinExc::TypeError,
                        "NULL object in marshal data for tuple",
                    );
                }
                FrameKind::List { .. } => {
                    return fail(
                        py,
                        BuiltinExc::TypeError,
                        "NULL object in marshal data for list",
                    );
                }
                _ => {
                    return fail(
                        py,
                        BuiltinExc::TypeError,
                        "NULL object in marshal data for set",
                    );
                }
            }
        };
        let complete = match &mut frame.kind {
            FrameKind::Tuple { index, length } => {
                let ptr = obj_from_bits(frame.owner.as_ref().unwrap().bits())
                    .as_ptr()
                    .unwrap();
                let old = unsafe {
                    crate::object::seq_access::replace_capi_item(py, ptr, *index, value.bits())
                }
                .ok_or(())?;
                dec_ref_bits(py, old);
                *index += 1;
                *index == *length
            }
            FrameKind::List { remaining } => {
                let result = owned(
                    py,
                    molt_list_append(frame.owner.as_ref().unwrap().bits(), value.bits()),
                )?;
                drop(result);
                *remaining -= 1;
                *remaining == 0
            }
            FrameKind::Dict { key } => {
                if let Some(key_value) = key.take() {
                    self.key_ready(key_value.bits())?;
                    let ptr = obj_from_bits(frame.owner.as_ref().unwrap().bits())
                        .as_ptr()
                        .unwrap();
                    if !unsafe { dict_set_in_place(py, ptr, key_value.bits(), value.bits()) } {
                        return Err(());
                    }
                } else {
                    *key = Some(value);
                }
                false
            }
            FrameKind::Set { remaining } => {
                self.key_ready(value.bits())?;
                let ptr = obj_from_bits(frame.owner.as_ref().unwrap().bits())
                    .as_ptr()
                    .unwrap();
                if !unsafe { set_add_in_place(py, ptr, value.bits(), HashContext::SetElement) } {
                    return Err(());
                }
                *remaining -= 1;
                *remaining == 0
            }
            FrameKind::FrozenSet {
                remaining,
                children,
            } => {
                self.key_ready(value.bits())?;
                reserve(py, children, 1)?;
                children.push(value);
                *remaining -= 1;
                *remaining == 0
            }
            FrameKind::Slice { children } => {
                reserve(py, children, 1)?;
                children.push(value);
                children.len() == 3
            }
        };
        if !complete {
            self.frames.push(frame);
            return Ok(None);
        }
        let result = match frame.kind {
            FrameKind::FrozenSet { children, .. } => {
                let mut bits = Vec::new();
                reserve(py, &mut bits, children.len())?;
                bits.extend(children.iter().map(Value::bits));
                allocated(
                    py,
                    crate::object::builders::alloc_set_like_with_entries(
                        py,
                        &bits,
                        TYPE_ID_FROZENSET,
                    ),
                )?
            }
            FrameKind::Slice { children } => allocated(
                py,
                alloc_slice_obj(
                    py,
                    children[0].bits(),
                    children[1].bits(),
                    children[2].bits(),
                ),
            )?,
            FrameKind::Tuple { .. } => {
                let result = frame.owner.take().unwrap();
                self.constructing_tuples.remove(&result.bits());
                result
            }
            _ => frame.owner.take().unwrap(),
        };
        if let Some(RefId(index)) = frame.reference {
            if matches!(self.references[index], Reference::Reserved) {
                self.references[index] = Reference::Ready(retained(py, result.bits()));
            }
        }
        Ok(Some(Record::Value(result)))
    }

    fn run(mut self) -> Result<Value<'a>> {
        loop {
            let code = self.source.byte(self.py, true)?;
            if self.frames.len() >= depth_limit(self.py) {
                return bad(self.py, "recursion limit exceeded");
            }
            let tag = Tag::decode(code & 0x7f).ok_or_else(|| {
                raise_exception::<()>(
                    self.py,
                    BuiltinExc::ValueError,
                    "bad marshal data (unknown type code)",
                );
            })?;
            let Some(mut result) = self.begin(tag, code & 0x80 != 0)? else {
                continue;
            };
            loop {
                if self.frames.is_empty() {
                    return match result {
                        Record::Value(value) => Ok(value),
                        Record::Null => fail(
                            self.py,
                            BuiltinExc::TypeError,
                            "NULL object in marshal data for object",
                        ),
                    };
                }
                let Some(parent) = self.accept(result)? else {
                    break;
                };
                result = parent;
            }
        }
    }
}

pub(super) fn decode<'a>(py: &'a PyToken<'_>, source: Source<'_>) -> Result<Value<'a>> {
    Reader {
        py,
        source,
        references: Vec::new(),
        frames: Vec::new(),
        constructing_tuples: HashSet::new(),
    }
    .run()
}
