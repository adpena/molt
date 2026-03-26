//! molt-runtime-asyncio: async I/O module group (asyncio, concurrent.futures)
//!
//! Extracted from molt-runtime to allow tree-shaking the async runtime
//! when not needed (e.g. synchronous CLI tools).
//!
//! # Extraction Status (2026-03-25)
//!
//! The async_rt module in molt-runtime (30,770 lines, 13 files) is **deeply coupled**
//! to the core runtime and cannot be moved here in a single step.
//!
//! ## Coupling analysis
//!
//! | Metric                          | Count |
//! |---------------------------------|-------|
//! | `crate::` references            |   762 |
//! | `with_gil_entry!` invocations   |   407 |
//! | Distinct runtime symbols used   |  ~60  |
//! | Files using glob `crate::*`     |     3 |
//! | Core re-exports of async_rt     |   22+ |
//!
//! Key dependencies that prevent extraction:
//!
//! 1. **`with_gil_entry!` macro** (407 uses, 10/13 files) — expands to
//!    `$crate::concurrency::GilGuard::new()`, so it can only compile inside
//!    molt-runtime. Would need to be re-exported as a standalone macro or
//!    replaced with a trait-based GIL API.
//!
//! 2. **Object model primitives** — `obj_from_bits`, `dec_ref_bits`, `inc_ref_bits`,
//!    `alloc_string`, `alloc_tuple`, `header_from_obj_ptr`, `resolve_obj_ptr`, etc.
//!    These live in `molt-runtime::object` and use NaN-boxing. Already partially
//!    in `molt-obj-model` crate but the async_rt code references the molt-runtime
//!    re-exports, not the upstream crate.
//!
//! 3. **Runtime state singletons** — `runtime_state()`, `exception_pending()`,
//!    `raise_exception()`, `record_exception()`, task exception stacks — all
//!    thread-local or global state in the core runtime.
//!
//! 4. **Bidirectional coupling** — `lib.rs` has 22+ `pub use crate::async_rt::*`
//!    lines re-exporting async symbols as part of the core public API. The
//!    `builtins/attributes.rs` module also directly imports async_rt internals.
//!
//! ## Incremental extraction plan
//!
//! ### Phase 1: Extract pure data types (low risk)
//! Move struct definitions that have no runtime deps:
//! - `MoltChannel`, `MoltStream`, `MoltWebSocket` (channels.rs, ~70 lines)
//! - `CancelTokenEntry` (cancellation.rs)
//! - `MoltTask`, `SleepQueue` (scheduler.rs, if field types allow)
//!
//! ### Phase 2: Create a GIL trait abstraction
//! Define a `GilProvider` trait in `molt-runtime-core` that both the core
//! runtime and this crate can implement/consume. Replace `with_gil_entry!`
//! macro with `GilProvider::with_gil(|token| { ... })` (~407 call sites).
//!
//! ### Phase 3: Route object-model deps through molt-obj-model
//! The async_rt code uses ~30 object primitives via `crate::*`. Most of these
//! should already exist in `molt-obj-model`. Update imports to use that crate
//! directly instead of the molt-runtime re-exports.
//!
//! ### Phase 4: Extract runtime-state via trait objects
//! `runtime_state()`, exception handling, and task stacks need a trait-based
//! interface so this crate can call them without owning the state.
//!
//! ### Phase 5: Move files
//! After Phases 1-4, the remaining coupling should be small enough to move
//! the full 13 files here with thin bridge re-exports in molt-runtime.
//!
//! ## Files and sizes (molt-runtime/src/async_rt/)
//!
//! | File              | Lines | `crate::` refs | `with_gil_entry!` |
//! |-------------------|-------|----------------|-------------------|
//! | sockets.rs        | 9,385 |           172  |              118  |
//! | generators.rs     | 7,936 |           236  |              107  |
//! | scheduler.rs      | 3,844 |            69  |               34  |
//! | channels.rs       | 2,976 |            76  |               52  |
//! | event_loop.rs     |   ~2K |            79  |               49  |
//! | process.rs        |   ~1K |            39  |               25  |
//! | poll.rs           |  ~800 |            39  |                0  |
//! | io_poller.rs      |  ~600 |            17  |                4  |
//! | cancellation.rs   |  ~500 |            15  |                9  |
//! | threads.rs        |  ~400 |            13  |                8  |
//! | net_stubs.rs      |  ~200 |             4  |                1  |
//! | task.rs           |    27 |             3  |                0  |
//! | mod.rs            |    94 |             0  |                0  |

// TODO: migrate from molt-runtime/src/async_rt/ — see extraction plan above.
// Phase 1 candidates for immediate extraction:
// pub mod channel_types;   // MoltChannel, MoltStream, MoltWebSocket structs
// pub mod cancel_types;    // CancelTokenEntry struct
