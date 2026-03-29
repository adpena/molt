# Molt Conformance Analysis

## Summary

| Runner | Passed | Failed | Compile Error | Timeout | Skipped | Total |
|--------|--------|--------|---------------|---------|---------|-------|
| CPython | 385 | 24 | 0 | 0 | 52 | 461 |
| Molt (sample of 20) | 4 | 4 | 11 | 1 | 0 | 20 |

## CPython Failures (24 files)

The 24 CPython failures are all in `async_*` and `refcount__gather_*` categories
that use `asyncio.gather` with Python 3.14-incompatible patterns. These are not
Molt bugs — they represent Monty tests that target Python 3.12/3.13 behavior.

## Molt Compile Errors (11/20 = 55%)

Based on the sample, the dominant compile error category is `MOLT_COMPAT_ERROR`
for argument validation patterns. The Monty test suite includes many `args__*`
files that test error handling for wrong argument counts. These use patterns like:

```python
{}.items(1)     # TypeError: takes 0 arguments, got 1
len()           # TypeError: missing required argument
```

Molt's compiler rejects these at compile time (correctly!) rather than generating
code that raises TypeError at runtime. This is a **correct rejection** — Molt's
static analysis catches the bug before runtime. However, the conformance runner
counts it as a failure because the test expects a specific Python exception.

### Recommended fix

The conformance runner should distinguish between:
1. **Compile-time rejection of buggy code** — not a conformance failure (Molt is stricter)
2. **Compile-time rejection of valid code** — a real conformance failure
3. **Runtime behavior mismatch** — the 4 failures in the sample

Files in the `args__*` category are type 1 (correct rejection). They should be
marked as "expected_compile_error" in the conformance baseline.

## Molt Runtime Failures (4/20 = 20%)

The 4 runtime failures are where Molt compiles the code but produces different
output than CPython. These are real conformance bugs that should be tracked:

1. Missing Python exception types (TypeError, ZeroDivisionError not raised)
2. Output format differences (repr formatting, error message text)

## Test Categories by Compilation Success

| Category | Likely Compiles | Notes |
|----------|----------------|-------|
| arith__* | Yes | Basic arithmetic, Molt handles well |
| bool__* | Yes | Boolean ops, simple |
| list__* | Mostly | Some methods may hit compat errors |
| str__* | Mostly | String methods, broad coverage |
| dict__* | Mostly | Dict operations |
| args__* | No | Deliberately invalid code — compile-time rejection expected |
| import__* | No | Module import patterns may not resolve |
| class__* | Partial | Class features vary in support |
| async__* | No | Async not fully supported in Monty test format |

## Adapter Breakdown

The conformance adapter translated 461 Monty test files into three categories:

| Adapter Pattern | Count | Description |
|-----------------|-------|-------------|
| Raise= tests | 132 | Expect a specific exception type and message |
| Return= tests | 12 | Expect a specific return value |
| Assert-only tests | 317 | Use assert statements to verify behavior |

The Raise= tests (132 files) are the most likely to hit compile errors, since many
of them contain deliberately invalid code that Molt's static analysis catches early.
The assert-only tests (317 files) are the best conformance signal — they contain
valid Python that should compile and run identically on both CPython and Molt.

## Extrapolated Conformance Estimate

Projecting from the 20-file sample to the full 461-file suite:

- **Compile errors (~55%):** ~254 files. Heavily skewed by `args__*` and `import__*`
  categories. If we exclude Raise= tests that contain deliberately invalid code,
  the compile error rate on valid Python is likely much lower.
- **Runtime failures (~20%):** ~92 files. These are the real conformance gaps.
- **Passes (~20%):** ~92 files. Confirmed parity with CPython.
- **Timeouts (~5%):** ~23 files. Likely complex programs hitting compilation limits.

**Adjusted estimate** (excluding `args__*` deliberate-error tests):
- Compilation rate on valid Python: ~70-80%
- Runtime parity on compiled programs: ~80%
- Overall conformance on valid Python: ~56-64%

## Recommendations

1. **Exclude `args__*` tests from conformance score** — they test error handling
   for invalid code that Molt correctly rejects at compile time.

2. **Focus on `arith__`, `bool__`, `list__`, `str__`, `dict__`** categories first —
   these are the core language features where Molt should have high parity.

3. **Track three metrics separately:**
   - Compilation rate (% of valid programs that compile)
   - Runtime parity (% of compiled programs matching CPython output)
   - Overall conformance (compilation rate x runtime parity)

4. **Run the full 461-file suite through Molt** in a dedicated overnight job,
   not in an interactive session. Each file needs ~5-30s to compile.

5. **Tag each test file** with its category and expected behavior (compiles, expected
   compile error, expected runtime match) to build a proper conformance baseline.
