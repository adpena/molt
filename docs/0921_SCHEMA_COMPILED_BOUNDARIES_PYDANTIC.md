# Schema-Compiled Boundaries: Pydantic v2 Strategy
**Spec ID:** 0921
**Status:** Draft
**Audience:** Runtime engineers, web engineers, AI coding agents

## Purpose
Use explicit schemas (authored in Pydantic v2 or Molt DSL) to compile
fast, safe boundaries for:
- HTTP request/response
- IPC (Django ↔ Molt worker)
- background jobs
- DB row decoding (optional)

## Why Boundaries Matter
Most Python service cost lives at boundaries:
- JSON parsing
- validation
- object allocation
- serialization

Molt treats schemas as a compilation surface.

## Authoring
- Pydantic v2 models as input
- Optional Molt-native schema DSL
- Schemas normalized into Schema IR (SIR)

## Compilation Pipeline
1. Extract Schema IR
2. Generate decoders/encoders (JSON, MsgPack, Arrow)
3. Generate validators
4. Define internal struct layout

## Runtime Behavior
- Validation occurs once at boundary
- Internal code operates on typed layout
- No Pydantic calls on hot path (strict mode)

## Safety Tiers
- Reference mode: Pydantic runtime as oracle
- Production mode: compiled codecs
- Strict tier: compiled-only, trusted shapes

## Benefits
- Fewer allocations
- Lower latency variance
- Stronger internal guarantees
- Better IPC contracts

## Non-Goals
- Full Pydantic feature parity
- ORM replacement
- Arbitrary dynamic validators
