//! Native callable snapshots and Argument Clinic keyword binding.
//! One admission core owns count, required slots, duplicate precedence and
//! unexpected-key diagnostics. Vector and dictionary lookup keep their distinct
//! CPython callback contracts; both retain the original keyword objects.

use crate::object::ops_compare::{CompareBoolOutcome, compare_object_eq_bool};
use crate::object::ops_format::with_string_bytes;
use crate::object::seq_access::{PinnedSequenceSnapshot, PinnedTuple};
use crate::*;

#[derive(Clone, Copy)]
pub(crate) struct NativeKeywords<'a> {
    names: &'a [u64],
    values: &'a [u64],
    stride: usize,
    dictionary: Option<u64>,
}

impl<'a> NativeKeywords<'a> {
    pub(crate) fn empty() -> Self {
        Self::vector(&[], &[])
    }

    pub(crate) fn vector(names: &'a [u64], values: &'a [u64]) -> Self {
        Self {
            names,
            values,
            stride: 1,
            dictionary: None,
        }
    }

    pub(crate) fn mapping(dictionary: u64, names: &'a [u64]) -> Self {
        Self {
            names,
            values: &[],
            stride: 1,
            dictionary: Some(dictionary),
        }
    }

    fn entries(entries: &'a [u64], dictionary: Option<u64>) -> Self {
        Self {
            names: entries,
            values: entries.get(1..).unwrap_or(&[]),
            stride: 2,
            dictionary,
        }
    }

    fn names(self) -> impl ExactSizeIterator<Item = u64> + 'a {
        self.names.iter().step_by(self.stride).copied()
    }

    pub(crate) fn is_empty(self) -> bool {
        self.names.is_empty()
    }

    fn lookup(self, py: &PyToken<'_>, parameter: u64) -> Option<Option<u64>> {
        let value = match self.dictionary {
            None => {
                // Clinic find_keyword checks all identities before Unicode
                // contents; str-subclass __eq__ is not parameter matching.
                if let Some(index) = self.names().position(|name| name == parameter) {
                    self.values.get(index * self.stride).copied()
                } else {
                    // The call and metadata owners retain both immutable
                    // strings; these readers cannot invoke Python callbacks.
                    unsafe {
                        with_string_bytes(obj_from_bits(parameter), |expected| {
                            self.names()
                                .position(|name| {
                                    with_string_bytes(obj_from_bits(name), |actual| {
                                        actual == expected
                                    })
                                    .unwrap_or(false)
                                })
                                .and_then(|index| self.values.get(index * self.stride).copied())
                        })?
                    }
                }
            }
            Some(dictionary) => {
                let ptr = obj_from_bits(dictionary).as_ptr()?;
                unsafe { dict_get_in_place(py, ptr, parameter) }
            }
        };
        if exception_pending(py) {
            None
        } else {
            Some(value)
        }
    }
}

/// Pins admitted values through later keyword callbacks and the native body.
/// A dictionary callback can retire an earlier match or insert a new value;
/// the original call snapshot alone cannot own those admitted edges.
pub(crate) struct NamedBinding<T> {
    slots: T,
    owners: Vec<PtrDropGuard>,
}

impl<T> std::ops::Deref for NamedBinding<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.slots
    }
}

impl<T> NamedBinding<T> {
    pub(crate) fn into_parts(self) -> (T, Vec<PtrDropGuard>) {
        (self.slots, self.owners)
    }
}

pub(crate) struct NativeArguments<'a, 'py> {
    pub(crate) positional: PinnedTuple<'a, 'py>,
    keyword_entries: PinnedSequenceSnapshot<'a, 'py>,
    dictionary: u64,
    _dictionary_owner: PtrDropGuard,
}

impl<'a, 'py> NativeArguments<'a, 'py> {
    pub(crate) fn read(py: &'a PyToken<'py>, label: &str, args: u64, kwargs: u64) -> Option<Self> {
        let Some(args_ptr) = obj_from_bits(args)
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE)
        else {
            return raise_exception(
                py,
                "TypeError",
                &format!("{label}() expects positional arguments tuple"),
            );
        };
        // Published tuples are immutable: retaining the tuple also retains
        // every positional edge and excludes checked C-API/iterator mutation.
        let positional = unsafe { crate::object::seq_access::pin_tuple(py, args_ptr)? };
        let Some(kwargs_ptr) = obj_from_bits(kwargs)
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) } == TYPE_ID_DICT)
        else {
            return raise_exception(
                py,
                "TypeError",
                &format!("{label}() expects keyword arguments dict"),
            );
        };
        inc_ref_bits(py, kwargs);
        let dictionary_owner = PtrDropGuard::preserving(kwargs_ptr);
        use crate::object::ops_dict::{DictSnapshotKind, dict_snapshot};
        // One retained, resource-accounted dictionary observation supplies both
        // keys and values; no projection or independent epoch can split them.
        let keyword_entries = unsafe { dict_snapshot(py, kwargs_ptr, DictSnapshotKind::Entries)? };
        for &key in keyword_entries.iter().step_by(2) {
            if unsafe { with_string_bytes(obj_from_bits(key), |_| ()) }.is_none() {
                return raise_exception(py, "TypeError", "keywords must be strings");
            }
        }
        Some(Self {
            positional,
            keyword_entries,
            dictionary: kwargs,
            _dictionary_owner: dictionary_owner,
        })
    }

    pub(crate) fn keyword_view(&self) -> NativeKeywords<'_> {
        NativeKeywords::entries(&self.keyword_entries, Some(self.dictionary))
    }

    /// A native FASTCALL method can use tuple/dict transport in Molt while its
    /// parser still has the vectorcall Unicode-content matching contract.
    pub(crate) fn vector_keyword_view(&self) -> NativeKeywords<'_> {
        NativeKeywords::entries(&self.keyword_entries, None)
    }

    pub(crate) fn keyword_pairs(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.keyword_entries
            .chunks_exact(2)
            .map(|pair| (pair[0], pair[1]))
    }

    pub(crate) fn receiver(&self, py: &PyToken<'_>) -> Option<u64> {
        self.positional.first().copied().or_else(|| {
            raise_exception::<()>(py, "TypeError", "constructor requires a receiver");
            None
        })
    }

    pub(crate) fn new_receiver(&self, py: &PyToken<'_>, base: u64) -> Option<u64> {
        crate::builtins::type_ops::native_constructor_receiver(
            py,
            base,
            self.positional.first().copied(),
            &class_name_for_error(base),
        )
        .map(|(class, _)| class)
    }

    pub(crate) fn values(&self) -> &[u64] {
        &self.positional[1..]
    }

    pub(crate) fn named<const N: usize>(
        &self,
        py: &PyToken<'_>,
        label: &str,
        names: [&str; N],
        required: usize,
    ) -> Option<NamedBinding<[Option<u64>; N]>> {
        bind_named(
            py,
            label,
            self.values(),
            self.keyword_view(),
            names,
            required,
        )
    }

    pub(crate) fn positional_only(
        &self,
        py: &PyToken<'_>,
        label: &str,
        allow_keywords: bool,
    ) -> Option<u64> {
        if !allow_keywords && !self.keyword_entries.is_empty() {
            return raise_exception(
                py,
                "TypeError",
                &format!("{label}() takes no keyword arguments"),
            );
        }
        if self.values().len() > 1 {
            return raise_exception(
                py,
                "TypeError",
                &format!(
                    "{label} expected at most 1 argument, got {}",
                    self.values().len(),
                ),
            );
        }
        Some(
            self.values()
                .first()
                .copied()
                .unwrap_or_else(|| missing_bits(py)),
        )
    }
}

/// Constructor declarations supply names here; published callable declarations
/// supply their already-owned typed parameter tuple directly to bind_named_slots.
pub(crate) fn bind_named<const N: usize>(
    py: &PyToken<'_>,
    label: &str,
    values: &[u64],
    keywords: NativeKeywords<'_>,
    names: [&str; N],
    required: usize,
) -> Option<NamedBinding<[Option<u64>; N]>> {
    let mut parameter_bits = [0; N];
    let mut owners = Vec::new();
    if owners.try_reserve_exact(N).is_err() {
        return raise_exception(
            py,
            "MemoryError",
            "native parameter custody allocation failed",
        );
    }
    for (slot, name) in parameter_bits.iter_mut().zip(names) {
        let bits = attr_name_bits_from_bytes(py, name.as_bytes())?;
        *slot = bits;
        if let Some(ptr) = obj_from_bits(bits).as_ptr() {
            owners.push(PtrDropGuard::preserving(ptr));
        }
    }
    bind_named_slots(
        py,
        label,
        values,
        keywords,
        &parameter_bits,
        required,
        [None; N],
    )
}

pub(crate) fn bind_named_slots<T: AsRef<[Option<u64>]> + AsMut<[Option<u64>]>>(
    py: &PyToken<'_>,
    label: &str,
    values: &[u64],
    keywords: NativeKeywords<'_>,
    names: &[u64],
    required: usize,
    slots: T,
) -> Option<NamedBinding<T>> {
    let count = names.len();
    if slots.as_ref().len() != count || required > count {
        return raise_exception(py, "SystemError", "invalid native parameter metadata");
    }
    let given = values.len() + keywords.names().len();
    if given > count {
        return raise_exception(
            py,
            "TypeError",
            &format!(
                "{label}() takes at most {count} {}argument{} ({given} given)",
                if values.is_empty() { "keyword " } else { "" },
                if count == 1 { "" } else { "s" },
            ),
        );
    }
    let mut bound = NamedBinding {
        slots,
        owners: Vec::new(),
    };
    if bound.owners.try_reserve_exact(count).is_err() {
        return raise_exception(
            py,
            "MemoryError",
            "native binding custody allocation failed",
        );
    }
    let mut remaining = keywords.names().len();
    for (index, &parameter) in names.iter().enumerate() {
        let positional_only =
            unsafe { with_string_bytes(obj_from_bits(parameter), <[u8]>::is_empty) }?;
        let value = if let Some(&value) = values.get(index) {
            Some(value)
        } else if remaining != 0 && !positional_only {
            let value = keywords.lookup(py, parameter)?;
            if value.is_some() {
                remaining -= 1;
            }
            value
        } else {
            None
        };
        if let Some(bits) = value {
            if let Some(ptr) = obj_from_bits(bits).as_ptr() {
                inc_ref_bits(py, bits);
                bound.owners.push(PtrDropGuard::preserving(ptr));
            }
            bound.slots.as_mut()[index] = Some(bits);
        } else if index < required {
            let name = string_obj_to_owned(obj_from_bits(parameter))?;
            return raise_exception(
                py,
                "TypeError",
                &format!(
                    "{label}() missing required argument '{name}' (pos {})",
                    index + 1,
                ),
            );
        }
    }
    if remaining == 0 {
        return Some(bound);
    }
    // Required matching finishes before duplicate occupied positional slots;
    // duplicate detection finishes before unexpected keyword membership.
    for (index, &parameter) in names.iter().take(values.len()).enumerate() {
        let positional_only =
            unsafe { with_string_bytes(obj_from_bits(parameter), <[u8]>::is_empty) }?;
        if !positional_only && keywords.lookup(py, parameter)?.is_some() {
            let name = string_obj_to_owned(obj_from_bits(parameter))?;
            return raise_exception(
                py,
                "TypeError",
                &format!(
                    "argument for {label}() given by name ('{name}') and position ({})",
                    index + 1,
                ),
            );
        }
    }
    unexpected_keyword(py, label, keywords.names(), names)?;
    None
}

fn unexpected_keyword(
    py: &PyToken<'_>,
    label: &str,
    keywords: impl Iterator<Item = u64>,
    parameters: &[u64],
) -> Option<()> {
    for keyword in keywords {
        if unsafe { with_string_bytes(obj_from_bits(keyword), |_| ()) }.is_none() {
            return raise_exception(py, "TypeError", "keywords must be strings");
        }
        let mut matched = false;
        for &parameter in parameters {
            if unsafe { with_string_bytes(obj_from_bits(parameter), <[u8]>::is_empty) }
                == Some(true)
            {
                continue;
            }
            match compare_object_eq_bool(py, obj_from_bits(parameter), obj_from_bits(keyword)) {
                CompareBoolOutcome::True => {
                    matched = true;
                    break;
                }
                CompareBoolOutcome::False => {}
                _ => return None,
            }
        }
        if matched {
            continue;
        }
        // %S renders the original key only after membership and suggestion
        // selection. Its __str__ may raise or mutate the builtin namespace.
        let suggestions_enabled = crate::object::ops_sys::runtime_target_at_least(py, 3, 13);
        let suggestion = suggestions_enabled
            .then(|| {
                // CPython's suggestion engine requires strict UTF-8. If encoding
                // fails, getargs falls through to PyErr_Format, which clears that
                // error before rendering the original key with %S. Do not feed a
                // lossy replacement or WTF-8 bytes to the edit-distance authority.
                unsafe {
                    with_string_bytes(obj_from_bits(keyword), |bytes| {
                        let source = std::str::from_utf8(bytes).ok()?;
                        let candidates: Option<Vec<String>> = parameters
                            .iter()
                            .filter_map(|&bits| {
                                with_string_bytes(obj_from_bits(bits), |bytes| {
                                    (!bytes.is_empty())
                                        .then(|| std::str::from_utf8(bytes).map(str::to_owned).ok())
                                })
                                .flatten()
                            })
                            .collect();
                        let candidates = candidates?;
                        let candidates: Vec<&str> = candidates.iter().map(String::as_str).collect();
                        crate::builtins::diagnostic_suggestions::calculate_suggestion(
                            source,
                            &candidates,
                        )
                        .map(str::to_owned)
                    })
                    .flatten()
                }
            })
            .flatten();
        let (prefix, suffix) = if !suggestions_enabled {
            (
                "'".to_owned(),
                format!("' is an invalid keyword argument for {label}()"),
            )
        } else if let Some(suggestion) = suggestion {
            (
                format!("{label}() got an unexpected keyword argument '"),
                format!("'. Did you mean '{suggestion}'?"),
            )
        } else {
            (
                format!("{label}() got an unexpected keyword argument '"),
                "'".to_owned(),
            )
        };
        use crate::object::ops_format::{FormatError, format_obj_str_output};
        let result: Result<Vec<u8>, FormatError> =
            format_obj_str_output(py, obj_from_bits(keyword)).with_bytes(py, |display| {
                let mut message = Vec::new();
                let capacity = prefix
                    .len()
                    .checked_add(display.len())
                    .and_then(|size| size.checked_add(suffix.len()));
                if capacity.is_none() || message.try_reserve_exact(capacity.unwrap_or(0)).is_err() {
                    return Err(FormatError::Diagnostic(
                        "MemoryError",
                        "keyword diagnostic allocation failed".into(),
                    ));
                }
                message.extend_from_slice(prefix.as_bytes());
                message.extend_from_slice(display);
                message.extend_from_slice(suffix.as_bytes());
                // PyUnicode_FromFormat retires the successful %S result before
                // PyErr_Format publishes TypeError. Allocation/callback errors
                // still use FormatOutput's pending-error-preserving cleanup.
                Ok(message)
            });
        return match result {
            Err(error) => error.raise(py),
            Ok(message) => {
                crate::builtins::exceptions::raise_exception_bytes(py, "TypeError", &message)
            }
        };
    }
    raise_exception(
        py,
        "TypeError",
        &format!("invalid keyword argument for {label}()"),
    )
}
