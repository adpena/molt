# Syntactic Features Matrix
**Spec ID:** 0021
**Status:** Draft
**Owner:** frontend
**Goal:** Track coverage of Python syntactic features (parser & AST handling) that don't map cleanly to single opcodes or types.

## 1. Syntax (Python 3.12+)

### 1.1 Structural Pattern Matching (`match` / `case`)
**Status:** Missing (Milestone 3)
**Complexity:** High (requires decision tree compilation).
- [ ] Match Literal (case 1:)
- [ ] Match Variable (case x:)
- [ ] Match Sequence (case [a, b]:)
- [ ] Match Mapping (case {k: v}:)
- [ ] Match Class (case Point(x=1):)
- [ ] Match Or (case A | B:)
- [ ] Match As (case A as x:)
- [ ] Match Guard (case A if x:)

### 1.2 Async / Await
**Status:** Supported (Milestone 2)
**Complexity:** High (State machine transformation).
- [x] `async def` (Coroutine creation)
- [x] `await` (Suspension point)
- [x] `async for` (Async iterator loop)
- [x] `async with` (Async context manager)
- [x] `async` comprehensions (`[await x async for y in z]`)

### 1.3 Comprehensions & Generators
**Status:** Supported (Milestone 1)
- [x] List comprehension (`[x for y in z]`)
- [x] Dict comprehension (`{k:v for x in y}`)
- [x] Set comprehension (`{x for y in z}`)
- [x] Generator expression (`(x for y in z)`)
- [x] Nested comprehensions
- [x] `await` inside comprehensions (see 1.2)

### 1.4 F-Strings
**Status:** Partial (Milestone 1)
- [x] Basic interpolation (`f"{x}"`)
- [ ] Expressions (`f"{x+1}"`)
- [ ] Format specifiers (`f"{x:.2f}"`)
- [ ] Debug specifier (`f"{x=}"`)
- [ ] Date formatting (needs datetime)

### 1.5 Type Hinting Syntax
**Status:** Supported (Ignored/Stored)
- [x] Function annotations (`def f(x: int) -> int:`)
- [x] Variable annotations (`x: int = 1`)
- [ ] `type` alias statement (`type Point = tuple[int, int]`) (3.12)
- [ ] Generic classes (`class A[T]:`) (3.12)
- [ ] ParamSpec/TypeVarTuple syntax

### 1.6 Decorators
**Status:** Partial
- [x] Function decorators (`@d`)
- [x] Class decorators (`@d`)
- [x] Stacked decorators (multiple `@` lines)
- [x] Decorator expressions (callables/attribute access/factory calls)

### 1.7 Assignment Expressions (Walrus)
**Status:** Supported (Milestone 1)
- [x] `(x := expr)` in `if`/`while`
- [x] `(x := expr)` in comprehensions

### 1.8 Unpacking
**Status:** Supported (Milestone 1)
- [x] Tuple unpacking (`a, b = c`)
- [x] Extended unpacking (`a, *b = c`)
- [x] Starred expression in calls (`f(*args)`)
- [x] Double-starred expression in calls (`f(**kwargs)`)
- [ ] Generalized unpacking in comprehensions

## 2. Parser Support
Molt uses `ruff-ast` (Rust) which supports Python 3.12 syntax.
The gap is primarily in **Lowering** (AST -> HIR), not Parsing.

## 3. TODOs
- TODO(syntax, owner:frontend, milestone:M3, priority:P2, status:missing): Implement `match` lowering (start with simple literals).
- TODO(syntax, owner:frontend, milestone:M2, priority:P2, status:partial): Implement `f-string` format specifiers + conversion flags + debug spec (needs `format()` protocol).


## 4. Matrix Audit (2026-01-16)
Coverage evidence (selected):
- `tests/differential/basic/class_decorators.py` (stacked decorators, factories, evaluation order).
- `tests/differential/basic/comprehensions.py` (list/set/dict comprehensions).
- `tests/differential/basic/generator_protocol.py` (generator expression lowering).
- `tests/differential/basic/async_for_else.py`, `tests/differential/basic/async_with_instance_callable.py` (async for/with lowering).
- `tests/differential/basic/async_comprehensions.py`, `tests/differential/basic/async_comprehensions_nested.py` (async comprehensions, nested + await coverage).

Gaps or missing coverage (audit findings):
