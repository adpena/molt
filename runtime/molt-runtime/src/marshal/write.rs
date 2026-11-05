use super::*;
use crate::object::seq_access::PinnedSequenceSnapshot;
use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(Clone, Copy)]
enum Kind {
    None,
    Bool(bool),
    StopIteration,
    Ellipsis,
    Int,
    Float,
    Complex,
    String,
    Bytes,
    Tuple,
    List,
    Dict,
    Set,
    FrozenSet,
    Slice,
    Buffer,
}

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
enum Identity {
    Heap(u64),
    SmallInt(i16),
}

#[derive(Clone, Copy)]
enum Publication {
    Ready,
    SliceChildren,
}

struct Reference<'py> {
    id: RefId,
    state: Publication,
    _owner: Value<'py>,
}

enum Snapshot<'a, 'py> {
    Pinned(PinnedSequenceSnapshot<'a, 'py>),
    Owned(Vec<Value<'a>>),
}

impl Snapshot<'_, '_> {
    fn len(&self) -> usize {
        match self {
            Self::Pinned(values) => values.len(),
            Self::Owned(values) => values.len(),
        }
    }
    fn get(&self, index: usize) -> u64 {
        match self {
            Self::Pinned(values) => values[index],
            Self::Owned(values) => values[index].bits(),
        }
    }
}

struct SetSort<'a, 'py> {
    source: u64,
    values: Snapshot<'a, 'py>,
    keys: Vec<(Vec<u8>, usize)>,
    next: usize,
    depth: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum KeyEffects {
    Unknown,
    BuiltinsOnly,
}

enum Action<'a, 'py> {
    Value {
        bits: u64,
        depth: usize,
        effects: KeyEffects,
    },
    Children {
        values: Snapshot<'a, 'py>,
        next: usize,
        depth: usize,
        effects: KeyEffects,
    },
    ReleaseSemantic(Snapshot<'a, 'py>),
    SetSort(SetSort<'a, 'py>),
    SetKey(SetSort<'a, 'py>),
    Null {
        depth: usize,
    },
    CompleteSlice(Identity),
}

struct Context<'a, 'py> {
    out: Vec<u8>,
    actions: Vec<Action<'a, 'py>>,
    references: HashMap<Identity, Reference<'a>>,
}

impl<'a, 'py> Context<'a, 'py> {
    fn new(py: &PyToken<'_>, bits: u64) -> Result<Self> {
        let mut actions = Vec::new();
        reserve(py, &mut actions, 1)?;
        actions.push(Action::Value {
            bits,
            depth: 1,
            effects: KeyEffects::Unknown,
        });
        Ok(Self {
            out: Vec::new(),
            actions,
            references: HashMap::new(),
        })
    }
}

struct Writer<'a, 'py> {
    py: &'a PyToken<'py>,
    version: i32,
    contexts: Vec<Context<'a, 'py>>,
    /// Snapshot/C-boundary pins not present in CPython's recursive writer.
    incidental: HashMap<u64, usize>,
    completed_key: Option<Vec<u8>>,
}

fn native(bits: u64) -> Option<*mut molt_cpython_abi::abi_types::PyObject> {
    let ptr = obj_from_bits(bits).as_ptr()?;
    (unsafe { object_type_id(ptr) } == TYPE_ID_FOREIGN).then(|| {
        std::ptr::with_exposed_provenance_mut(unsafe {
            crate::object::foreign::foreign_ptr_from_obj(ptr)
        })
    })
}

fn native_failure(py: &PyToken<'_>, operation: &str) {
    crate::cpython_abi_hooks::propagate_native_failure(py, operation);
}

fn kind(py: &PyToken<'_>, bits: u64) -> Result<Kind> {
    let value = obj_from_bits(bits);
    if value.is_none() {
        return Ok(Kind::None);
    }
    if let Some(value) = value.as_bool() {
        return Ok(Kind::Bool(value));
    }
    if value.is_int() {
        return Ok(Kind::Int);
    }
    if value.is_float() {
        return Ok(Kind::Float);
    }
    let class = type_of_bits(py, bits);
    let builtins = builtin_classes(py);
    if class == builtins.int {
        return Ok(Kind::Int);
    }
    if class == builtins.float {
        return Ok(Kind::Float);
    }
    if class == builtins.complex {
        return Ok(Kind::Complex);
    }
    if class == builtins.str {
        return Ok(Kind::String);
    }
    if class == builtins.bytes {
        return Ok(Kind::Bytes);
    }
    if class == builtins.tuple {
        return Ok(Kind::Tuple);
    }
    if class == builtins.list {
        return Ok(Kind::List);
    }
    if class == builtins.dict
        && value
            .as_ptr()
            .is_some_and(|pointer| unsafe { object_type_id(pointer) } == TYPE_ID_DICT)
    {
        return Ok(Kind::Dict);
    }
    if class == builtins.set
        && value
            .as_ptr()
            .is_some_and(|pointer| unsafe { object_type_id(pointer) } == TYPE_ID_SET)
    {
        return Ok(Kind::Set);
    }
    if class == builtins.frozenset
        && value
            .as_ptr()
            .is_some_and(|pointer| unsafe { object_type_id(pointer) } == TYPE_ID_FROZENSET)
    {
        return Ok(Kind::FrozenSet);
    }
    if class == builtins.slice
        && value
            .as_ptr()
            .is_some_and(|pointer| unsafe { object_type_id(pointer) } == TYPE_ID_SLICE)
    {
        return Ok(Kind::Slice);
    }
    if class == builtins.ellipsis_type {
        return Ok(Kind::Ellipsis);
    }
    if bits == crate::builtins::exceptions::exception_class_bits(py, BuiltinExc::StopIteration) {
        return Ok(Kind::StopIteration);
    }
    match crate::object::buffer_exports::supports_buffer(py, bits) {
        Ok(true) => Ok(Kind::Buffer),
        Ok(false) => bad(py, "unmarshallable object"),
        Err(_) => bad(py, "unmarshallable object"),
    }
}

fn integer(py: &PyToken<'_>, bits: u64) -> Result<BigInt> {
    crate::builtins::numbers::integer_storage_bigint(py, bits)?.ok_or_else(|| {
        raise_exception::<()>(py, BuiltinExc::ValueError, "unmarshallable object");
    })
}

fn float(py: &PyToken<'_>, bits: u64) -> Result<f64> {
    if let Some(value) = as_float_extended(obj_from_bits(bits)) {
        return Ok(value);
    }
    let pointer = native(bits).ok_or(())?;
    unsafe { molt_cpython_abi::api::numbers::float_as_double(pointer) }
        .map_err(|_| native_failure(py, "marshal float storage"))
}

impl<'a, 'py> Writer<'a, 'py> {
    fn context(&mut self) -> &mut Context<'a, 'py> {
        self.contexts.last_mut().unwrap()
    }

    fn push(&mut self, action: Action<'a, 'py>) -> Result<()> {
        let py = self.py;
        let actions = &mut self.context().actions;
        reserve(py, actions, 1)?;
        actions.push(action);
        Ok(())
    }

    fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let py = self.py;
        let out = &mut self.context().out;
        reserve(py, out, bytes.len())?;
        out.extend_from_slice(bytes);
        Ok(())
    }

    fn tag(&mut self, tag: Tag, flag: bool) -> Result<()> {
        self.bytes(&[tag as u8 | if flag { 0x80 } else { 0 }])
    }
    fn i32(&mut self, value: i32) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }

    fn length(&mut self, length: usize) -> Result<()> {
        let value = i32::try_from(length).map_err(|_| {
            raise_exception::<()>(self.py, BuiltinExc::ValueError, "unmarshallable object");
        })?;
        self.i32(value)
    }

    fn add_incidental(&mut self, bits: u64) -> Result<()> {
        if obj_from_bits(bits).as_ptr().is_some() {
            self.incidental
                .try_reserve(1)
                .map_err(|_| memory_error(self.py))?;
            *self.incidental.entry(bits).or_default() += 1;
        }
        Ok(())
    }

    fn remove_incidental(&mut self, bits: u64) {
        if let Some(count) = self.incidental.get_mut(&bits) {
            *count -= 1;
            if *count == 0 {
                self.incidental.remove(&bits);
            }
        }
    }

    fn snapshot(&mut self, bits: u64, kind: Kind) -> Result<Snapshot<'a, 'py>> {
        let py = self.py;
        let ptr = obj_from_bits(bits).as_ptr().ok_or(())?;
        let snapshot = match kind {
            Kind::Tuple if native(bits).is_none() => Snapshot::Pinned(
                unsafe {
                    crate::object::seq_access::snapshot(
                        py,
                        ptr,
                        "marshal tuple snapshot allocation failed",
                    )
                }
                .ok_or(())?,
            ),
            Kind::Tuple => {
                let tuple =
                    crate::object::tuple_storage::TupleStorage::from_bits(py, bits).ok_or(())?;
                let length = tuple.len().ok_or(())?;
                let mut values = Vec::new();
                reserve(py, &mut values, length)?;
                for index in 0..length {
                    values.push(owned(py, tuple.item(index).ok_or(())?)?);
                }
                Snapshot::Owned(values)
            }
            Kind::List if unsafe { object_type_id(ptr) } == TYPE_ID_LIST => Snapshot::Pinned(
                unsafe {
                    crate::object::seq_access::snapshot(
                        py,
                        ptr,
                        "marshal list snapshot allocation failed",
                    )
                }
                .ok_or(())?,
            ),
            Kind::List => {
                let list =
                    crate::object::list_storage::ListStorage::from_bits(py, bits).ok_or(())?;
                let length = list.len().ok_or(())?;
                let mut values = Vec::new();
                reserve(py, &mut values, length)?;
                for index in 0..length {
                    values.push(owned(py, list.item(index).ok_or(())?.into_bits())?);
                }
                Snapshot::Owned(values)
            }
            Kind::Dict => Snapshot::Pinned(
                unsafe {
                    crate::object::ops_dict::dict_snapshot(
                        py,
                        ptr,
                        crate::object::ops_dict::DictSnapshotKind::Entries,
                    )
                }
                .ok_or(())?,
            ),
            Kind::Set | Kind::FrozenSet => Snapshot::Pinned(
                unsafe { crate::object::ops_set::set_snapshot(py, ptr) }.ok_or(())?,
            ),
            Kind::Slice => {
                let mut values = Vec::new();
                reserve(py, &mut values, 3)?;
                for child in unsafe {
                    [
                        slice_start_bits(ptr),
                        slice_stop_bits(ptr),
                        slice_step_bits(ptr),
                    ]
                } {
                    values.push(retained(py, child));
                }
                Snapshot::Owned(values)
            }
            _ => unreachable!(),
        };
        for index in 0..snapshot.len() {
            self.add_incidental(snapshot.get(index))?;
        }
        Ok(snapshot)
    }

    fn shared(&self, bits: u64) -> bool {
        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
            return false;
        };
        let header = unsafe { &*header_from_obj_ptr(ptr) };
        let flags = header.load_metadata_flags();
        if flags & (crate::object::HEADER_FLAG_IMMORTAL | crate::object::HEADER_FLAG_INTERNED) != 0
        {
            return true;
        }
        let count = header.ref_count_snapshot() as usize;
        let extra = self.incidental.get(&bits).copied().unwrap_or(0);
        let runtime = count.saturating_sub(extra);
        if let Some(pointer) = native(bits) {
            // The FOREIGN lifetime owns one physical C reference. Its runtime
            // owners and the remaining native owners jointly describe sharing;
            // snapshot/ingress pins are counted separately above.
            let physical = unsafe { (*pointer).ob_refcnt };
            if molt_cpython_abi::abi_types::is_immortal_refcnt(physical) {
                return true;
            }
            runtime.saturating_add((physical.max(0) as usize).saturating_sub(1)) > 1
        } else {
            runtime > 1
        }
    }

    /// Return None after emitting an existing reference, else the first tag flag.
    fn reference(&mut self, bits: u64, kind: Kind, int: Option<&BigInt>) -> Result<Option<bool>> {
        if self.version < 3 {
            return Ok(Some(false));
        }
        let identity = match kind {
            Kind::Int => match int
                .and_then(BigInt::to_i16)
                .filter(|value| (-5..=256).contains(value))
            {
                Some(value) => Identity::SmallInt(value),
                None => return Ok(Some(false)),
            },
            Kind::Float | Kind::Complex => return Ok(Some(false)),
            _ => {
                if !self.shared(bits) {
                    return Ok(Some(false));
                }
                Identity::Heap(bits)
            }
        };
        if let Some(reference) = self.context().references.get(&identity) {
            if matches!(reference.state, Publication::SliceChildren) {
                return bad(self.py, "cannot marshal recursion slice objects");
            }
            let id = reference.id.0 as i32;
            self.tag(Tag::Ref, false)?;
            self.i32(id)?;
            return Ok(None);
        }
        let py = self.py;
        let context = self.context();
        if context.references.len() >= i32::MAX as usize {
            return bad(py, "too many objects");
        }
        context
            .references
            .try_reserve(1)
            .map_err(|_| memory_error(py))?;
        let id = RefId(context.references.len());
        let state = if matches!(kind, Kind::Slice) {
            Publication::SliceChildren
        } else {
            Publication::Ready
        };
        context.references.insert(
            identity,
            Reference {
                id,
                state,
                _owner: retained(py, bits),
            },
        );
        if matches!(kind, Kind::Slice) {
            self.push(Action::CompleteSlice(identity))?;
        }
        Ok(Some(true))
    }

    fn write_float(&mut self, value: f64) -> Result<()> {
        let value = if value.is_nan() {
            f64::from_bits(0x7ff8_0000_0000_0000)
        } else {
            value
        };
        if self.version > 1 {
            return self.bytes(&value.to_le_bytes());
        }
        let spec = crate::object::ops_format::FormatSpec {
            precision: Some(17),
            ty: Some('g' as u32),
            ..Default::default()
        };
        let text = crate::object::ops_format::format_float_value_with_spec(value, &spec).map_err(
            |error| {
                error.raise::<()>(self.py);
            },
        )?;
        self.bytes(&[u8::try_from(text.len()).expect("g17 fits one-byte length")])?;
        self.bytes(&text)
    }

    fn write_integer(&mut self, value: &BigInt, flag: bool) -> Result<()> {
        if let Some(value) = value.to_i32() {
            self.tag(Tag::Int, flag)?;
            return self.i32(value);
        }
        self.tag(Tag::Long, flag)?;
        let (sign, words) = value.to_u32_digits();
        let bit_length = value.bits() as usize;
        let count = bit_length.div_ceil(15);
        let signed = i32::try_from(count).map_err(|_| {
            raise_exception::<()>(self.py, BuiltinExc::ValueError, "unmarshallable object");
        })?;
        self.i32(if sign == Sign::Minus { -signed } else { signed })?;
        let mut accumulator = 0u64;
        let mut available = 0;
        let mut next = 0;
        for _ in 0..count {
            if available < 15 {
                if next < words.len() {
                    accumulator |= (words[next] as u64) << available;
                    next += 1;
                }
                available += 32;
            }
            self.bytes(&((accumulator & 0x7fff) as u16).to_le_bytes())?;
            accumulator >>= 15;
            available -= 15;
        }
        Ok(())
    }

    fn string(&mut self, bits: u64, flag: bool) -> Result<()> {
        let py = self.py;
        let mut native_bytes = Vec::new();
        let (bytes, interned) = if let Some(native) = native(bits) {
            let length = unsafe { molt_cpython_abi::api::strings::PyUnicode_GetLength(native) };
            if length < 0 {
                native_failure(py, "marshal string storage");
                return Err(());
            }
            reserve(
                py,
                &mut native_bytes,
                (length as usize)
                    .checked_mul(4)
                    .ok_or_else(|| memory_error(py))?,
            )?;
            for index in 0..length {
                let code =
                    unsafe { molt_cpython_abi::api::strings::PyUnicode_ReadChar(native, index) };
                if code == u32::MAX {
                    native_failure(py, "marshal string storage");
                    return Err(());
                }
                molt_obj_model::text_storage::push_code_point(&mut native_bytes, code);
            }
            (&native_bytes[..], false)
        } else {
            let ptr = obj_from_bits(bits).as_ptr().unwrap();
            (
                unsafe { std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr)) },
                unsafe { (*header_from_obj_ptr(ptr)).load_metadata_flags() }
                    & crate::object::HEADER_FLAG_INTERNED
                    != 0,
            )
        };
        if self.version >= 4 && bytes.is_ascii() {
            if bytes.len() < 256 {
                self.tag(
                    if interned {
                        Tag::ShortAsciiInterned
                    } else {
                        Tag::ShortAscii
                    },
                    flag,
                )?;
                self.bytes(&[bytes.len() as u8])?;
            } else {
                self.tag(
                    if interned {
                        Tag::AsciiInterned
                    } else {
                        Tag::Ascii
                    },
                    flag,
                )?;
                self.length(bytes.len())?;
            }
        } else {
            self.tag(
                if interned && self.version >= 3 {
                    Tag::Interned
                } else {
                    Tag::Unicode
                },
                flag,
            )?;
            self.length(bytes.len())?;
        }
        self.bytes(bytes)
    }

    /// Prove a singleton sort subtree cannot invoke an exporter. The proof
    /// travels down its frames; nested singleton sets do not rescan the tree.
    fn key_effects(&self, root: u64, remaining_depth: usize) -> Result<KeyEffects> {
        let py = self.py;
        let value = obj_from_bits(root);
        if value.is_none() || value.is_bool() || value.is_int() || value.is_float() {
            return Ok(KeyEffects::BuiltinsOnly);
        }
        let builtins = builtin_classes(py);
        let stop = crate::builtins::exceptions::exception_class_bits(py, BuiltinExc::StopIteration);
        let mut work = Vec::new();
        reserve(py, &mut work, 1)?;
        work.push((root, 1usize));
        let mut seen = HashMap::new();
        while let Some((bits, depth)) = work.pop() {
            if depth > remaining_depth {
                return Ok(KeyEffects::Unknown);
            }
            let object = obj_from_bits(bits);
            if object.is_none()
                || object.is_bool()
                || object.is_int()
                || object.is_float()
                || bits == stop
            {
                continue;
            }
            seen.try_reserve(1).map_err(|_| memory_error(py))?;
            if seen.get(&bits).is_some_and(|&previous| previous >= depth) {
                continue;
            }
            seen.insert(bits, depth);
            let class = type_of_bits(py, bits);
            if [
                builtins.int,
                builtins.float,
                builtins.complex,
                builtins.str,
                builtins.bytes,
                builtins.ellipsis_type,
            ]
            .contains(&class)
            {
                continue;
            }
            if native(bits).is_some() {
                return Ok(KeyEffects::Unknown);
            }
            let Some(pointer) = object.as_ptr() else {
                return Ok(KeyEffects::Unknown);
            };
            if class == builtins.tuple || class == builtins.frozenset {
                let values = if class == builtins.tuple {
                    unsafe {
                        crate::object::seq_access::snapshot(
                            py,
                            pointer,
                            "marshal key proof allocation failed",
                        )
                    }
                } else {
                    unsafe { crate::object::ops_set::set_snapshot(py, pointer) }
                }
                .ok_or(())?;
                reserve(py, &mut work, values.len())?;
                work.extend(values.iter().map(|&bits| (bits, depth + 1)));
            } else if class == builtins.slice {
                reserve(py, &mut work, 3)?;
                work.extend(
                    unsafe {
                        [
                            slice_start_bits(pointer),
                            slice_stop_bits(pointer),
                            slice_step_bits(pointer),
                        ]
                    }
                    .into_iter()
                    .map(|bits| (bits, depth + 1)),
                );
            } else {
                return Ok(KeyEffects::Unknown);
            }
        }
        Ok(KeyEffects::BuiltinsOnly)
    }

    fn visit(&mut self, bits: u64, depth: usize, effects: KeyEffects) -> Result<()> {
        let py = self.py;
        if depth > depth_limit(py) {
            return bad(py, "object too deeply nested to marshal");
        }
        let kind = kind(py, bits)?;
        match kind {
            Kind::None => return self.tag(Tag::None, false),
            Kind::Bool(value) => {
                return self.tag(if value { Tag::True } else { Tag::False }, false);
            }
            Kind::StopIteration => return self.tag(Tag::StopIteration, false),
            Kind::Ellipsis => return self.tag(Tag::Ellipsis, false),
            Kind::Slice if self.version < 5 || target_minor(py) < 14 => {
                return bad(py, "unmarshallable object");
            }
            _ => {}
        }
        let int = if matches!(kind, Kind::Int) {
            Some(integer(py, bits)?)
        } else {
            None
        };
        let Some(flag) = self.reference(bits, kind, int.as_ref())? else {
            return Ok(());
        };
        match kind {
            Kind::Int => self.write_integer(int.as_ref().unwrap(), flag),
            Kind::Float => {
                self.tag(
                    if self.version > 1 {
                        Tag::BinaryFloat
                    } else {
                        Tag::Float
                    },
                    flag,
                )?;
                self.write_float(float(py, bits)?)
            }
            Kind::Complex => {
                let (real, imag) =
                    if let Some(ptr) = crate::builtins::numbers::complex_ptr_from_bits(bits) {
                        let parts = unsafe { crate::builtins::numbers::complex_ref(ptr) };
                        (parts.re, parts.im)
                    } else {
                        let parts = unsafe {
                            molt_cpython_abi::api::numbers::PyComplex_AsCComplex(
                                native(bits).ok_or(())?,
                            )
                        };
                        if molt_cpython_abi::api::errors::raised_error_pending() {
                            native_failure(py, "marshal complex storage");
                            return Err(());
                        }
                        (parts.real, parts.imag)
                    };
                self.tag(
                    if self.version > 1 {
                        Tag::BinaryComplex
                    } else {
                        Tag::Complex
                    },
                    flag,
                )?;
                self.write_float(real)?;
                self.write_float(imag)
            }
            Kind::String => self.string(bits, flag),
            Kind::Bytes | Kind::Buffer => {
                let buffer = crate::object::buffer_exports::ScopedBuffer::simple(py, bits)
                    .map_err(|_| {
                        raise_exception::<()>(py, BuiltinExc::ValueError, "unmarshallable object");
                    })?;
                self.tag(Tag::Bytes, flag)?;
                self.length(buffer.as_slice().len())?;
                self.bytes(buffer.as_slice())
            }
            Kind::Tuple | Kind::List | Kind::Dict | Kind::Set | Kind::FrozenSet | Kind::Slice => {
                let values = self.snapshot(bits, kind)?;
                let length = values.len();
                match kind {
                    Kind::Tuple if self.version >= 4 && length < 256 => {
                        self.tag(Tag::SmallTuple, flag)?;
                        self.bytes(&[length as u8])?;
                    }
                    Kind::Tuple | Kind::List | Kind::Set | Kind::FrozenSet => {
                        let tag = match kind {
                            Kind::Tuple => Tag::Tuple,
                            Kind::List => Tag::List,
                            Kind::Set => Tag::Set,
                            _ => Tag::FrozenSet,
                        };
                        self.tag(tag, flag)?;
                        self.length(length)?;
                    }
                    Kind::Dict => {
                        self.tag(Tag::Dict, flag)?;
                        self.push(Action::Null { depth: depth + 1 })?;
                    }
                    Kind::Slice => self.tag(Tag::Slice, flag)?,
                    _ => unreachable!(),
                }
                if matches!(kind, Kind::Set | Kind::FrozenSet) {
                    let singleton_effects = if length == 1 && effects == KeyEffects::Unknown {
                        self.key_effects(values.get(0), depth_limit(py).saturating_sub(depth))?
                    } else {
                        effects
                    };
                    if length == 0 || (length == 1 && singleton_effects == KeyEffects::BuiltinsOnly)
                    {
                        for index in 0..length {
                            self.remove_incidental(values.get(index));
                        }
                        let first = if length == 1 {
                            Some(values.get(0))
                        } else {
                            None
                        };
                        self.push(Action::ReleaseSemantic(values))?;
                        if let Some(bits) = first {
                            self.push(Action::Value {
                                bits,
                                depth: depth + 1,
                                effects: singleton_effects,
                            })?;
                        }
                        Ok(())
                    } else {
                        self.push(Action::SetSort(SetSort {
                            source: bits,
                            values,
                            keys: Vec::new(),
                            next: 0,
                            depth,
                        }))
                    }
                } else {
                    self.push(Action::Children {
                        values,
                        next: 0,
                        depth: depth + 1,
                        effects,
                    })
                }
            }
            _ => unreachable!(),
        }
    }

    fn sort(&mut self, state: SetSort<'a, 'py>) -> Result<()> {
        if target_minor(self.py) >= 14 {
            let pointer = obj_from_bits(state.source).as_ptr().ok_or(())?;
            if unsafe { set_len(pointer) } != state.values.len() {
                return fail(
                    self.py,
                    BuiltinExc::RuntimeError,
                    "set changed size during iteration",
                );
            }
        }
        if state.next < state.values.len() {
            let bits = state.values.get(state.next);
            self.remove_incidental(bits); // current loop/sort-record semantic retain
            let context = Context::new(self.py, bits)?;
            self.push(Action::SetKey(state))?;
            reserve(self.py, &mut self.contexts, 1)?;
            self.contexts.push(context);
            return Ok(());
        }
        let py = self.py;
        // Python comparisons need not form a total order (notably NaNs).
        // Reuse the same fallible, less-than-only sort as Python list.sort;
        // Rust's sort_by requires a total-order comparator.
        let mut order = Vec::new();
        reserve(py, &mut order, state.keys.len())?;
        order.extend(0..state.keys.len());
        let runs = if target_minor(py) >= 13 {
            molt_obj_model::timsort::RunDetection::WeaklyDescending
        } else {
            molt_obj_model::timsort::RunDetection::StrictlyDescending
        };
        let sorted = molt_obj_model::timsort::timsort(&mut order, runs, |li, ri| {
            let (left, left_index) = &state.keys[*li];
            let (right, right_index) = &state.keys[*ri];
            match left.cmp(right) {
                Ordering::Less => return Ok(true),
                Ordering::Greater => return Ok(false),
                Ordering::Equal => {}
            }
            if target_minor(py) >= 14 {
                return Ok(left_index < right_index);
            }
            let left = state.values.get(*left_index);
            let right = state.values.get(*right_index);
            if left == right {
                return Ok(false);
            }
            let value = owned(py, molt_eq(left, right))?;
            let equal = is_truthy(py, obj_from_bits(value.bits()));
            if exception_pending(py) {
                return Err(());
            }
            if equal {
                return Ok(false);
            }
            let value = owned(py, molt_lt(left, right))?;
            let less = is_truthy(py, obj_from_bits(value.bits()));
            if exception_pending(py) {
                return Err(());
            }
            Ok(less)
        });
        if sorted.is_err() {
            // marshal.c maps any PyList_Sort failure (including an element's
            // comparison error) to WFERR_NOMEMORY, then PyErr_NoMemory.
            memory_error(py);
            return Err(());
        }
        let depth = state.depth + 1;
        // Retain the sorting records through final emission, as CPython does.
        let mut actions = Vec::new();
        reserve(py, &mut actions, state.keys.len())?;
        for position in order.iter().rev() {
            let index = state.keys[*position].1;
            actions.push(Action::Value {
                bits: state.values.get(index),
                depth,
                effects: KeyEffects::Unknown,
            });
        }
        self.push(Action::ReleaseSemantic(state.values))?;
        for action in actions {
            self.push(action)?;
        }
        Ok(())
    }

    fn run(mut self) -> Result<Vec<u8>> {
        let result = self.run_inner();
        if result.is_err() && self.contexts.len() > 1 {
            // A failed standalone set-key dump is WFERR_UNMARSHALLABLE in
            // its enclosing writer, regardless of the nested failure kind.
            return bad(self.py, "unmarshallable object");
        }
        result
    }

    fn run_inner(&mut self) -> Result<Vec<u8>> {
        loop {
            let Some(action) = self.context().actions.pop() else {
                let complete = self.contexts.pop().unwrap();
                let bytes = complete.out;
                if self.contexts.is_empty() {
                    return Ok(bytes);
                }
                self.completed_key = Some(bytes);
                continue;
            };
            match action {
                Action::Value {
                    bits,
                    depth,
                    effects,
                } => self.visit(bits, depth, effects)?,
                Action::Children {
                    values,
                    next,
                    depth,
                    effects,
                } => {
                    if next == values.len() {
                        for index in 0..values.len() {
                            self.remove_incidental(values.get(index));
                        }
                        drop(values);
                    } else {
                        let bits = values.get(next);
                        self.push(Action::Children {
                            values,
                            next: next + 1,
                            depth,
                            effects,
                        })?;
                        self.push(Action::Value {
                            bits,
                            depth,
                            effects,
                        })?;
                    }
                }
                Action::ReleaseSemantic(values) => drop(values),
                Action::SetSort(state) => self.sort(state)?,
                Action::SetKey(mut state) => {
                    let key = self
                        .completed_key
                        .take()
                        .expect("completed child key context");
                    reserve(self.py, &mut state.keys, 1)?;
                    state.keys.push((key, state.next));
                    state.next += 1;
                    self.push(Action::SetSort(state))?;
                }
                Action::Null { depth } => {
                    if depth > depth_limit(self.py) {
                        return bad(self.py, "object too deeply nested to marshal");
                    }
                    self.tag(Tag::Null, false)?;
                }
                Action::CompleteSlice(identity) => {
                    self.context().references.get_mut(&identity).unwrap().state =
                        Publication::Ready;
                }
            }
        }
    }
}

pub(super) fn encode(py: &PyToken<'_>, bits: u64, version: i32) -> Result<Vec<u8>> {
    encode_with_ownership(
        py,
        bits,
        version,
        molt_cpython_abi::hooks::MarshalInputOwnership::Borrowed,
    )
}

pub(super) fn encode_with_ownership(
    py: &PyToken<'_>,
    bits: u64,
    version: i32,
    ownership: molt_cpython_abi::hooks::MarshalInputOwnership,
) -> Result<Vec<u8>> {
    let mut writer = Writer {
        py,
        version,
        contexts: Vec::new(),
        incidental: HashMap::new(),
        completed_key: None,
    };
    if ownership == molt_cpython_abi::hooks::MarshalInputOwnership::TemporaryWrapper {
        writer.add_incidental(bits)?;
    }
    reserve(py, &mut writer.contexts, 1)?;
    writer.contexts.push(Context::new(py, bits)?);
    writer.run()
}
