# Filesystem metadata and directory-relative operations

The runtime owns filesystem admission and transport; compiled Python source owns
Python policy. The capability contract is
[CAPABILITIES.md](../../CAPABILITIES.md), and outstanding release acceptance is
[V1-19/V1-30/HF-122](../../agent/V1_HANDOFF_FINDINGS.md). This document replaces
an obsolete implementation plan whose directory-relative intrinsics already
exist. Source implementation does not qualify every platform cell.

## One selected route

`builtins/io.rs` owns mount selection and aggregate admission shared by builtin
open and metadata. Select the path after its Python path protocol, then require
the operation's grants together with its mount grants before accessing a
backend. A custom opener or borrowed descriptor keeps its own route. Neither a
missing mounted path nor an unsupported mounted operation may retry the host.
Invalid bytes must not alias a Unicode mount path through lossy conversion.

`builtins/io_path.rs` owns `stat`, `lstat`, metadata predicates and `readlink`.
The path satellite calls that owner for related metadata operations; it does not
maintain a second host-only implementation. A fixed metadata value carries facts
through the existing typed bridge without building a Python stat tuple for a
predicate. The intrinsic manifest and generated ABI/resolver projections expose
only the canonical providers. Python `os` and `pathlib` do not perform a prior
single-grant check that would hide the selected mount's missing grants.

Host metadata preserves kernel identity. VFS metadata exposes only facts its
backend supplies; unavailable identity must never make different files compare
equal. Current bundle/tmp/device backends do not implement symbolic links.
`readlink` must report that boundary without accessing a similarly named host
path. Adding link or identity support later requires a backend-owned protocol,
not a second path cache or fabricated host inode.

## Python and platform semantics

The reference boundary is CPython's pinned
[`genericpath.py`](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Lib/genericpath.py)
and [`posixmodule.c`](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Modules/posixmodule.c).
`stat` and its predicates accept descriptors; `lstat` and `readlink` use the path
protocol. Borrowed descriptors remain open. Preserve `str`/`bytes` flavor and
raw kernel errno through the satellite bridge. Predicate suppression of ordinary
filesystem errors must remain separate from capability denial.

Directory-relative variants use target `libc::AT_FDCWD` and the corresponding
`*at` operation on supported Unix targets. Unsupported target operations fail
explicitly. An absolute path does not become relative to a descriptor. A real
descriptor does not acquire virtual-mount grants. Conformance includes invalid
and nondirectory descriptors, nofollow, empty paths, embedded NULs, integer
conversion and callback/error order; existing differential files own these
cases. Version-specific `realpath`/`resolve` policy and archive extraction remain
separate, open source migrations over this primitive boundary.

## Cost and verification

A native metadata query performs one host metadata operation; a mounted query
performs one backend metadata operation. Neither reads file content. Route
selection and aggregate admission are necessary runtime security work. Reuse
open's route owner instead of adding a second registry, host retry, prefix walk,
process, flush or cache. Predicates use stack metadata rather than allocate and
unpack a Python tuple. Existing VFS path normalization/route allocations scale
with path length; this does not promise constant-time path lookup or eliminate
kernel pathname traversal. Matching the call budget does not establish a wall
clock or memory win; those require measurements in the accepted target cells.

Runtime boundary tests prove zero backend access on denial, complete missing
grants, mounted misses, descriptor borrowing and transport. The existing facade
suite proves the selected runtime refusal survives unchanged. The existing
`os_stat_rename_replace_intrinsic` differential case exercises actual metadata,
path-like arguments, descriptors, symlinks and readlink flavor against CPython.
Native and WASM execution remain distinct proof cells. These development checks
add no proof machinery or host Python dependency to emitted binaries.
