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

The existing `logging_filters_propagation.py` and
`logging_percent_style_intrinsic.py` differential cases own callback order,
replacement scope, exceptions, lock release, mutable fields, styles and time
conversion. They run in the standard differential proof family. The filesystem
facade suite supplies only unrelated native allocation/locking hooks while
executing the actual Python classes against the host CPython behavior. Generated
ABI tests prove the private exports are removed. Numerical performance and
release-cell qualification require actual executed artifacts; source cost
removal alone establishes no wall-clock speedup.
