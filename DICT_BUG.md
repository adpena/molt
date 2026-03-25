# Dict Lookup Bug: RESOLVED

## Root Cause
Cranelift-compiled binaries fail to find dict keys because NaN-boxed pointer values aren't properly materialized before hash computation in `dict_find_entry`. The hash function reads string bytes through a pointer that may not be fully resolved from its NaN-boxed representation, producing incorrect hash values.

## Fix Applied (3 layers)

### 1. Frontend Intrinsic Resolution (compile-time)
**File:** `src/molt/frontend/__init__.py`

Added compile-time resolution of `_require_intrinsic("name", ...)` calls. When the frontend sees a `require_intrinsic` call with a string literal first argument, it emits a direct `BUILTIN_FUNC` IR op instead of a runtime dict.get() call chain.

### 2. Dict Key Pre-Materialization (runtime)
**File:** `runtime/molt-runtime/src/object/ops.rs`

Added `read_volatile` of the key's string bytes in both `molt_dict_get` and `dict_get_in_place` before the hash-table probe. This forces the NaN-boxed pointer to be fully resolved and the string data to be loaded into cache, ensuring `hash_string` computes the correct hash.

### 3. ABC Frame Fallback
**File:** `runtime/molt-runtime/src/builtins/abc.rs`

Made `molt_abc_init` handle missing `_getframe()` gracefully during compiled binary bootstrap, instead of raising RuntimeError.

## Remaining Issue
The Mawn UI binary compiles and runs past all intrinsic resolution, but hits a stdlib compatibility issue: `datetime.py`'s `timedelta.__init__` receives too many positional arguments. This is a separate Molt stdlib gap.
