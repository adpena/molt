# Logging record and filter authority

Python `LogRecord` fields are the sole mutable record state. `Filterer` passes
one record through the live callback list, preferring an object's `filter`
method to its callable protocol. A false result vetoes immediately; a returned
`LogRecord` becomes the next callback's record. Attribute lookup, callback and
truth-value failures propagate. The result retains its identity.

`Logger.handle` distributes its resulting record to the handler chain.
`Handler.handle` owns a local replacement and returns the exact filter result;
sibling and ancestor handlers receive the logger's record. Propagation consults
ancestor handlers without rerunning ancestor logger filters. The handler's lock
policy is selected once when its class is defined: CPython 3.12 uses the public
acquire/release hooks; 3.13 and later enter the lock directly. Every path releases
the selected lock when emission raises. The base handler emits the existing
closed compatibility-error protocol's CPython-mandated abstract-method outcome.

`getMessage` reads current `msg` and `args` and preserves conversion/formatting
exceptions. Formatter styles consume the record's live dictionary through the
existing `%`, `str.format` and Template operations. Time formatting uses current
record fields and the selected converter. Formatting publishes `message` and
`asctime` back to that same record. The duplicate native snapshots, registries,
format parsers and their private exports are retired with all producers and
consumers; no synchronization bridge or native record finalizer remains.

These rules follow the corresponding `Filterer`, `Handler`, `Logger`, `LogRecord`
and `Formatter` implementations in CPython
[3.12](https://github.com/python/cpython/blob/v3.12.13/Lib/logging/__init__.py),
[3.13](https://github.com/python/cpython/blob/v3.13.11/Lib/logging/__init__.py),
[3.14](https://github.com/python/cpython/blob/v3.14.3/Lib/logging/__init__.py) and
[3.15](https://github.com/python/cpython/blob/v3.15.0/Lib/logging/__init__.py).
Target-version and release qualification remain in the V1 ledger and canonical
support matrices. Python code here compiles into Molt programs using the existing
runtime primitives; execution needs no host Python.

## Cost and proof

Filtering is O(filters visited), stops at the first veto and allocates no filter
snapshot. Formatting uses the existing record dictionary directly, removing the
extra field-list walk and dictionary copy. Record/formatter construction removes
native registry insertion, retained string copies, atomics and eventual drop
calls. A logging format no longer enters a second record/formatter state owner
or queries its registries. The required message conversion occurs when requested,
so unused messages cause no eager `str` callback.

The `$` style follows [CPython's Template protocol](https://github.com/python/cpython/blob/v3.12.13/Lib/string.py)
through the shared `string.Template` lexical authority. One Rust
token iterator serves substitution, validation and ordered identifier discovery.
The intrinsic manifest declares its four fields: literal text, optional name,
optional original spelling and optional invalid-character offset. Syntax failure
is distinct from mapping failure: strict substitution propagates lookup and
conversion errors, while safe substitution preserves the placeholder on
`KeyError`, including one raised during conversion, as CPython does.
Invalid offsets count Python characters as the scanner advances. Braced names
must end at the closing brace; no parser searches the remaining suffix for one.
Identifier discovery uses a membership set and preserves first-seen order.
Projection ownership uses the runtime's existing drop guards until aggregate
construction retains each field and row. Custom Template pattern/flag semantics
remain partial in the stdlib matrix.

The existing `logging_filters_propagation.py` and
`logging_percent_style_intrinsic.py` differential cases own callback order,
replacement scope, exceptions, lock release, mutable fields, styles and time
conversion. They run in the standard differential proof family. The filesystem
facade suite supplies only unrelated native allocation/locking hooks while
executing the actual Python classes against the host CPython behavior. Generated
ABI tests prove the private exports are removed. Numerical performance and
release-cell qualification require actual executed artifacts; source cost
removal alone establishes no wall-clock speedup.

An advisory Linux/AArch64 development-profile probe at `f91d0cf42` and
`fb7a4c9b3` constructs 20,000 records, retrieves their unchanged messages and
formats them. Three alternating executions of each retained binary, with an
empty guest PATH and Python environment selectors removed, produce these median
costs. The CPython column uses 3.12.15 on the same host.

| Operation | Previous Molt, µs | Unified Molt, µs | CPython, µs |
| --- | ---: | ---: | ---: |
| Record construction | 803.636 | 626.177 | 1.096 |
| `getMessage` | 49.130 | 30.912 | 0.035 |
| Formatting an existing record | 52.948 | 200.707 | 0.251 |

The median sum of all three costs is 906.885 versus 859.303 µs. The unstripped
development executable is 76,942,160 versus 68,795,072 bytes. Earlier sequential
samples showed substantial host-speed drift and cannot establish the relative
timing. The alternating result is a narrow development observation, not release
performance acceptance or a claim about other workloads. Correct formatting now
pays the compiled Python method-dispatch path instead of rendering a stale
native snapshot. Its isolated regression and the remaining gap to CPython must
be recovered through shared dispatch, representation and inlining mechanisms
(HF-198/HF-202 and V1-07), without restoring a duplicate state owner or lowering
logging by spelled names. Release profiles and the full performance matrix
remain unqualified.
