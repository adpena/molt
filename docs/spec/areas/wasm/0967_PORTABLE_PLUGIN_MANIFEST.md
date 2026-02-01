# Molt Portable Plugin Manifest + Schema Resolution
**Spec ID:** 0967
**Status:** Draft
**Priority:** P1
**Audience:** runtime engineers, compiler engineers, host integrators
**Goal:** Define a portable, schema-first plugin manifest and deterministic schema
resolution rules for ABI v0.1 modules.

---

## 0. Constraints (Non-negotiable)
- ABI surface remains **init / call / poll / cancel** (see 0964).
- All boundary data is **schema-defined**; no dynamic object proxies.
- **Determinism** is required by default; avoid runtime “latest” resolution.
- Hosts may load modules dynamically, but **modules never dynamically import**.

---

## 1. Manifest Format (v0.1)
A plugin MUST ship a manifest as a custom section (preferred) or sidecar JSON.

```json
{
  "abi_version": "0.1",
  "module_name": "molt_rules",
  "module_version": "1.2.0",
  "exports": [
    {
      "function_id": 12,
      "name": "validate_order",
      "input_schemas": ["schema://orders/v3"],
      "output_schema": "schema://orders/validation/v3",
      "codec": "msgpack",
      "deterministic": true,
      "capabilities": []
    },
    {
      "function_id": 21,
      "name": "score_cart",
      "input_schemas": ["schema://cart/v1", "schema://cart/v2"],
      "output_schema": "schema://pricing/v2",
      "default_schema_id": "schema://cart/v2",
      "schema_compat": "backward",
      "codec": "msgpack",
      "deterministic": true,
      "capabilities": ["time.read"]
    }
  ],
  "schemas": [
    "schema://orders/v3",
    "schema://orders/validation/v3",
    "schema://cart/v1",
    "schema://cart/v2",
    "schema://pricing/v2"
  ]
}
```

### 1.1 Required fields
- `abi_version`, `module_name`, `module_version`
- `exports[]` entries with `function_id`, `name`, `input_schemas`,
  `output_schema`, `codec`, `deterministic`

### 1.2 Optional fields
- `default_schema_id`: default used when caller omits `schema_id`
- `schema_compat`: `exact` (default) or `backward`
- `capabilities`: explicit required host capabilities

---

## 2. Deterministic Schema Resolution
Schema resolution is performed by the **host**; modules never infer schemas.

### 2.1 Call rules
When the caller omits `schema_id`:
1. If `default_schema_id` is present → use it.
2. Else if exactly one `input_schemas` entry exists → use it.
3. Else return a **SchemaRequired** error (ambiguous).

### 2.2 “Latest” policy
- **Runtime “latest” is forbidden** (nondeterministic).
- “Latest” can be resolved at **build/publish time** and recorded as
  `default_schema_id` in the manifest.

---

## 3. Host Dispatch Algorithm (Reference)
1. Validate manifest + ABI version.
2. Register `function_id → module` routing.
3. On call:
   - Resolve `schema_id` using §2.
   - Validate payload against schema.
   - Dispatch `call(function_id, payload, schema_id)`.
4. Enforce capability grants before invoking module.

---

## 4. Error Codes (Recommended)
- `SchemaRequired` (missing/ambiguous)
- `SchemaNotFound`
- `SchemaMismatch`
- `CapabilityDenied`
- `InvalidPayload`

---

## 5. Compatibility Guarantees
- `schema_compat = exact`: payload must match schema ID exactly.
- `schema_compat = backward`: payload may match prior compatible versions,
  but must be validated by host.

---

## 6. Security Notes
- Manifest is part of the trust boundary; host must validate signature or
  integrity if loading untrusted modules.
- Capabilities are **host-controlled**; modules cannot self‑grant.

---

## 7. Open Questions
- Embed manifest in custom section vs sidecar JSON standardization.
- JSON schema format for `schemas` registry.
