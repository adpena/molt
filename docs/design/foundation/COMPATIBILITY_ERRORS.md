# Compatibility error outcomes

`tools/compatibility_error_protocol.py` owns a closed set of CPython error
outcomes and projects their implementation to `_compatibility_errors.py` and
the shared Rust `builtins/compatibility_error.rs`. This is separate from the
closed numeric exception policy. Adding an arbitrary message, pathname, or
exception site cannot opt it into this protocol.

The Rust runtime checks target versions and each outcome's semantic applicability.
The Python projection contains only intrinsic forwarders; generated numeric tags
select typed Rust outcomes, without Python predicate duplication or string-based
runtime dispatch. Counter and both abstract/Windows signal methods share this
primitive. Exception construction preserves empty versus one-message args.
Windows signal refusal checks the real platform and operation. Memoryview
outcomes carry rank, index count, slice shape, or the actual format. An invalid
outcome request raises `SystemError`; it cannot manufacture a compatible
`NotImplementedError`. Counter's deliberately undefined class method rejects
before consuming either argument. WASM event-loop restrictions and unrelated
raw `NotImplementedError` sites remain implementation obligations.

The structural audit validates both complete runtime projections, pinned source
coordinates, and the presence of the existing differential witnesses. It
inventories declarations and consumer calls as `compatibility_error_outcome`
findings with operation, version, platform and predicate details. A modified
projection, dynamic or unknown context, missing witness, or unresolved import
consumes the existing Python/Rust stub metric. Raw raises retain their normal
classification regardless of matching messages. Generated-file filtering does
not bypass this verification. No ratchet baseline or release metric changed.

Source classification is not an execution receipt. The normal guarded
differential/runtime proof lane must still exercise the admitted target cells.

## CPython evidence

`config/python_compatibility_error_sources.json` records SHA-256 hashes and
version-tagged source URLs for CPython 3.12.0, 3.13.0 and 3.14.0, with
`memoryobject.c` pinned to the maintained oracle patches 3.12.13, 3.13.11
and 3.14.3. The evidence
covers the complete Windows loop inheritance chain and these operation owners:

| Operation | CPython authority | Relevant behavior |
| --- | --- | --- |
| Counter.fromkeys | `Lib/collections/__init__.py`, `Counter.fromkeys` | Exact exception/message, no argument consumption |
| Windows add/remove signal handler | `Lib/asyncio/events.py`, `AbstractEventLoop`; `windows_events.py`, `selector_events.py`, `proactor_events.py`, `base_events.py` | Windows loop classes inherit the unconditional bare refusal |
| Memoryview scalar access | `Objects/memoryobject.c`, `adjust_fmt`, `unpack_single`, `pack_single` | Syntax rejection differs from unsupported native scalar code; half-float `e` is supported |
| Memoryview rank/slice admission | `memory_item`, `memory_item_multi`, `memory_subscript`, `memory_ass_sub` | Scalar and tuple partial indexing have distinct messages; slicing preserves format without unpacking |
| Memoryview iteration | `memory_iter`, `memoryiter_next` | Scalar support is deferred until a value is read; maintained 3.12/3.13/3.14 check released state before rank and syntax |
| Memoryview count/index | `memoryview_count_impl`, `memoryview_index_impl` | Public methods begin in 3.14; count follows iterator construction; empty index does not inspect scalar format |
| Unicode array export | `Modules/arraymodule.c`, `array_buffer_getbuf` | Valid descriptors export `u` for 16-bit wchar and `w` for 32-bit wchar; scalar memoryview operations deliberately reject these codes |

Older CPython iterator implementations could return an iterator while retaining
a pending exception. Their call-specialization-dependent failure is not a stable
semantic oracle; differential receipts must identify the actual patch interpreter.
Historical `.0` source readings do not establish behavior of a maintained oracle.

Memoryview format syntax, leaf scalar admission, rank rejection, and released
iterator ordering share runtime helpers. Half-float packing/unpacking uses
`molt_obj_model::float_bits`, including rounding, signed zero, subnormals,
infinities and overflow. No memoryview-local float codec was introduced.

## Replaying the proof

Regenerate projections with `python tools/compatibility_error_protocol.py --write`;
validate them with `--check`. The Rust source is emitted in canonical rustfmt
form. `tools/generator_io.py` supplies publication and byte-exact comparison;
only checkout newline differences are normalized. There is no protocol-local
token equivalence or diagnostic whitespace normalization.

`tools/generator_manifest.toml` owns the two output paths and the
`CompatibilityError` closed domain. CI checks it with every other manifest
generator through the single `repository.generators` proof command
(`tools/generators.py check`); `python3 tools/molt_dev.py fix` rewrites it
together with the proof-plan projections. The structural audit performs
the same compatibility validation as part of its existing probes. Existing
proof consumers are:

- `tests/test_structural_audit.py` compatibility mutation cases.
- `tests/differential/stdlib/collections_counter.py`.
- `tests/differential/stdlib/asyncio_signal_handler_semantics.py` on Windows and POSIX native/LLVM cells.
- `tests/differential/basic/memoryview_multidim.py` and `memoryview_format_codes.py` on the claimed native/WASM and Python-version cells.
- `split_contract_scalar_conversion_revalidates_released_destination`, extended with half-float conversion in the existing memoryview runtime test module.

No extra test runner or proof lane is required.
