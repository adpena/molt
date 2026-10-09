//! Call argument custody, builder provenance and instruction release order.
//!
//! State stays with its existing owner; calls preserve transaction and drop order.

use super::*;

/// CPython's two call instructions, fixed by the compiler at each call site.
/// A `Stack` call (CALL) passes value-stack entries that an inlined Python
/// frame takes over. An `Expanded` call (CALL_FUNCTION_EX) passes a positional
/// tuple and a keyword mapping, which own the arguments until frame admission
/// ends and lend their contents to the frame. Runtime values cannot recover
/// the form; the frontend's call-argument schedule records it on
/// `callargs_new`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CallForm {
    Stack,
    Expanded,
}

pub(crate) struct CallArgs {
    pub(super) pos: Vec<u64>,
    pub(super) keywords: u64,
    pub(super) form: CallForm,
}

impl CallArgs {
    /// Read the live dictionary, never a projection cached during expansion.
    pub(super) unsafe fn keyword_count(&self) -> usize {
        obj_from_bits(self.keywords)
            .as_ptr()
            .map_or(0, |dict| unsafe { dict_len(dict) })
    }
}

/// True when the caller's reference is the only one: no other owner, borrowed
/// C view or frozen-layout authority can observe a mutation of `ptr`. Moving
/// edges out of an object needs this proof; without it a call retains copies.
unsafe fn exclusively_owned(ptr: *mut u8) -> bool {
    const SHARED_AUTHORITY: u32 = crate::object::HEADER_FLAG_HAS_ABI_VIEW
        | crate::object::HEADER_FLAG_FROZEN_LAYOUT_MAP
        | crate::object::HEADER_FLAG_IMMORTAL;
    let header = unsafe { &*header_from_obj_ptr(ptr) };
    header.is_uniquely_owned() && header.load_synchronized_flags() & SHARED_AUTHORITY == 0
}

/// Who owns a call's arguments while its callee runs. CPython decides this at
/// the call instruction from the callee: a plain Python function, and under
/// CALL a bound method of one, gets an inlined frame that takes the arguments
/// over. Any other callee borrows them, and the instruction releases them
/// after it returns, even when that callee binds a Python frame of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ArgumentCustody {
    Frame,
    Instruction,
    /// Public C-API calls retain their inputs through dispatch and release the
    /// unpacked stack forward, independently of the source CALL instruction.
    Capi,
}

/// The CPython path that releases the arguments a call still owns. The
/// `*ByTarget` paths changed in CPython 3.14 and consult the runtime target
/// version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReleaseOrder {
    /// `initialize_locals` and `_PyEvalFramePushAndInit` failures: positional
    /// values, then keyword values, each first to last.
    Forward,
    /// _PyStack_UnpackDict_Free releases values forward, then the names tuple
    /// releases its items backward. Source CALL keeps its separate policy.
    Capi,
    /// CALL's cleanup after a callee without an inlined frame (DECREF_INPUTS),
    /// and an inlined frame's parameters (`_PyFrame_ClearLocals`): first to
    /// last through 3.13, last to first from 3.14.
    StackByTarget,
    /// `_PyEvalFramePushAndInit_Ex`: the positional tuple (last to first), then
    /// the keyword mapping (insertion order), in every version.
    TupleThenMapping,
    /// CALL_FUNCTION_EX's cleanup after a callee without an inlined frame: the
    /// tuple first through 3.13, the mapping first from 3.14.
    ContainersByTarget,
}

impl ReleaseOrder {
    /// The call instruction's own cleanup, before any frame takes custody.
    fn instruction(form: CallForm) -> Self {
        match form {
            CallForm::Stack => Self::StackByTarget,
            CallForm::Expanded => Self::ContainersByTarget,
        }
    }
}

/// How frame binding admits a call's arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Admission {
    /// A CALL's inlined frame: values move into their slots, surplus
    /// positional values end at once, and a failure releases the unbound
    /// keyword values before the partial frame (`initialize_locals`).
    Move,
    /// Every other binding: slots take new references and the call keeps its
    /// own until admission ends (CALL_FUNCTION_EX into an inlined frame) or
    /// until the callee returns (a callee without an inlined frame).
    Copy,
}

/// Public C-API spans are borrowed only through synchronous dispatch. Source
/// instructions own their vectors; receiver insertion explicitly promotes just
/// the positional span before it can mutate or acquire argument ownership.
enum PositionalArguments<'a> {
    Owned(Vec<u64>),
    Borrowed(&'a [u64]),
}
impl Default for PositionalArguments<'_> {
    fn default() -> Self {
        Self::Owned(Vec::new())
    }
}
impl std::ops::Deref for PositionalArguments<'_> {
    type Target = [u64];
    fn deref(&self) -> &[u64] {
        match self {
            Self::Owned(values) => values,
            Self::Borrowed(values) => values,
        }
    }
}
impl PositionalArguments<'_> {
    fn owned_mut(&mut self) -> &mut Vec<u64> {
        match self {
            Self::Owned(values) => values,
            Self::Borrowed(_) => unreachable!("borrowed C-API operands require explicit promotion"),
        }
    }
    fn capacity(&self) -> usize {
        match self {
            Self::Owned(values) => values.capacity(),
            Self::Borrowed(_) => 0,
        }
    }
}

/// The consuming call's arguments: CPython's value-stack operands of a CALL,
/// or the positional tuple and keyword mapping of a CALL_FUNCTION_EX. The
/// consuming entry moves the heap builder's edges here (T1). A CALL's inlined
/// frame takes them over (T2); every other callee borrows them. Whatever
/// remains is released by the CPython path that owns it at that point.
pub(crate) struct CallArguments<'a, 'py> {
    py: &'a PyToken<'py>,
    pub(super) form: CallForm,
    /// Decided once, from the call instruction's callee; redispatch keeps it.
    custody: Option<ArgumentCustody>,
    release: ReleaseOrder,
    positional: PositionalArguments<'a>,
    /// Positional values before this index have moved into a frame.
    positional_start: usize,
    keywords: CallKeywords<'a>,
}

/// Keyword custody. The builder's dictionary stays whole until a consumer
/// needs ordered entries, so extension callees still receive it directly.
enum CallKeywords<'a> {
    /// One owned reference to the keyword dictionary, or `None`.
    Mapping(u64),
    Unpacked(KeywordArguments),
    /// The hook caller pins these spans through return; Capi custody always
    /// copies into a callee frame and cannot move or release borrowed edges.
    Borrowed {
        names: &'a [u64],
        values: &'a [u64],
    },
    /// Constructor inspection pins an observation while the original mapping
    /// remains authoritative for each subsequent child call.
    ObservedMapping {
        mapping: u64,
        entries: KeywordArguments,
    },
}

/// Owned keyword entries in insertion order.
struct KeywordArguments {
    names: Vec<u64>,
    values: Vec<u64>,
    /// Values before this index have moved into a frame or its `**kwargs`.
    start: usize,
}

/// How a call's keyword arguments end.
#[derive(Clone, Copy)]
enum KeywordRelease {
    /// Value-stack entries, first to last.
    InOrder,
    Capi,
    /// Value-stack entries, last to first.
    Reversed,
    /// A mapping's teardown: each entry's name, then its value, in insertion
    /// order.
    Mapping,
}

impl CallKeywords<'_> {
    /// Keyword values this call still owns.
    fn owned_count(&self) -> usize {
        match self {
            CallKeywords::Mapping(bits) | CallKeywords::ObservedMapping { mapping: bits, .. } => {
                obj_from_bits(*bits)
                    .as_ptr()
                    .map_or(0, |dict| unsafe { dict_len(dict) })
            }
            CallKeywords::Unpacked(keywords) => keywords.values.len() - keywords.start,
            CallKeywords::Borrowed { values, .. } => values.len(),
        }
    }

    fn release(self, py: &PyToken<'_>, order: KeywordRelease) {
        match self {
            CallKeywords::Borrowed { .. } => {}
            CallKeywords::ObservedMapping { mapping, entries } => {
                // Retire the pinned observation while the live dictionary still
                // owns its current entries; removed entries follow caller order.
                CallKeywords::Unpacked(entries).release(py, order);
                CallKeywords::Mapping(mapping).release(py, order);
            }
            CallKeywords::Mapping(bits) => {
                // Stack entries end one by one. Only a mapping this call alone
                // owns can be taken apart; a shared one ends with its owners.
                let reversed = matches!(order, KeywordRelease::Reversed)
                    .then(|| {
                        let dict = obj_from_bits(bits).as_ptr()?;
                        unsafe { exclusively_owned(dict) }.then(|| unsafe {
                            crate::object::ops::dict_clear_deferred(py, dict)
                                .expect("an exclusively owned call dictionary is mutable")
                                .into_owned_storage()
                        })
                    })
                    .flatten();
                dec_ref_bits(py, bits);
                if let Some(entries) = reversed {
                    for row in entries.iter().rev().filter(|row| row.hash.is_some()) {
                        dec_ref_bits(py, row.value);
                        dec_ref_bits(py, row.key);
                    }
                }
            }
            CallKeywords::Unpacked(KeywordArguments {
                names,
                values,
                start,
            }) => match order {
                KeywordRelease::Capi => {
                    release_in_order(py, &values[start..]);
                    release_reversed(py, &names);
                }
                KeywordRelease::InOrder => {
                    release_in_order(py, &values[start..]);
                    release_in_order(py, &names);
                }
                KeywordRelease::Reversed => {
                    release_reversed(py, &values[start..]);
                    release_in_order(py, &names);
                }
                KeywordRelease::Mapping => {
                    for (index, &name) in names.iter().enumerate() {
                        dec_ref_bits(py, name);
                        if index >= start {
                            dec_ref_bits(py, values[index]);
                        }
                    }
                }
            },
        }
    }
}

fn release_in_order(py: &PyToken<'_>, values: &[u64]) {
    for &bits in values {
        dec_ref_bits(py, bits);
    }
}

fn release_reversed(py: &PyToken<'_>, values: &[u64]) {
    for &bits in values.iter().rev() {
        dec_ref_bits(py, bits);
    }
}

/// A borrowed projection of unpacked `CallArguments` for builtin, extension
/// and constructor binders. It owns nothing.
#[derive(Clone, Copy)]
pub(crate) struct CallArgumentView<'a> {
    pub(crate) pos: &'a [u64],
    pub(crate) kw_names: &'a [u64],
    pub(crate) kw_values: &'a [u64],
}

impl<'a, 'py> CallArguments<'a, 'py> {
    pub(super) fn empty(py: &'a PyToken<'py>, form: CallForm) -> Self {
        Self {
            py,
            form,
            custody: None,
            release: ReleaseOrder::instruction(form),
            positional: PositionalArguments::default(),
            positional_start: 0,
            keywords: CallKeywords::Mapping(MoltObject::none().bits()),
        }
    }

    /// T1. The consuming call holds the builder's reference and inherits its
    /// call form. When that reference is the only one, the builder's edges move
    /// here without reference traffic and the builder is left empty. A builder
    /// that another owner can still reach keeps its edges, and the call
    /// retains its own.
    pub(super) unsafe fn from_builder(
        py: &'a PyToken<'py>,
        builder_ptr: *mut u8,
    ) -> Result<Self, u64> {
        if builder_ptr.is_null() {
            return Ok(Self::empty(py, CallForm::Stack));
        }
        let builder = unsafe { &mut *require_callargs_ptr(py, builder_ptr)? };
        let mut arguments = Self::empty(py, builder.form);
        if unsafe { exclusively_owned(builder_ptr) } {
            arguments.positional = PositionalArguments::Owned(std::mem::take(&mut builder.pos));
            arguments.keywords = CallKeywords::Mapping(std::mem::replace(
                &mut builder.keywords,
                MoltObject::none().bits(),
            ));
            return Ok(arguments);
        }
        if arguments
            .positional
            .owned_mut()
            .try_reserve_exact(builder.pos.len())
            .is_err()
        {
            return Err(raise_exception::<_>(
                py,
                "MemoryError",
                "call arguments allocation failed",
            ));
        }
        ALLOC_BYTES_CALLARGS.fetch_add(
            (arguments.positional.capacity() * std::mem::size_of::<u64>()) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        for &bits in &builder.pos {
            inc_ref_bits(py, bits);
            arguments.positional.owned_mut().push(bits);
        }
        inc_ref_bits(py, builder.keywords);
        arguments.keywords = CallKeywords::Mapping(builder.keywords);
        Ok(arguments)
    }

    /// An argument vector retaining borrowed values, for runtime callers and
    /// for the constructor phases that `type.__call__` lends its arguments
    /// to. `receiver` becomes the first positional value. The lender keeps
    /// its own references past this call.
    pub(super) fn retained(
        py: &'a PyToken<'py>,
        receiver: Option<u64>,
        positional: &[u64],
        names: &[u64],
        values: &[u64],
    ) -> Result<Self, u64> {
        debug_assert_eq!(names.len(), values.len());
        let mut arguments = Self::empty(py, CallForm::Stack);
        let mut keywords = KeywordArguments {
            names: Vec::new(),
            values: Vec::new(),
            start: 0,
        };
        let receivers = usize::from(receiver.is_some());
        if arguments
            .positional
            .owned_mut()
            .try_reserve_exact(receivers + positional.len())
            .is_err()
            || keywords.names.try_reserve_exact(names.len()).is_err()
            || keywords.values.try_reserve_exact(values.len()).is_err()
        {
            return Err(raise_exception::<_>(
                py,
                "MemoryError",
                "call arguments allocation failed",
            ));
        }
        ALLOC_BYTES_CALLARGS.fetch_add(
            ((arguments.positional.capacity()
                + keywords.names.capacity()
                + keywords.values.capacity())
                * std::mem::size_of::<u64>()) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        for &bits in receiver.iter().chain(positional) {
            inc_ref_bits(py, bits);
            arguments.positional.owned_mut().push(bits);
        }
        for (&name, &value) in names.iter().zip(values) {
            inc_ref_bits(py, name);
            inc_ref_bits(py, value);
            keywords.names.push(name);
            keywords.values.push(value);
        }
        arguments.keywords = CallKeywords::Unpacked(keywords);
        Ok(arguments)
    }

    /// C-API vector ingress already supplies separate names and values. Retain
    /// it directly; only a resolved non-vector target may later request a dict.
    pub(crate) fn capi_vector(
        py: &'a PyToken<'py>,
        positional: &'a [u64],
        names: &'a [u64],
        values: &'a [u64],
    ) -> Result<Self, u64> {
        debug_assert_eq!(names.len(), values.len());
        let mut arguments = Self::empty(py, CallForm::Stack);
        arguments.positional = PositionalArguments::Borrowed(positional);
        arguments.keywords = CallKeywords::Borrowed { names, values };
        arguments.custody = Some(ArgumentCustody::Capi);
        arguments.release = ReleaseOrder::Capi;
        Ok(arguments)
    }

    /// Retain the actual dictionary-call carrier. Unpacking is deferred until
    /// the target requires a vector, without hashing or rebuilding its names.
    pub(crate) fn capi(
        py: &'a PyToken<'py>,
        receiver: Option<u64>,
        positional: &'a [u64],
        mapping: u64,
    ) -> Result<Self, u64> {
        let mut arguments = Self::empty(py, CallForm::Stack);
        arguments.positional = PositionalArguments::Borrowed(positional);
        inc_ref_bits(py, mapping);
        arguments.keywords = CallKeywords::Mapping(mapping);
        arguments.custody = Some(ArgumentCustody::Capi);
        arguments.release = ReleaseOrder::Capi;
        if let Some(receiver) = receiver {
            arguments.prepend_positional(receiver)?;
        }
        Ok(arguments)
    }

    /// Only dictionary ingress may lend its original mapping to a tuple-based
    /// C target. Source CALL and vector ingress retain their existing unpacking
    /// and construction semantics.
    pub(crate) fn capi_mapping(&self) -> Option<u64> {
        if self.custody != Some(ArgumentCustody::Capi) {
            return None;
        }
        match &self.keywords {
            CallKeywords::Mapping(bits) | CallKeywords::ObservedMapping { mapping: bits, .. } => {
                Some(*bits)
            }
            CallKeywords::Unpacked(_) | CallKeywords::Borrowed { .. } => None,
        }
    }

    /// A non-vector type call requires one mapping. Vector ingress constructs
    /// it here once; dictionary ingress retains its exact original carrier.
    pub(crate) fn prepare_constructor_mapping(&mut self) -> Result<(), u64> {
        if matches!(
            self.keywords,
            CallKeywords::Unpacked(_) | CallKeywords::Borrowed { .. }
        ) {
            let mapping = self.keyword_mapping()?;
            let old = std::mem::replace(&mut self.keywords, CallKeywords::Mapping(mapping));
            old.release(self.py, KeywordRelease::Capi);
        }
        Ok(())
    }

    /// The prepared type-call carrier remains owned by this call.
    pub(crate) fn constructor_mapping(&self) -> u64 {
        match &self.keywords {
            CallKeywords::Mapping(bits) | CallKeywords::ObservedMapping { mapping: bits, .. } => {
                *bits
            }
            _ => unreachable!("constructor mapping must be prepared"),
        }
    }

    /// Child phases borrow the existing mapping carrier, not the observation.
    /// Positional replacement supports metaclass winner/base normalization.
    pub(crate) fn constructor_child<'b>(
        &'b self,
        receiver: Option<u64>,
        positional: &'b [u64],
    ) -> Result<CallArguments<'b, 'py>, u64> {
        let mapping = match &self.keywords {
            CallKeywords::Mapping(bits) | CallKeywords::ObservedMapping { mapping: bits, .. } => {
                *bits
            }
            CallKeywords::Unpacked(_) | CallKeywords::Borrowed { .. } => {
                unreachable!("constructor mapping must be prepared before a child call")
            }
        };
        CallArguments::capi(self.py, receiver, positional, mapping)
    }

    /// Allocate an owned inspection only at a constructor branch that actually
    /// consumes names/values. Plain __new__/__init__ forwarding never needs it.
    pub(crate) unsafe fn observe_constructor_keywords(&mut self) -> Result<(), u64> {
        self.prepare_constructor_mapping()?;
        if let CallKeywords::Mapping(mapping) = self.keywords {
            if self.keyword_count() == 0 {
                return Ok(());
            }
            let entries = unsafe { self.mapping_entries(mapping, false) }?;
            self.keywords = CallKeywords::ObservedMapping { mapping, entries };
        }
        Ok(())
    }

    pub(crate) fn constructor_view(&self) -> CallArgumentView<'_> {
        if matches!(self.keywords, CallKeywords::Mapping(_)) && self.keyword_count() == 0 {
            return CallArgumentView {
                pos: self.positional(),
                kw_names: &[],
                kw_values: &[],
            };
        }
        let CallKeywords::ObservedMapping { entries, .. } = &self.keywords else {
            unreachable!("constructor inspection requires an owned observation");
        };
        CallArgumentView {
            pos: self.positional(),
            kw_names: &entries.names,
            kw_values: &entries.values,
        }
    }

    /// A call instruction's adopted positional operands, `receiver` first when
    /// the instruction adopted one. The instruction's references move here
    /// without reference traffic; if the vector cannot be allocated they end
    /// as the instruction's inputs (`release_stack_arguments`).
    pub(super) fn moved(
        py: &'a PyToken<'py>,
        receiver: Option<u64>,
        positional: &[u64],
    ) -> Result<Self, u64> {
        let mut arguments = Self::empty(py, CallForm::Stack);
        let receivers = usize::from(receiver.is_some());
        if arguments
            .positional
            .owned_mut()
            .try_reserve_exact(receivers + positional.len())
            .is_err()
        {
            release_stack_arguments(py, receiver, positional);
            return Err(raise_exception::<_>(
                py,
                "MemoryError",
                "call arguments allocation failed",
            ));
        }
        ALLOC_BYTES_CALLARGS.fetch_add(
            (arguments.positional.capacity() * std::mem::size_of::<u64>()) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        arguments.positional.owned_mut().extend(receiver);
        arguments
            .positional
            .owned_mut()
            .extend_from_slice(positional);
        Ok(arguments)
    }

    /// Record the call instruction's custody decision. Redispatch through a
    /// descriptor, `__call__`, a class or a generic alias never revisits it.
    pub(super) fn admit_custody(&mut self, custody: ArgumentCustody) {
        if self.custody.is_some() {
            return;
        }
        self.custody = Some(custody);
        self.release = match (custody, self.form) {
            (ArgumentCustody::Frame, CallForm::Stack) => ReleaseOrder::Forward,
            (ArgumentCustody::Frame, CallForm::Expanded) => ReleaseOrder::TupleThenMapping,
            (ArgumentCustody::Instruction, form) => ReleaseOrder::instruction(form),
            (ArgumentCustody::Capi, _) => ReleaseOrder::Capi,
        };
    }

    pub(super) fn custody(&self) -> ArgumentCustody {
        self.custody.unwrap_or(ArgumentCustody::Instruction)
    }

    pub(super) fn admission(&self) -> Admission {
        if self.custody() == ArgumentCustody::Frame && self.form == CallForm::Stack {
            Admission::Move
        } else {
            Admission::Copy
        }
    }

    /// An inlined frame borrows the positional vector as its exact parameters
    /// (trampoline and cached direct calls). Those references are the frame's
    /// and end in frame order after it returns.
    pub(super) fn enter_inlined_frame(&mut self) {
        if self.custody() == ArgumentCustody::Frame {
            self.release = ReleaseOrder::StackByTarget;
        }
    }

    pub(crate) fn positional(&self) -> &[u64] {
        &self.positional[self.positional_start..]
    }

    /// An adopting entry takes every remaining positional value over as its
    /// parameters; this call releases none of them afterwards.
    pub(super) fn surrender_positional(&mut self) -> &[u64] {
        debug_assert!(
            matches!(self.positional, PositionalArguments::Owned(_)),
            "C-API borrowed arguments cannot move into a frame"
        );
        let start = std::mem::replace(&mut self.positional_start, self.positional.len());
        &self.positional[start..]
    }

    /// Keyword entries this call still owns.
    pub(crate) fn keyword_count(&self) -> usize {
        self.keywords.owned_count()
    }

    /// Bound-method dispatch owns its receiver as the first positional value.
    pub(super) fn prepend_positional(&mut self, bits: u64) -> Result<(), u64> {
        debug_assert_eq!(
            self.positional_start, 0,
            "receivers bind before any transfer"
        );
        if let PositionalArguments::Borrowed(values) = &self.positional {
            let mut owned = Vec::new();
            let Some(count) = values.len().checked_add(1) else {
                return Err(raise_exception::<_>(
                    self.py,
                    "MemoryError",
                    "call arguments allocation failed",
                ));
            };
            if owned.try_reserve_exact(count).is_err() {
                return Err(raise_exception::<_>(
                    self.py,
                    "MemoryError",
                    "call arguments allocation failed",
                ));
            }
            inc_ref_bits(self.py, bits);
            owned.push(bits);
            for &value in *values {
                inc_ref_bits(self.py, value);
                owned.push(value);
            }
            ALLOC_BYTES_CALLARGS.fetch_add(
                (owned.capacity() * std::mem::size_of::<u64>()) as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            self.positional = PositionalArguments::Owned(owned);
            return Ok(());
        }
        if self.positional.owned_mut().try_reserve(1).is_err() {
            return Err(raise_exception::<_>(
                self.py,
                "MemoryError",
                "call arguments allocation failed",
            ));
        }
        inc_ref_bits(self.py, bits);
        self.positional.owned_mut().insert(0, bits);
        Ok(())
    }

    /// Keyword names are strings (subclasses included) at every call boundary.
    pub(super) fn validate_keywords(&self) -> bool {
        match &self.keywords {
            CallKeywords::Mapping(bits) | CallKeywords::ObservedMapping { mapping: bits, .. } => {
                obj_from_bits(*bits).as_ptr().is_none_or(|dict| unsafe {
                    crate::object::mapping_merge::validate_keywords(self.py, dict)
                })
            }
            CallKeywords::Borrowed { names, .. } => {
                crate::object::mapping_merge::validate_keyword_names(self.py, names.iter().copied())
            }
            CallKeywords::Unpacked(keywords) => {
                crate::object::mapping_merge::validate_keyword_names(
                    self.py,
                    keywords.names.iter().copied(),
                )
            }
        }
    }

    /// An owned keyword mapping for extension callees: the builder's own
    /// dictionary while it is whole, otherwise a fresh dictionary of the
    /// entries this call still owns. `None` when there are no keywords.
    pub(crate) fn keyword_mapping(&self) -> Result<u64, u64> {
        let (names, values, start) = match &self.keywords {
            CallKeywords::Mapping(bits) | CallKeywords::ObservedMapping { mapping: bits, .. } => {
                inc_ref_bits(self.py, *bits);
                return Ok(*bits);
            }
            CallKeywords::Unpacked(keywords) => (
                keywords.names.as_slice(),
                keywords.values.as_slice(),
                keywords.start,
            ),
            CallKeywords::Borrowed { names, values } => (*names, *values, 0),
        };
        if start == values.len() {
            return Ok(MoltObject::none().bits());
        }
        let mut pairs = Vec::new();
        if pairs.try_reserve_exact(2 * (values.len() - start)).is_err() {
            return Err(raise_exception::<_>(
                self.py,
                "MemoryError",
                "call keywords allocation failed",
            ));
        }
        for index in start..values.len() {
            pairs.push(names[index]);
            pairs.push(values[index]);
        }
        let dict = alloc_dict_with_pairs(self.py, &pairs);
        if dict.is_null() {
            // The dictionary constructor raised: MemoryError, or a keyword
            // name's hash or equality callback. That exception is the result.
            return Err(MoltObject::none().bits());
        }
        Ok(MoltObject::from_ptr(dict).bits())
    }

    /// Keyword admission, then a view of the arguments. Entries leave the
    /// builder's dictionary without reference traffic when this call holds
    /// its only reference. A dictionary another owner can reach stays intact
    /// and the call retains its entries, as `_PyStack_UnpackDict` does. Either
    /// way no later keyword callback can change what binding reads.
    unsafe fn mapping_entries(&self, bits: u64, detach: bool) -> Result<KeywordArguments, u64> {
        let mut keywords = KeywordArguments {
            names: Vec::new(),
            values: Vec::new(),
            start: 0,
        };
        if let Some(dict) = obj_from_bits(bits).as_ptr() {
            let count = unsafe { dict_len(dict) };
            if keywords.names.try_reserve_exact(count).is_err()
                || keywords.values.try_reserve_exact(count).is_err()
            {
                return Err(raise_exception::<_>(
                    self.py,
                    "MemoryError",
                    "call keywords allocation failed",
                ));
            }
            ALLOC_BYTES_CALLARGS.fetch_add(
                ((keywords.names.capacity() + keywords.values.capacity())
                    * std::mem::size_of::<u64>()) as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            if detach && unsafe { exclusively_owned(dict) } {
                // Publishing the dictionary empty is unobservable: no
                // other owner can reach it.
                let detached = unsafe { crate::object::ops::dict_clear_deferred(self.py, dict) };
                let entries = detached
                    .expect("an exclusively owned call dictionary is mutable")
                    .into_owned_storage();
                for row in entries.iter().filter(|row| row.hash.is_some()) {
                    keywords.names.push(row.key);
                    keywords.values.push(row.value);
                }
            } else {
                // No Python callback or dictionary mutation occurs here.
                for row in unsafe { dict_live_entries(dict) } {
                    inc_ref_bits(self.py, row.key);
                    inc_ref_bits(self.py, row.value);
                    keywords.names.push(row.key);
                    keywords.values.push(row.value);
                }
            }
        }
        Ok(keywords)
    }

    pub(crate) unsafe fn unpacked_view(&mut self) -> Result<CallArgumentView<'_>, u64> {
        if let CallKeywords::Mapping(bits) = self.keywords {
            let keywords = unsafe { self.mapping_entries(bits, true) }?;
            self.keywords = CallKeywords::Unpacked(keywords);
            dec_ref_bits(self.py, bits);
        } else if matches!(self.keywords, CallKeywords::ObservedMapping { .. }) {
            let CallKeywords::ObservedMapping { mapping, entries } = std::mem::replace(
                &mut self.keywords,
                CallKeywords::Mapping(MoltObject::none().bits()),
            ) else {
                unreachable!()
            };
            self.keywords = CallKeywords::Unpacked(entries);
            dec_ref_bits(self.py, mapping);
        }
        let (names, values) = match &self.keywords {
            CallKeywords::Unpacked(keywords) => (
                &keywords.names[keywords.start..],
                &keywords.values[keywords.start..],
            ),
            CallKeywords::Borrowed { names, values } => (*names, *values),
            _ => unreachable!("keywords unpacked"),
        };
        Ok(CallArgumentView {
            pos: self.positional(),
            kw_names: names,
            kw_values: values,
        })
    }

    /// T2 under `Admission::Move`: the next positional value moves into its
    /// frame slot.
    pub(super) fn take_positional(&mut self) -> u64 {
        debug_assert!(
            matches!(self.positional, PositionalArguments::Owned(_)),
            "C-API borrowed arguments cannot move into a frame"
        );
        let bits = self.positional[self.positional_start];
        self.positional_start += 1;
        bits
    }

    /// T2 under `Admission::Move`: the remaining positional values become the
    /// frame's `*args` tuple. Ownership transfers only when the tuple exists.
    pub(super) fn take_positional_tuple(&mut self) -> Option<u64> {
        debug_assert!(
            matches!(self.positional, PositionalArguments::Owned(_)),
            "C-API borrowed arguments cannot move into a frame"
        );
        let tuple = crate::object::builders::alloc_tuple_owned(self.py, self.positional());
        if tuple.is_null() {
            return None;
        }
        self.positional_start = self.positional.len();
        Some(MoltObject::from_ptr(tuple).bits())
    }

    /// `*args` under `Admission::Copy`: a tuple of new references to the
    /// positional values from `from` on. This call keeps its own.
    pub(super) fn copy_positional_tuple(&self, from: usize) -> Option<u64> {
        let tuple = alloc_tuple(self.py, &self.positional()[from..]);
        (!tuple.is_null()).then(|| MoltObject::from_ptr(tuple).bits())
    }

    /// Under `Admission::Move`, `initialize_locals` releases surplus positional
    /// values as soon as the binder finds them surplus. The arity error follows
    /// keyword binding, whose callbacks observe the release.
    pub(super) fn release_surplus_positional(&mut self) {
        while self.positional_start < self.positional.len() {
            let bits = self.positional[self.positional_start];
            self.positional_start += 1;
            dec_ref_bits(self.py, bits);
        }
    }

    /// Number of unpacked keyword entries, bound or not.
    pub(super) fn keyword_len(&self) -> usize {
        match &self.keywords {
            CallKeywords::Borrowed { values, .. } => values.len(),
            CallKeywords::Unpacked(keywords)
            | CallKeywords::ObservedMapping {
                entries: keywords, ..
            } => keywords.values.len(),
            CallKeywords::Mapping(bits) => {
                assert!(
                    obj_from_bits(*bits).is_none(),
                    "binding reads unpacked keywords"
                );
                0
            }
        }
    }

    /// Keyword entry `index` of the unpacked call, borrowed.
    pub(super) fn keyword_entry(&self, index: usize) -> (u64, u64) {
        if let CallKeywords::Borrowed { names, values } = &self.keywords {
            return (names[index], values[index]);
        }
        let CallKeywords::Unpacked(keywords) = &self.keywords else {
            unreachable!("binding reads unpacked keywords");
        };
        (keywords.names[index], keywords.values[index])
    }

    /// T2 under `Admission::Move`: keyword `index` moves out of this call.
    /// Entries move in order, so the unbound remainder stays a suffix.
    pub(super) fn take_keyword(&mut self, index: usize) -> u64 {
        let CallKeywords::Unpacked(keywords) = &mut self.keywords else {
            unreachable!("binding reads unpacked keywords");
        };
        debug_assert_eq!(index, keywords.start, "keywords move in order");
        let bits = keywords.values[keywords.start];
        keywords.start += 1;
        bits
    }

    /// Every keyword name of the call, bound or not.
    pub(super) fn keyword_names(&self) -> &[u64] {
        match &self.keywords {
            CallKeywords::Borrowed { names, .. } => names,
            CallKeywords::Unpacked(keywords)
            | CallKeywords::ObservedMapping {
                entries: keywords, ..
            } => &keywords.names,
            CallKeywords::Mapping(_) => &[],
        }
    }
}

impl Drop for CallArguments<'_, '_> {
    fn drop(&mut self) {
        let py = self.py;
        let positional = std::mem::take(&mut self.positional);
        let positional = match &positional {
            PositionalArguments::Owned(values) => &values[self.positional_start..],
            PositionalArguments::Borrowed(_) => &[],
        };
        let keywords = std::mem::replace(
            &mut self.keywords,
            CallKeywords::Mapping(MoltObject::none().bits()),
        );
        // Only two or more owners have an observable order; the target version
        // is consulted only then.
        let owned = positional.len() + keywords.owned_count();
        let from_3_14 = || owned > 1 && crate::object::ops_sys::runtime_target_at_least(py, 3, 14);
        match self.release {
            ReleaseOrder::Capi => {
                release_in_order(py, positional);
                keywords.release(py, KeywordRelease::Capi);
            }
            ReleaseOrder::StackByTarget if from_3_14() => {
                keywords.release(py, KeywordRelease::Reversed);
                release_reversed(py, positional);
            }
            ReleaseOrder::Forward | ReleaseOrder::StackByTarget => {
                release_in_order(py, positional);
                keywords.release(py, KeywordRelease::InOrder);
            }
            ReleaseOrder::ContainersByTarget if from_3_14() => {
                keywords.release(py, KeywordRelease::Mapping);
                release_reversed(py, positional);
            }
            ReleaseOrder::TupleThenMapping | ReleaseOrder::ContainersByTarget => {
                release_reversed(py, positional);
                keywords.release(py, KeywordRelease::Mapping);
            }
        }
    }
}

fn trace_callargs_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_CALLARGS").as_deref() == Ok("1"))
}

#[derive(Copy, Clone)]
struct CallArgsPtr(*mut CallArgs);

// CallArgs allocations are owned by the runtime object they are attached to
// and protected by the GIL-like runtime lock. The registry only preserves
// pointer provenance for lookups from object payload addresses.
unsafe impl Send for CallArgsPtr {}
unsafe impl Sync for CallArgsPtr {}

pub(crate) struct CallBindRuntimeState {
    callargs_builder_map: HashMap<usize, CallArgsPtr>,
    callargs_storage_registry: HashSet<usize>,
}

impl CallBindRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            callargs_builder_map: HashMap::new(),
            callargs_storage_registry: HashSet::new(),
        }
    }
}

fn call_bind_runtime_state(_py: &PyToken<'_>) -> MutexGuard<'static, CallBindRuntimeState> {
    runtime_state(_py)
        .call_bind
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn call_bind_runtime_state_if_available() -> Option<MutexGuard<'static, CallBindRuntimeState>> {
    runtime_state_for_gil().map(|state| {
        state
            .call_bind
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    })
}

pub(crate) fn note_callargs_alloc(
    _py: &PyToken<'_>,
    builder_ptr: *mut u8,
    args_ptr: *mut CallArgs,
) {
    let mut state = call_bind_runtime_state(_py);
    if !builder_ptr.is_null() {
        state
            .callargs_builder_map
            .insert(builder_ptr as usize, CallArgsPtr(args_ptr));
    }
    if args_ptr.is_null() {
        return;
    }
    state.callargs_storage_registry.insert(args_ptr as usize);
}

pub(crate) fn note_callargs_free(_py: &PyToken<'_>, builder_ptr: *mut u8, args_ptr: *mut CallArgs) {
    if trace_callargs_enabled() && !builder_ptr.is_null() {
        eprintln!(
            "[molt callargs] free builder_ptr=0x{:x} args_ptr=0x{:x}",
            builder_ptr as usize, args_ptr as usize,
        );
    }
    let mut state = call_bind_runtime_state(_py);
    if !builder_ptr.is_null() {
        state.callargs_builder_map.remove(&(builder_ptr as usize));
    }
    if args_ptr.is_null() {
        return;
    }
    state.callargs_storage_registry.remove(&(args_ptr as usize));
}

#[cfg(any(test, feature = "molt_gpu_primitives"))]
pub(crate) unsafe fn clone_callargs_builder_bits(
    _py: &PyToken<'_>,
    builder_bits: u64,
) -> Result<u64, u64> {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "invalid callargs builder",
        ));
    }
    if unsafe { object_type_id(builder_ptr) } != TYPE_ID_CALLARGS {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "invalid callargs builder",
        ));
    }
    let args_ptr = unsafe { require_callargs_ptr(_py, builder_ptr) }?;
    let args = unsafe { &*args_ptr };
    let clone_bits = callargs_new_with_form(
        _py,
        MoltObject::from_int(args.pos.len() as i64).bits(),
        MoltObject::from_int(0).bits(),
        args.form,
    );
    if clone_bits == 0 || obj_from_bits(clone_bits).is_none() {
        return Err(clone_bits);
    }
    for &value_bits in &args.pos {
        let pushed = unsafe { molt_callargs_push_pos(clone_bits, value_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, clone_bits);
            return Err(pushed);
        }
    }
    if !obj_from_bits(args.keywords).is_none() {
        let keywords = crate::molt_dict_copy(args.keywords);
        if exception_pending(_py) || obj_from_bits(keywords).is_none() {
            dec_ref_bits(_py, clone_bits);
            return Err(if exception_pending(_py) {
                keywords
            } else {
                raise_exception::<_>(_py, "MemoryError", "call keywords allocation failed")
            });
        }
        unsafe { (*callargs_ptr(ptr_from_bits(clone_bits))).keywords = keywords };
    }
    Ok(clone_bits)
}

#[allow(dead_code)]
pub(crate) unsafe fn callargs_positional_snapshot(
    _py: &PyToken<'_>,
    builder_bits: u64,
) -> Result<Vec<u64>, u64> {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "invalid callargs builder",
        ));
    }
    let args_ptr = unsafe { require_callargs_ptr(_py, builder_ptr) }?;
    let args = unsafe { &*args_ptr };
    if unsafe { args.keyword_count() } != 0 {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "gpu kernel launch does not support keyword arguments",
        ));
    }
    Ok(args.pos.clone())
}

pub(super) fn callargs_builder_is_live(_py: &PyToken<'_>, builder_ptr: *mut u8) -> bool {
    if builder_ptr.is_null() {
        return false;
    }
    call_bind_runtime_state(_py)
        .callargs_builder_map
        .contains_key(&(builder_ptr as usize))
}

fn callargs_storage_is_live(_py: &PyToken<'_>, args_ptr: *mut CallArgs) -> bool {
    if args_ptr.is_null() {
        return false;
    }
    call_bind_runtime_state(_py)
        .callargs_storage_registry
        .contains(&(args_ptr as usize))
}

pub(crate) unsafe fn callargs_ptr(ptr: *mut u8) -> *mut CallArgs {
    if ptr.is_null() {
        return std::ptr::null_mut();
    }
    let Some(state) = call_bind_runtime_state_if_available() else {
        return std::ptr::null_mut();
    };
    state
        .callargs_builder_map
        .get(&(ptr as usize))
        .copied()
        .map_or(std::ptr::null_mut(), |raw| raw.0)
}

pub(super) unsafe fn require_callargs_ptr(
    _py: &PyToken<'_>,
    builder_ptr: *mut u8,
) -> Result<*mut CallArgs, u64> {
    unsafe {
        if builder_ptr.is_null() {
            return Ok(std::ptr::null_mut());
        }
        if !callargs_builder_is_live(_py, builder_ptr) {
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] invalid_builder builder_ptr=0x{:x}",
                    builder_ptr as usize,
                );
            }
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "invalid callargs builder",
            ));
        }
        let args_ptr = callargs_ptr(builder_ptr);
        if args_ptr.is_null() || !callargs_storage_is_live(_py, args_ptr) {
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] invalid_storage builder_ptr=0x{:x} args_ptr=0x{:x}",
                    builder_ptr as usize, args_ptr as usize,
                );
            }
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "invalid callargs storage",
            ));
        }
        Ok(args_ptr)
    }
}

pub(crate) unsafe fn callargs_visit_owned(args_ptr: *mut CallArgs, mut visit: impl FnMut(u64)) {
    unsafe {
        if args_ptr.is_null() {
            return;
        }
        let args = &*args_ptr;
        for &bits in &args.pos {
            visit(bits);
        }
        visit(args.keywords);
    }
}

pub(crate) unsafe fn callargs_detach_owned(
    _py: &PyToken<'_>,
    builder_ptr: *mut u8,
    args_ptr: *mut CallArgs,
    mut detach: impl FnMut(u64),
) {
    unsafe {
        if args_ptr.is_null() {
            return;
        }
        // Registry visibility is retired before any edge can be released by
        // the caller's object-wide sink.
        note_callargs_free(_py, builder_ptr, args_ptr);
        let args = &mut *args_ptr;
        // Consuming calls take the builder's edges at entry, so a builder that
        // still holds arguments is the value stack of a failed preparation. It
        // unwinds as CPython's does: the keyword mapping sits above the
        // positional entries, which release last to first.
        detach(std::mem::replace(
            &mut args.keywords,
            MoltObject::none().bits(),
        ));
        for bits in std::mem::take(&mut args.pos).into_iter().rev() {
            detach(bits);
        }
        drop(Box::from_raw(args_ptr));
    }
}

/// The builder of a CALL call site: value-stack arguments.
#[unsafe(no_mangle)]
pub extern "C" fn molt_callargs_new(pos_capacity_bits: u64, kw_capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        callargs_new_with_form(_py, pos_capacity_bits, kw_capacity_bits, CallForm::Stack)
    })
}

/// The builder of a CALL_FUNCTION_EX call site: its storage is that call's
/// positional tuple and keyword mapping.
#[unsafe(no_mangle)]
pub extern "C" fn molt_callargs_new_expanded(pos_capacity_bits: u64, kw_capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        callargs_new_with_form(_py, pos_capacity_bits, kw_capacity_bits, CallForm::Expanded)
    })
}

fn callargs_new_with_form(
    _py: &PyToken<'_>,
    pos_capacity_bits: u64,
    kw_capacity_bits: u64,
    form: CallForm,
) -> u64 {
    if exception_pending(_py) {
        return 0;
    }
    let decode_capacity = |bits: u64| -> Option<usize> {
        let obj = MoltObject::from_bits(bits);
        if let Some(value) = obj.as_int() {
            return usize::try_from(value).ok();
        }
        if let Some(value) = obj.as_bool() {
            return Some(usize::from(value));
        }
        if obj.is_ptr() || obj.is_none() || obj.is_pending() {
            return None;
        }
        crate::provenance::abi::address(bits)
    };
    let (Some(pos_capacity), Some(kw_capacity)) = (
        decode_capacity(pos_capacity_bits),
        decode_capacity(kw_capacity_bits),
    ) else {
        raise_exception::<()>(_py, "TypeError", "callargs capacity expects an integer");
        return 0;
    };
    let mut args = Box::new(CallArgs {
        pos: Vec::new(),
        keywords: MoltObject::none().bits(),
        form,
    });
    if args.pos.try_reserve_exact(pos_capacity).is_err() {
        raise_exception::<()>(_py, "MemoryError", "call arguments allocation failed");
        return 0;
    }
    if kw_capacity != 0 {
        let dict =
            crate::object::builders::alloc_dict_with_capacity_and_pairs(_py, kw_capacity, &[]);
        if dict.is_null() {
            // The dictionary constructor already raised; keep its exception.
            return 0;
        }
        args.keywords = MoltObject::from_ptr(dict).bits();
    }
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut CallArgs>();
    let ptr = alloc_object(_py, total, TYPE_ID_CALLARGS);
    if ptr.is_null() {
        dec_ref_bits(_py, args.keywords);
        return 0;
    }
    let callargs_bytes =
        std::mem::size_of::<CallArgs>() + args.pos.capacity() * std::mem::size_of::<u64>();
    ALLOC_BYTES_CALLARGS.fetch_add(callargs_bytes as u64, std::sync::atomic::Ordering::Relaxed);
    unsafe {
        let args_ptr = Box::into_raw(args);
        note_callargs_alloc(_py, ptr, args_ptr);
        *(ptr as *mut *mut CallArgs) = args_ptr;
        if trace_callargs_enabled() {
            eprintln!(
                "[molt callargs] new builder_bits=0x{:x} builder_ptr=0x{:x} args_ptr=0x{:x} pos_cap={} kw_cap={} form={:?}",
                bits_from_ptr(ptr),
                ptr as usize,
                args_ptr as usize,
                pos_capacity,
                kw_capacity,
                form,
            );
        }
    }
    bits_from_ptr(ptr)
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new` and
/// remain owned by the caller for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_callargs_push_pos(builder_bits: u64, val: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            if !callargs_builder_is_live(_py, builder_ptr) {
                return raise_exception::<_>(_py, "TypeError", "invalid callargs builder");
            }
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] push_pos_builder builder_bits=0x{:x} builder_ptr=0x{:x} live=true",
                    builder_bits, builder_ptr as usize,
                );
            }
            let args_ptr = match require_callargs_ptr(_py, builder_ptr) {
                Ok(ptr) => ptr,
                Err(err) => return err,
            };
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] push_pos_raw builder_bits=0x{:x} builder_ptr=0x{:x} args_ptr=0x{:x} val_type={} val_bits=0x{:x}",
                    builder_bits,
                    builder_ptr as usize,
                    args_ptr as usize,
                    type_name(_py, obj_from_bits(val)),
                    val,
                );
            }
            let args = &mut *args_ptr;
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] push_pos builder_bits=0x{:x} builder_ptr=0x{:x} args_ptr=0x{:x} len_before={} val_type={} val_bits=0x{:x}",
                    builder_bits,
                    builder_ptr as usize,
                    args_ptr as usize,
                    args.pos.len(),
                    type_name(_py, obj_from_bits(val)),
                    val,
                );
            }
            // CallArgs must keep arguments alive even if the caller drops its temporaries before
            // `molt_call_bind` executes.
            if args.pos.try_reserve(1).is_err() {
                return raise_exception::<_>(
                    _py,
                    "MemoryError",
                    "call arguments allocation failed",
                );
            }
            inc_ref_bits(_py, val);
            args.pos.push(val);
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] push_pos_done builder_bits=0x{:x} len_after={}",
                    builder_bits,
                    args.pos.len(),
                );
            }
            MoltObject::none().bits()
        })
    }
}

/// Keywords have one owned dictionary authority throughout expansion.
unsafe fn callargs_keyword_dict(_py: &PyToken<'_>, args_ptr: *mut CallArgs) -> Option<*mut u8> {
    unsafe {
        if obj_from_bits((*args_ptr).keywords).is_none() {
            let ptr = alloc_dict_with_pairs(_py, &[]);
            if ptr.is_null() {
                return None;
            }
            (*args_ptr).keywords = MoltObject::from_ptr(ptr).bits();
        }
        obj_from_bits((*args_ptr).keywords).as_ptr()
    }
}

unsafe fn callargs_push_kw(
    _py: &PyToken<'_>,
    builder_ptr: *mut u8,
    name_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        let args_ptr = match require_callargs_ptr(_py, builder_ptr) {
            Ok(ptr) => ptr,
            Err(err) => return err,
        };
        let Some(dict) = callargs_keyword_dict(_py, args_ptr) else {
            return MoltObject::none().bits();
        };
        if !crate::object::mapping_merge::keyword_available(_py, dict, name_bits, None)
            || !crate::object::mapping_merge::insert_dict(_py, dict, name_bits, val_bits, None)
        {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    }
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
/// `name_bits` must reference a Molt string object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_callargs_push_kw(
    builder_bits: u64,
    name_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            if !callargs_builder_is_live(_py, builder_ptr) {
                return raise_exception::<_>(_py, "TypeError", "invalid callargs builder");
            }
            callargs_push_kw(_py, builder_ptr, name_bits, val_bits)
        })
    }
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_callargs_expand_star(builder_bits: u64, iterable_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            if !callargs_builder_is_live(_py, builder_ptr) {
                return raise_exception::<_>(_py, "TypeError", "invalid callargs builder");
            }
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] expand_star_builder builder_bits=0x{:x} builder_ptr=0x{:x} live=true",
                    builder_bits, builder_ptr as usize,
                );
            }
            let args_ptr = match require_callargs_ptr(_py, builder_ptr) {
                Ok(ptr) => ptr,
                Err(err) => return err,
            };
            if trace_callargs_enabled() {
                let len = if args_ptr.is_null() {
                    0
                } else {
                    (&*args_ptr).pos.len()
                };
                eprintln!(
                    "[molt callargs] expand_star builder_bits=0x{:x} builder_ptr=0x{:x} args_ptr=0x{:x} len_before={} iterable_type={} iterable_bits=0x{:x}",
                    builder_bits,
                    builder_ptr as usize,
                    args_ptr as usize,
                    len,
                    type_name(_py, obj_from_bits(iterable_bits)),
                    iterable_bits,
                );
            }
            let Some(mut iter) = crate::object::iterable::OwnedIterator::new(_py, iterable_bits)
            else {
                return MoltObject::none().bits();
            };
            let Some(hint) = crate::object::iterable::length_hint(_py, iterable_bits) else {
                return MoltObject::none().bits();
            };
            if (*args_ptr).pos.try_reserve(hint).is_err() {
                return raise_exception::<_>(
                    _py,
                    "MemoryError",
                    "call arguments allocation failed",
                );
            }
            loop {
                match iter.next() {
                    Ok(Some(item)) => {
                        if (*args_ptr).pos.try_reserve(1).is_err() {
                            dec_ref_bits(_py, item);
                            return raise_exception::<_>(
                                _py,
                                "MemoryError",
                                "call arguments allocation failed",
                            );
                        }
                        (*args_ptr).pos.push(item);
                    }
                    Ok(None) => break,
                    Err(molt_runtime_core::ErrorIndicatorSet) => return MoltObject::none().bits(),
                }
            }
            MoltObject::none().bits()
        })
    }
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_callargs_expand_kwstar(builder_bits: u64, mapping_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            if !callargs_builder_is_live(_py, builder_ptr) {
                return raise_exception::<_>(_py, "TypeError", "invalid callargs builder");
            }
            let args = match require_callargs_ptr(_py, builder_ptr) {
                Ok(args) => args,
                Err(err) => return err,
            };
            let Some(dict) = callargs_keyword_dict(_py, args) else {
                return MoltObject::none().bits();
            };
            crate::object::mapping_merge::merge_keywords(_py, dict, mapping_bits);
            MoltObject::none().bits()
        })
    }
}

/// The call instruction's custody decision for `call_bits`. CPython inlines a
/// frame only for a plain Python function (`PyFunction_Type`), and CALL also
/// expands a bound method of one. The native-function class family
/// (`builtin_function_or_method`, C-API and extension callables) and every
/// other callable borrow the arguments instead.
pub(super) unsafe fn callee_custody(
    py: &PyToken<'_>,
    call_bits: u64,
    form: CallForm,
) -> ArgumentCustody {
    let python_function = |bits: u64| {
        obj_from_bits(bits).as_ptr().is_some_and(|ptr| unsafe {
            object_type_id(ptr) == TYPE_ID_FUNCTION
                && !builtin_classes(py).is_native_callable_class(object_class_bits(ptr))
        })
    };
    let inlined =
        obj_from_bits(call_bits)
            .as_ptr()
            .is_some_and(|ptr| match unsafe { object_type_id(ptr) } {
                TYPE_ID_FUNCTION => python_function(call_bits),
                TYPE_ID_BOUND_METHOD => {
                    form == CallForm::Stack
                        && python_function(unsafe { bound_method_func_bits(ptr) })
                }
                _ => false,
            });
    if inlined {
        ArgumentCustody::Frame
    } else {
        ArgumentCustody::Instruction
    }
}

/// Argument words a runtime call entry reads through `(args_ptr, nargs)`,
/// copied out on entry. The WASM lane spills them into one shared region that
/// every reentrant compiled call reuses, so no entry may read that region once
/// Python code can run.
pub(crate) struct EntryArguments {
    inline: [u64; ENTRY_ARGUMENTS_INLINE],
    len: usize,
    heap: Vec<u64>,
}

const ENTRY_ARGUMENTS_INLINE: usize = 16;

impl EntryArguments {
    /// Copy `args_ptr_bits[..nargs]`. `None`: the range lies outside the
    /// active target.
    ///
    /// # Safety
    /// A nonzero `nargs` requires `args_ptr_bits` to address that many
    /// initialized words, as every compiled call site's spill does.
    pub(crate) unsafe fn copy(args_ptr_bits: u64, nargs: u64) -> Option<Self> {
        let args_ptr = crate::provenance::abi::const_ptr::<u64>(args_ptr_bits)?;
        let raw = unsafe { crate::provenance::abi::slice(args_ptr, nargs) }?;
        let mut arguments = Self {
            inline: [0; ENTRY_ARGUMENTS_INLINE],
            len: raw.len(),
            heap: Vec::new(),
        };
        if raw.len() <= ENTRY_ARGUMENTS_INLINE {
            arguments.inline[..raw.len()].copy_from_slice(raw);
        } else {
            arguments.heap = raw.to_vec();
        }
        Some(arguments)
    }

    pub(crate) fn as_slice(&self) -> &[u64] {
        if self.len <= ENTRY_ARGUMENTS_INLINE {
            &self.inline[..self.len]
        } else {
            &self.heap
        }
    }
}

/// A call instruction's cleanup of the value-stack references it still owns
/// when no frame took them over: after a callee without an inlined frame
/// returns, or when no callee ran at all (`DECREF_INPUTS`). First to last
/// through 3.13, last to first from 3.14; `receiver` is the bottom value.
pub(crate) fn release_stack_arguments(py: &PyToken<'_>, receiver: Option<u64>, args: &[u64]) {
    let owned = usize::from(receiver.is_some()) + args.len();
    if owned > 1 && crate::object::ops_sys::runtime_target_at_least(py, 3, 14) {
        release_reversed(py, args);
        if let Some(receiver) = receiver {
            dec_ref_bits(py, receiver);
        }
    } else {
        if let Some(receiver) = receiver {
            dec_ref_bits(py, receiver);
        }
        release_in_order(py, args);
    }
}

#[cfg(test)]
#[path = "arguments_binding_tests.rs"]
mod tests;
