# Strict Tier Rules: Trusted Types and Aggressive Optimization
**Spec ID:** 0922
**Status:** Draft
**Audience:** Language designers, compiler engineers, power users

## Purpose
Define an opt-in **Strict Tier** where Molt may fully trust types,
enabling aggressive optimizations impossible in general Python.

## Motivation
Dynamic mutation defeats optimization.
Strict Tier introduces explicit constraints in exchange for speed and predictability.

## Entry Points
- Module marker (e.g. `# molt:strict`)
- Function decorator `@molt.strict`
- Package-level config

## Core Rules
- No monkeypatching after init
- Closed object shapes for strict models
- Restricted reflection
- Deterministic imports

## Boundary Enforcement
- Strict ↔ non-strict crossings validated
- Prefer schema-compiled boundaries
- Automatic guards where needed

## Optimization Privileges
When rules hold, Molt may:
- unbox primitives
- lower records to structs
- inline aggressively
- remove guards
- specialize without deopt

## Failure Model
Violations produce:
- compile-time errors when possible
- otherwise clear runtime traps

## Tooling
- `molt check --strict`
- strict compliance report
- integration with Type Facts trust levels

## Non-Goals
- Supporting all Python metaprogramming
- Preserving every dynamic behavior
- Being as permissive as CPython

Strict Tier is a contract: less magic, more power.
