# Type System and Specialization Policy
**Spec ID:** 0017
**Status:** Draft (implementation-targeting)
**Owner:** frontend + runtime
**Goal:** Define Molt's current type universe, hint intake, and specialization rules.

---

## 1. Type Universe (Current)
### 1.1 Primitive tags
- `int`, `float`, `bool`, `None`

### 1.2 Heap types
- `str`, `bytes`, `bytearray`
- `list`, `tuple`, `dict`
- `range`, `slice`, `memoryview`, `buffer2d`
- `dataclass`
- user-defined classes (nominal)

### 1.3 Meta types
- `Any`: opt-out; no specialization or guards
- `Unknown`: alias of `Any` at runtime

## 2. Hint Sources and Normalization
### 2.1 Sources
- Python annotations (PEP 484/526)
- Type Facts Artifact via `molt check` (`docs/spec/0920_TYPE_FACTS_ARTIFACT_TY_INTEGRATION.md`)

### 2.2 Normalization rules
- `typing.`/`builtins.` prefixes are stripped.
- `Union`/`Optional`/`|` normalize to `Any`.
- Container generics are limited to:
  - `list[T]`, `tuple[T]` where `T` in {`int`,`float`,`str`,`bytes`,`bytearray`,`bool`}
  - `dict[str,V]` where `V` in {`int`,`float`,`str`,`bytes`,`bytearray`,`bool`}
- Unsupported hints are ignored (treated as `Any`).

## 3. Type Hint Policies
### 3.1 `ignore`
- Ignore all hints and type facts.
- No specialization or guards.

### 3.2 `trust`
- Use hints for specialization without runtime guards.
- Incorrect hints are user error and may produce undefined Molt behavior.

### 3.3 `check`
- Use hints for specialization with runtime guards.
- Guard failures raise `TypeError: type guard mismatch`.

## 4. Specialization Rules (Current)
- **Structification:** Fixed-layout fields for non-dynamic classes.
- **Access lowering:**
  - `GUARDED_GETATTR`/`SETATTR` for known fields.
  - Generic access when class is dynamic or attribute is a data descriptor.
- **Container hints:** propagate `list[T]`, `tuple[T]`, `dict[str,V]` to
  specialized ops where available.
- **No interprocedural inference:** specialization is local to known hints
  or type facts.

## 5. Guard Semantics
- Guards check the outer runtime tag only (no element checks).
- Guards may be emitted for arguments, locals, and returns when policy is `check`.
- Guard failure is a hard error (no deopt path yet).

## 6. Error Message Contract
- Guard failure message: `TypeError: type guard mismatch`.
- Invalid hints are ignored (no compile-time error or warning).
- No deopt messages are emitted yet; guard failures are terminal.

## 7. Roadmap Links
- Type facts pipeline: `docs/spec/0920_TYPE_FACTS_ARTIFACT_TY_INTEGRATION.md`.
- Strict tier rules: `docs/spec/0922_STRICT_TIER_RULES_FOR_TRUSTED_TYPES.md`.
