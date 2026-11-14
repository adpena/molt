//! One builder-parametric generator for a compiled method call site
//! (call_anatomy.md 4.5 "Methods", invariant J3, stage 4): CPython's
//! `LOAD_ATTR_METHOD_WITH_VALUES` and `CALL_PY_EXACT_ARGS` /
//! `CALL_PY_WITH_DEFAULTS`, guarded by a per-site record in writable data.
//!
//! ```text
//!   load:  receiver is an object; its header shape and class word equal the
//!          record's; the class's layout version equals the record's; the
//!          instance has no attribute dictionary (or the class has none)
//!            hit  -> [record.function, receiver], both retained
//!            miss -> molt_method_site_load(record, receiver, self_out)
//!   call:  self is not NULL; callable is record.function; record.entry is
//!          set; the call supplies arity - k words for some k <= the
//!          record's usable trailing defaults; the recursion limit admits it
//!            hit  -> entry(context, self, args.., defaults[k]) with the
//!                    callable handed to the callee's frame entry
//!            miss -> molt_call_loaded_method<N>(callable, self, args..)
//! ```
//!
//! The runtime fills the record on a load miss and owns every edge in it
//! (molt-runtime `call/bind/method_site.rs`). The record is the site's typed
//! handle: it carries the attribute name, which the runtime interns on the
//! site's first miss and keeps in the record, so no path passes name bytes
//! and the hit path reads no name. A backend spells instructions only
//! (`MethodSiteBuilder`) and emits the record image; the record layout, the
//! guard order and the custody rules live here once.

use crate::boxed_word::{BoxTag, generate_has_tag};
use crate::frame_record::FrameBuilder;
use crate::word::WordComparison;
use crate::{
    HEADER_AUX_KIND_CLASS_INLINE, HEADER_AUX_KIND_OFFSET, HEADER_AUX_OFFSET,
    HEADER_CLASS_WORD_BITS_MASK, POINTER_MASK, QNAN, TAG_NONE,
    THREAD_RECORD_PENDING_INVOCATION_OFFSET,
};

/// Words in a site record before its name bytes. Every word is 8-byte
/// aligned.
pub const METHOD_SITE_RECORD_WORDS: usize = 13;
pub const METHOD_SITE_RECORD_BYTES: usize = METHOD_SITE_RECORD_WORDS * 8;
/// The receiver's class: the masked class word of its header, a boxed class
/// object the record owns a reference to. `METHOD_SITE_EMPTY_CLASS` when the
/// record is empty: its low bit is set, so it equals no masked class word.
pub const METHOD_SITE_CLASS_OFFSET: i32 = 0;
/// The receiver header's 32-bit word at `METHOD_SITE_HEADER_SHAPE_OFFSET`
/// (size class, then aux kind): the auxiliary word holds the class inline,
/// and every instance of the class has the same allocation size.
pub const METHOD_SITE_SHAPE_OFFSET: i32 = 8;
/// The class's layout version when the record was filled. The runtime bumps
/// it on every mutation of the class or of a class in its MRO (HF-812).
pub const METHOD_SITE_VERSION_OFFSET: i32 = 16;
/// The byte offset of the instance-dictionary word from the object address,
/// or `METHOD_SITE_NO_DICT` when instances of the class have none.
pub const METHOD_SITE_DICT_OFFSET: i32 = 24;
/// The plain function the class resolves the name to (owned).
pub const METHOD_SITE_FUNCTION_OFFSET: i32 = 32;
/// The function's fixed-arity entry, `entry(context, args..)`: a code address
/// on native targets, a table index on wasm32. Zero: the call takes the
/// runtime.
pub const METHOD_SITE_ENTRY_OFFSET: i32 = 40;
/// The entry's positional arity.
pub const METHOD_SITE_ARITY_OFFSET: i32 = 48;
/// How many of the function's trailing positional defaults the record holds,
/// at most `METHOD_SITE_MAX_DEFAULTS`.
pub const METHOD_SITE_DEFAULT_COUNT_OFFSET: i32 = 56;
/// The function's last `METHOD_SITE_MAX_DEFAULTS` positional defaults, right
/// aligned: the last default is the last word. Each is owned.
pub const METHOD_SITE_DEFAULTS_OFFSET: i32 = 64;
/// Nonzero once the runtime has registered the record for teardown and
/// function-mutation invalidation.
pub const METHOD_SITE_REGISTERED_OFFSET: i32 = 80;
/// The interned attribute name (owned), or zero until the site's first miss.
pub const METHOD_SITE_NAME_OFFSET: i32 = 88;
/// The byte length of the attribute name; its UTF-8 bytes follow the record
/// words. Both are fixed when the program is built.
pub const METHOD_SITE_NAME_LEN_OFFSET: i32 = 96;
pub const METHOD_SITE_NAME_BYTES_OFFSET: i32 = 104;

pub const METHOD_SITE_EMPTY_CLASS: u64 = 1;
pub const METHOD_SITE_NO_DICT: u64 = u64::MAX;
pub const METHOD_SITE_MAX_DEFAULTS: usize = 2;

/// The 32-bit header word the shape test reads: `size_class` (u16) then
/// `aux_kind` (u16), little-endian on every target.
pub const METHOD_SITE_HEADER_SHAPE_OFFSET: i32 = HEADER_AUX_KIND_OFFSET - 2;
/// The shape word of an object whose auxiliary word holds its class inline.
pub const fn method_site_header_shape(size_class: u16) -> u64 {
    ((HEADER_AUX_KIND_CLASS_INLINE as u64) << 16) | size_class as u64
}

/// A class object's layout version word (molt-runtime `object/layout.rs`
/// `class_layout_version_bits` asserts it).
pub const CLASS_LAYOUT_VERSION_OFFSET: i32 = 32;

/// The size of the record of a site whose attribute name is `name_len`
/// bytes: the words, then the name, padded to a whole word.
pub const fn method_site_record_size(name_len: usize) -> usize {
    METHOD_SITE_RECORD_BYTES + name_len.div_ceil(8) * 8
}

/// Write the initial record of a site that loads `name` into `image`, which
/// is `method_site_record_size(name.len())` bytes: an empty class, the name,
/// everything else zero. Little-endian, as every target is.
pub fn write_method_site_record_image(name: &[u8], image: &mut [u8]) {
    assert_eq!(
        image.len(),
        method_site_record_size(name.len()),
        "record size"
    );
    image.fill(0);
    let word = |image: &mut [u8], offset: i32, value: u64| {
        let at = offset as usize;
        image[at..at + 8].copy_from_slice(&value.to_le_bytes());
    };
    word(image, METHOD_SITE_CLASS_OFFSET, METHOD_SITE_EMPTY_CLASS);
    word(image, METHOD_SITE_NAME_LEN_OFFSET, name.len() as u64);
    let at = METHOD_SITE_NAME_BYTES_OFFSET as usize;
    image[at..at + name.len()].copy_from_slice(name);
}

const _: () = {
    assert!(METHOD_SITE_NAME_BYTES_OFFSET as usize == METHOD_SITE_RECORD_BYTES);
    assert!(METHOD_SITE_REGISTERED_OFFSET + 8 == METHOD_SITE_NAME_OFFSET);
    assert!(
        METHOD_SITE_DEFAULTS_OFFSET as usize + METHOD_SITE_MAX_DEFAULTS * 8
            == METHOD_SITE_REGISTERED_OFFSET as usize
    );
    assert!(METHOD_SITE_NAME_OFFSET + 8 == METHOD_SITE_NAME_LEN_OFFSET);
    assert!(METHOD_SITE_NAME_LEN_OFFSET + 8 == METHOD_SITE_NAME_BYTES_OFFSET);
    assert!(METHOD_SITE_EMPTY_CLASS & !HEADER_CLASS_WORD_BITS_MASK != 0);
};

/// Backend spelling for a method site beyond the frame primitives. Every
/// `FrameBuilder` operation keeps its meaning; `enter_slow` and `exit_slow`
/// are not used.
pub trait MethodSiteBuilder: FrameBuilder {
    /// `molt_recursion_enter_fast()`: nonzero when the recursion limit admits
    /// one more Python frame, which it then counts; zero counts nothing.
    fn recursion_enter(&mut self) -> Self::Word;
    /// `molt_recursion_exit_fast()`.
    fn recursion_exit(&mut self);
    /// Call the fixed-arity compiled entry `entry` with the execution
    /// context, then `self_word`, `args` and `defaults` in that order, each a
    /// boxed word; the result is the entry's owned word.
    fn call_entry(
        &mut self,
        entry: Self::Word,
        self_word: Self::Word,
        args: &[Self::Word],
        defaults: &[Self::Word],
    ) -> Self::Word;
}

/// The callable of a method load at the site whose record is at `record`;
/// the `self_or_null` word is stored at `self_out`. Both words are owned.
///
/// Requires: `receiver` is borrowed and live; `self_out` is writable.
/// `cold` is the site's runtime miss, which writes `self_out` and returns
/// the callable.
pub fn generate_method_site_load<B: MethodSiteBuilder>(
    b: &mut B,
    record: B::Word,
    receiver: B::Word,
    self_out: B::Word,
    cold: impl FnOnce(&mut B) -> B::Word,
) -> B::Word {
    use WordComparison::{Equal, NotEqual};
    let callable = b.local();
    b.attempt(
        |b| {
            let is_object = generate_has_tag(b, receiver, BoxTag::Ptr);
            b.require(is_object);
            let object = b.and(receiver, POINTER_MASK);
            let shape = b.load_header_u32(object, METHOD_SITE_HEADER_SHAPE_OFFSET);
            let expected_shape = b.load_word(record, METHOD_SITE_SHAPE_OFFSET);
            let same_shape = b.compare(Equal, shape, expected_shape);
            b.require(same_shape);
            let class_word = b.load_word(object, HEADER_AUX_OFFSET);
            let class = b.and(class_word, HEADER_CLASS_WORD_BITS_MASK);
            let expected_class = b.load_word(record, METHOD_SITE_CLASS_OFFSET);
            let same_class = b.compare(Equal, class, expected_class);
            b.require(same_class);
            let class_object = b.and(class, POINTER_MASK);
            let version = b.load_word(class_object, CLASS_LAYOUT_VERSION_OFFSET);
            let expected_version = b.load_word(record, METHOD_SITE_VERSION_OFFSET);
            let same_version = b.compare(Equal, version, expected_version);
            b.require(same_version);
            // An instance dictionary may shadow the method. Its word holds
            // zero, None or a dictionary; only a dictionary sets a bit
            // outside None's.
            let dict_offset = b.load_word(record, METHOD_SITE_DICT_OFFSET);
            let no_dict = b.constant(METHOD_SITE_NO_DICT);
            let has_dict = b.compare(NotEqual, dict_offset, no_dict);
            b.if_then(has_dict, |b| {
                let at = b.add(object, dict_offset);
                let dict = b.load_word(at, 0);
                let present = b.and(dict, !(QNAN | TAG_NONE));
                let zero = b.constant(0);
                let absent = b.compare(Equal, present, zero);
                b.require(absent);
            });
            let function = b.load_word(record, METHOD_SITE_FUNCTION_OFFSET);
            b.retain(function);
            b.retain(receiver);
            b.store_word(self_out, 0, receiver);
            b.set(callable, function);
        },
        |b| {
            let loaded = cold(b);
            b.set(callable, loaded);
        },
    );
    b.get(callable)
}

/// The result of the call that consumes a method site's load, with `args`
/// the positional arguments after `self`.
///
/// Requires: the call adopts `callable`, `self_word` and `args`; `thread` is
/// this invocation's thread record; `record` is the record its load used.
/// `cold` is the runtime call that adopts the same inputs.
///
/// Ensures: on the inline path the entry adopts `self` and the arguments,
/// each default it appends is retained for it, the callable is handed to the
/// callee's frame entry and released after the call, and the outer handoff
/// and recursion depth are restored.
pub fn generate_method_site_call<B: MethodSiteBuilder>(
    b: &mut B,
    thread: B::Word,
    record: B::Word,
    callable: B::Word,
    self_word: B::Word,
    args: &[B::Word],
    cold: impl FnOnce(&mut B) -> B::Word,
) -> B::Word {
    use WordComparison::{Equal, NotEqual, UnsignedLessThan};
    let result = b.local();
    b.attempt(
        |b| {
            let null = b.constant(crate::loaded_method_null_bits() as u64);
            let has_self = b.compare(NotEqual, self_word, null);
            b.require(has_self);
            let function = b.load_word(record, METHOD_SITE_FUNCTION_OFFSET);
            let same_function = b.compare(Equal, callable, function);
            b.require(same_function);
            let entry = b.load_word(record, METHOD_SITE_ENTRY_OFFSET);
            let zero = b.constant(0);
            let has_entry = b.compare(NotEqual, entry, zero);
            b.require(has_entry);
            // k = arity - supplied: the trailing defaults the call appends.
            let supplied = (args.len() + 1) as i64;
            let arity = b.load_word(record, METHOD_SITE_ARITY_OFFSET);
            let appended = b.add_immediate(arity, -supplied);
            let usable = b.load_word(record, METHOD_SITE_DEFAULT_COUNT_OFFSET);
            let past_usable = b.add_immediate(usable, 1);
            let covered = b.compare(UnsignedLessThan, appended, past_usable);
            b.require(covered);
            let admitted = b.recursion_enter();
            let refused = b.constant(0);
            let admitted = b.compare(NotEqual, admitted, refused);
            b.require(admitted);
            let outer = b.load_word(thread, THREAD_RECORD_PENDING_INVOCATION_OFFSET);
            b.store_word(thread, THREAD_RECORD_PENDING_INVOCATION_OFFSET, callable);
            for k in 0..=METHOD_SITE_MAX_DEFAULTS {
                let shape = b.constant(k as u64);
                let this_shape = b.compare(Equal, appended, shape);
                b.if_then(this_shape, |b| {
                    let mut defaults = [self_word; METHOD_SITE_MAX_DEFAULTS];
                    for (index, slot) in
                        (METHOD_SITE_MAX_DEFAULTS - k..METHOD_SITE_MAX_DEFAULTS).enumerate()
                    {
                        let default =
                            b.load_word(record, METHOD_SITE_DEFAULTS_OFFSET + 8 * slot as i32);
                        b.retain(default);
                        defaults[index] = default;
                    }
                    let returned = b.call_entry(entry, self_word, args, &defaults[..k]);
                    b.set(result, returned);
                });
            }
            b.store_word(thread, THREAD_RECORD_PENDING_INVOCATION_OFFSET, outer);
            b.recursion_exit();
            b.release(callable);
        },
        |b| {
            let returned = cold(b);
            b.set(result, returned);
        },
    );
    b.get(result)
}

#[cfg(test)]
mod tests;
