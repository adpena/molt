# CPython marshal over Molt values

## Decision and scope

HF-744 replaces the JSON stdlib serializer and pickle-backed C API with one
Rust wire codec in `runtime/molt-runtime/src/marshal/`. Native, LLVM and WASM
call that implementation. This is format support, not CPython bytecode
execution. The parent owns integration, the V1 ledger and lane retirement.

The parent correction of 2026-11-05T06:31Z decides scalar identity. **No numeric
identity migration is required or authorized by this change.** The Python
[data model](https://docs.python.org/3/reference/datamodel.html#objects-values-and-types)
allows immutable values to reuse existing identities. Molt's scalar words use
that latitude. The earlier identity witnesses establish a difference from
CPython's allocation choices; they do not block the corrected contract.

Values round-trip, and each runtime loads the other's bytes into equal values.
Heap sharing is preserved for str, bytes, tuple, list, dict, set, frozenset and
slice. Non-cached int, float and complex occurrences are separate unflagged
records, including heap carriers of those scalar types. Incoming references
to scalars yield their values. Integers in [-5, 256] use FLAG_REF and value-keyed
references, matching CPython's immortal small integers. This intentional
compatibility deviation is recorded in the stdlib compatibility surface.

Exact-byte proof uses graphs with non-cached scalars unshared in CPython and
matching heap refcounts and string interning. Outside it, notably CPython's
shared numeric objects, record structure and reference numbering may differ;
cross-load equality is required. Heap identity remains part of the oracle.
NaNs use canonical positive quiet-NaN bits 0x7ff8000000000000 on writing;
payload/sign variants are outside exact-byte proof. Current Molt heap NaNs can
retain payloads; canonicalization is a wire policy, not a storage necessity.

## Sources and existing mechanisms

The admitted target authority is `src/molt/target_python_policy.py`: 3.12,
3.13 and 3.14. This design read these references (the live pins are in
`config/cpython_regrtest_sources.toml`):

| Target | Tag | Commit | Default format |
| --- | --- | --- | --- |
| 3.12 | v3.12.15 | e848e4b09ca4bc67c0041c9ce7526dae471d4c36 | 4 |
| 3.13 | v3.13.17 | 64e080d38154391ff2d2caa1161c298bb7cd8f30 | 4 |
| 3.14 | v3.14.9 | ead82c07db1f111d613926ed1b6667411d768125 | 5 |

Sources: [3.12 marshal.c](https://github.com/python/cpython/blob/e848e4b09ca4bc67c0041c9ce7526dae471d4c36/Python/marshal.c),
[3.13 marshal.c](https://github.com/python/cpython/blob/64e080d38154391ff2d2caa1161c298bb7cd8f30/Python/marshal.c),
[3.14 marshal.c](https://github.com/python/cpython/blob/ead82c07db1f111d613926ed1b6667411d768125/Python/marshal.c),
and `Lib/test/test_marshal.py` at those commits. `w_object`, `w_ref`,
`w_complex_object`, `r_object`, `r_string` and `marshal_load_impl` own the wire
and file semantics. Implement those semantics against Molt's runtime model;
do not embed a Python interpreter or copy the pickle VM. This design uses
source study, not verbatim CPython code.

Prior art inside Molt: binary pickle uses the existing allocation, numeric,
buffer and pinned sequence/dict/set authorities. Reuse those boundaries, not
its protocol. `alloc_tuple_uninitialized` fills slots with the internal missing
singleton and marks reference-bearing storage; the GC never sees uninitialized
machine words. `seq_access::replace_capi_item` provides the existing fixed-slot
storage operation. Use it only during private decoder construction, with the
same edge accounting, before returning a tuple to user code. Tuple/list cycles
need early tuple publication in the reference vector, not tuple replacement
after parsing. Missing slots and partial cycles remain GC-safe on failure.

Observed baseline: marshal.py writes JSON, ignores version and omits file APIs
and version; the two header marshal functions call pickle and ignore version.
No runtime marshal codec or uncommitted donor was found at the lane base.
No performance improvement is inferred merely from removing those imports.

## Typed protocol and ownership

One closed `Tag` enum decodes the wire byte, one `RefId` indexes a flat reader
reference vector, and typed reference states distinguish reserved and ready.
NULL is a separate decoded result, never Python None or an exception sentinel.
One decoder consumes either a memory cursor or a file cursor. Both use the
same record machine. Frames and owned values have explicit release paths.

Reader construction uses explicit frames, not recursive Rust calls. Lists,
tuples, dicts and sets are allocated and registered before children; each frame
holds its construction owner and the reference vector owns its own retained
reference. For a tuple containing a list containing a reference to that tuple,
the backreference resolves to the original tuple; the list gains that edge,
then the tuple's missing slot is replaced with the list. Frozensets and slices
reserve an unresolved reference before children and publish only on completion;
references to unresolved slots fail. Empty frozensets can publish immediately.
On failure frame owners and reference roots release; cyclic residuals are owned
by the normal cycle collector. Immutable-only tuple cycles retain their edges
under the existing no-tp_clear contract; a synthetic fixture must keep a tuple
owner and explicitly retire its edge, rather than polluting later GC tests.
No partial object escapes through the result.
Before hashing a decoded dict key or set element, walk its immutable dependencies
iteratively. An edge to any still-constructing tuple is invalid and raises
ValueError("bad marshal data (invalid reference)") before insertion. The walk
has a visited-identity set; direct/indirect tuple cycles cannot recurse in Rust
or publish an unstable hash. Ordinary unhashable list/dict/set keys still reach
the existing hash authority and its TypeError. This protects malformed streams
that cannot be emitted from a valid completed Python value graph; the oracle
must not assume CPython safely accepts construction-invalid graphs.

The writer also uses explicit frames, with canonical retained snapshots for
container iteration. Operation-owned retains must be excluded when deciding
whether a heap object is shared: serialization must not change the flag it is
measuring. Memo entries retain their heap owners until completion. The memo is
operation-local graph state, not a persistent cache. Typed keys distinguish
heap identity from cached integer value. Slice references are unresolved until
their children complete; recursive slices fail rather than emit invalid refs.

Set sorting has semantic retains distinct from incidental bookkeeping. On all
three pinned targets the current element is retained while its standalone key
is generated, then retained by its sorting record until final emission. A Molt
snapshot may retain future elements early; subtract those incidental retains
until their traversal turn, and count one semantic retain for the current and
already-processed elements. Retains corresponding to CPython memo ownership
also remain semantic. This matters when a tuple is owned only by a set.

Standalone key serialization is scheduled as a fresh context in the same
iterative writer machine, with its own output, references and depth starting at
one. The suspended set frame resumes with the returned key bytes; there is no
recursive call of the codec. Singleton sets skip the sort key only after an
iterative proof that their element's immutable subtree consists of exact
builtins, fits the remaining record depth and cannot call a buffer exporter.
That typed proof travels down child frames, avoiding repeated scans of nested
singleton frozensets. Unknown exporters
still run standalone key serialization, preserving callback effects even when
ordering is trivial. The semantic element retain remains present in both cases.
Target 3.14 checks set size after callbacks, following its iterator-based source.

## Wire rules

* `N F T S .` denote None, bool, StopIteration type and Ellipsis. They do not
  consume a reference slot, even if a reader sees FLAG_REF on their tags.
* For version >=3, heap values use CPython's w_ref rule with Molt refcounts:
  refcount >1 or an interned string gets FLAG_REF on first visit and `r` on
  repetition. Immortal heap objects count as shared. Cached integers use the
  explicit scalar rule above. Versions <3 do not write references.
* Signed i32 uses `i`; other integers use `l`, signed i32 count followed by
  base-2**15 little-endian u16 digits. Reader also accepts historical `I` i64.
  Zero long digits, normalization, range and size checks follow the source.
* Versions >1 write IEEE little-endian `g`/`y`; older versions write `f`/`x`,
  length-prefixed decimal g17. Reuse `ops_format::format_float_value_with_spec`
  with precision 17 and type g, not Rust Display. Signed zero and inf/NaN must
  match CPython's spellings; decimal parsing uses the existing float authority.
* `s` writes bytes and contiguous buffer exporters; Unicode writes UTF-8 with
  surrogatepass. `u`/`t` are general/non-ASCII strings; >=4 selects `a/A` for
  ASCII and `z/Z` for ASCII length <256. Interned variants follow the actual
  runtime flag, not identifier spelling. ASCII-tag input bytes >=128 denote
  Latin-1 code points, as CPython's one-byte Unicode constructor does.
* Tuples use `)` for length <256 at >=4, otherwise `(`. Lists use `[`, dicts
  `{` with NULL termination; dict insertion order is preserved. A NULL dict
  value also ends the dict, per CPython. Sets `<` and frozensets `>` sort their
  elements by standalone dumps under the selected version. 3.14 sorts
  (dump, traversal-index, value), so ties retain traversal order and never
  compare unorderable values. 3.12/3.13 sort (dump, value): compare equal-key
  values with Python equality followed by less-than. marshal.c maps any sort
  failure, including unorderable unequal values, to MemoryError. Reuse Molt's
  target-aware fallible Timsort with less-than alone, as list.sort does.
  NaNs do not form the total order
  required by Rust sort_by; they must not turn a valid graph into a Rust panic.
  A failed standalone key dump becomes ValueError("unmarshallable object")
  in its enclosing writer, including nested depth, exporter or comparison errors.
* Slice `:` writes only when both target >=3.14 and version >=5; otherwise
  writing raises the existing unmarshallable-object ValueError. The reader
  accepts it only on target >=3.14. Other tags are accepted regardless of writer
  version. Target facts come from `runtime_target_python_info`, never the build host. 3.15 and its
  frozendict format are outside the admitted target set.

All lengths/IDs use checked conversions, checked reservations and typed
MemoryError paths. Depth counts each record, including refs: Windows 1000,
WASI 1500, other targets 2000; 3.13+ non-macOS Apple targets use 1500 too.
Writer over-depth raises ValueError("object too deeply nested to marshal");
reader raises ValueError("recursion limit exceeded"). Explicit frames avoid
native/WASM stack exhaustion before those limits.

Memory reads ignore trailing bytes. Missing type byte is
EOFError("EOF read where object expected"); missing memory payload is
EOFError("marshal data too short"). File payload EOF uses
EOFError("EOF read where not expected"). Negative lengths and malformed refs
retain the tag-specific CPython ValueError messages. NULL inside tuple/list/set
uses the corresponding TypeError; top-level NULL uses the object TypeError.
Malformed UTF-8 delegates to the existing surrogatepass decoder and preserves
its UnicodeDecodeError. No replacement decoding or permissive validation.

## Python and C boundaries

The Python module is thin intrinsic binding. Target 3.12 exposes positional-only
value/file/version with no allow_code. 3.13+ accepts keyword-only allow_code
(introduced in 3.13). Defaults are selected from runtime target facts. All
dump/dumps code objects raise ValueError("unmarshallable object"). All load/loads
code records raise the registered AOT-boundary ValueError before parsing their
fields, regardless of allow_code. No build-time refusal is added in this lane.

`load(file)` calls read(0), checks for bytes and then calls the typed Readinto
method with a writable memoryview for exactly the requested amount. It validates
the returned index and rejects over-report with CPython's formatted ValueError;
it never reads a second object. dump calls write with the complete encoded bytes
and returns the write method's result, propagating exceptions (the pinned
sources and CPython 3.12.15 return the byte count for BytesIO). The compatibility-error authority
gains an explicit AOT-boundary outcome/provenance, rather than pretending that
Molt's diagnostic is a CPython source quotation.

`PyMarshal_WriteObjectToString` and `PyMarshal_ReadObjectFromString` use the same
runtime codec through typed `RuntimeHooks::marshal_write/marshal_read`, including version, errors
and ownership conversion. Delete both fake delegates in the same implementation
commit. Intrinsic declarations and WASM ABI sources generate all their consumers.
Exact builtin admission must include native C-ABI values; it must not silently
accept subclasses through numeric conversion or lose their physical ownership.
Both source and linked ABI headers include `_marshal_exports.h`; neither imports
a Python serialization module. The C-ABI crate owns public PyObject ingress and
result projection; the runtime owns encoding/decoding. Hook ABI version 76 adds
the two optional capability fields. A host without a runtime refuses explicitly;
there is no alternative codec. Existing managed callers remain borrowed from
their C owners; native values use canonical temporary wrapper custody. A typed
MarshalInputOwnership distinguishes those two cases so the writer excludes
only transport pins. Foreign heap sharing accounts for both runtime owners and
physical C owners, subtracting the wrapper's one physical reference. Neither
ingress nor container snapshots silently change FLAG_REF.

## Review answers

Two independent sequential read-only Goldfish reviews examined the previous
design. Their original reports and input receipts remain in the lane artifacts.
They did not execute implementation tests. The parent decision supersedes their
scalar prerequisite, not their engineering findings:

| Finding | Answer under the corrected contract |
| --- | --- |
| design-1: unresolved gates | Scalar scope is decided; code-error precedence follows the brief; engineering below is owned by this lane. |
| design-2: escape-identity migration missing | Superseded by explicit parent decision: do not migrate scalar identities. |
| design-3: undefined byte partition | Cached integer/value and heap-identity rules and CPython unshared scalar partition are explicit above. Shared numeric graphs require cross-load values. |
| design-4: recursive construction safety | Missing-filled tuple allocation, fixed-slot edge replacement, retained frame/ref roots, delayed frozen/slice publication and normal GC custody close the mechanism. |
| design-5: decimal/set/depth/code rules | g17 uses existing formatter; set ties follow pinned target source; depth/error table and unconditional code-boundary precedence are above. |
| readiness-1: conflict is scope, not ambiguity | Acknowledged; parent resolved scope and explicitly changed the marshal contract. |
| readiness-2: unrealizable scalar prerequisite | Superseded; no compiler/value-model/ABI identity expansion is undertaken. |
| readiness-3: no reader construction lifecycle | The tuple/list-cycle and failure-root walkthrough above defines construction without exposing uninitialized words. |
| readiness-4: set algorithm unfinished | Standalone dump ordering and target-specific tie rule are explicit; ties/NaNs remain adversarial oracle cases. |
| readiness-5: missing chosen mechanisms | Reuse formatting/allocation/buffer authorities, typed iterative frames and per-tag errors rather than new parallel machinery. |

The corrected design's independent adversarial review completed successfully
as a review, with two High and two Medium findings. Answers:

| Finding | Resolution |
| --- | --- |
| correction-design-1: incomplete tuples may be hashed | Guard immutable key dependencies against constructing tuple identities before dict/set insertion; malformed graph regression required. |
| correction-design-2: set retains alter flags | Distinguish incidental snapshot roots from semantic current/sort-record and memo retains; target pair/triple tie algorithms are explicit above. |
| correction-design-3: standalone key dumps recurse | Schedule independent key contexts in one iterative machine; singleton ordering needs no key and avoids repeated nested serialization. |
| correction-design-4: NaN equality oracle undefined | Structural comparison below checks scalar NaN class, signed zero, heap alias correspondence and one-to-one unordered matching. |

The second independent sequential review found two Medium specification gaps
and explicitly concluded implementation can proceed after these clarifications:

| Finding | Resolution |
| --- | --- |
| correction-readiness-1: slice writer target guard missing | Writer requires both target >=3.14 and version >=5; explicit version 5 is tested on all admitted targets. |
| correction-readiness-2: buffer oracle normalization missing | Buffer exporters normalize to bytes before structural comparison; repeated identity follows references at versions >=3, preserving sharing between the resulting bytes objects. Ordinary bytes retain their own heap-identity obligation. |

Both reviewers used gpt-6.1-sol at high effort, read only the lane design and
completed with exit 0. All findings are answered here. Readiness is a design
judgment; every implementation and qualification obligation still requires proof.

## Proof obligations and qualification

Independent CPython 3.12/3.13/3.14 oracles distinguish wrong tags/flags/RefIds,
numeric value aliasing, lost heap cycles, wrong signatures/defaults, file
over-read, malformed-input errors and C-API bypass. Cover versions 0 through
default, historical tags, string lengths 0/255/256, surrogatepass, big integers,
float specials, equal-but-distinct scalars, nested/shared/recursive containers,
buffers, malformed/truncated streams, repeated loads and bad readers.

Cross-load comparison is structural: identical scalar types and values, signed
zero bits, and NaN-to-NaN equivalence; complex components follow those same
rules. Ordered containers compare corresponding edges, with a bidirectional
heap-identity correspondence checked before descending cycles. Dict/set
comparison uses one-to-one element/key matching, including NaNs, and checks
cardinality, so distinct NaN keys cannot collapse or match one key twice.
Canonical NaN output bits are checked separately. Scalar identity is never
part of that mapping; heap identity always is for sharing-capable versions.
Inputs exposing a buffer normalize to their contiguous bytes payload before
comparison, with exporter identities mapped to decoded bytes identities for
sharing-capable versions. This is the protocol's bytes conversion, not type
normalization for arbitrary values. Signed zero and NaN rules remain explicit.

Run generated byte and cross-load corpus on native, LLVM and WASM; report each
unverified cell. Execute pinned test.test_marshal and test.test_zipimport through
the existing regrtest expectation authority. HF-579 owns the first code-object
case; report its observed exception, not an inferred pass. Run changed Python
readers, generators write/check, touched Rust --lib suites in dev/release-fast
with nonzero counts, landing static-only and precheck-only from the runner's
plain environment. Keep command/status/count/source/tool receipts private.
Measure whole-program time/RSS/size; do not claim speed from static deletion.

This document chooses the implementation contract. It does not itself assert
that the implementation, backend matrix, performance or landing is qualified.
