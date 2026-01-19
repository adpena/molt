# Molt Glossary

This document defines key terms and concepts used within the Molt project.

## General Terms

- **AOT (Ahead-Of-Time) Compilation**: The process of compiling Python code into native machine code *before* execution, as opposed to JIT (Just-In-Time) compilation during execution.
- **Closed World Assumption**: The compiler assumes it can see all code that will run, allowing global optimizations like tree-shaking and type inference.
- **Determinism**: The guarantee that for a given input source and environment, the compiler will *always* produce the exact same binary (bit-for-bit).
- **Soundness**: The property that the compiler never generates code that violates the language semantics (unless explicitly allowed by a break policy).

## Architecture & IR (Intermediate Representation)

- **Frontend**: The part of the compiler that parses Python source code and lowers it to the initial IR.
- **HIR (High-Level IR)**: A tree-based representation of the Python code, desugared (e.g., `for` loops become `while` loops) but still recognizable as Python.
- **TIR (Typed IR)**: An SSA-based Control Flow Graph where every value has an inferred type. This is where type checking and high-level optimizations occur.
- **LIR (Low-Level IR)**: A representation close to machine code, dealing with explicit memory management (RC), registers, and memory slots.
- **Cranelift**: The primary code generation backend used by Molt to turn LIR into native machine code. It is a pure-Rust code generator known for speed.

## Optimization Techniques

- **Invariant Mining**: The use of runtime trace analysis or static analysis to discover properties that hold true for the specific application (e.g., "this class is never modified after init"), enabling aggressive optimization.
- **Guard Synthesis**: Automatically generating runtime checks (`if type(x) == int: ...`) to allow specialized code to run safely in a dynamic context.
- **Monomorphization**: A compilation technique where a function is compiled multiple times ("cloned"), once for each specific combination of input types used in the program. This eliminates dynamic dispatch overhead.
- **Structification**: The process of converting a dynamic Python class (which normally uses a dictionary for attributes) into a native C-style struct with fixed-offset field access.

## Runtime

- **NaN-boxing**: A memory optimization technique where value types (like pointers, integers, booleans) are encoded into the 64-bit space of a floating-point NaN (Not-a-Number). This allows efficient value passing without separate type tags.
- **RC (Reference Counting)**: The primary memory management strategy used by Molt, similar to CPython, but with compiler optimizations to remove redundant operations.
- **Tier 0**: The subset of Python that Molt compiles to highly optimized, static native code.
- **Tier 1**: The "guarded" dynamic tier that handles more dynamic Python features by adding runtime checks and fallbacks.
- **Wasm (WebAssembly)**: A binary instruction format that allows Molt binaries to run in web browsers and other sandboxed environments.

## Development

- **Differential Testing**: A testing strategy where the same code is run on both Molt and CPython, and the results are compared to ensure exact parity.
