# Build early cutoff

Status: implemented, 2026-10-16 (HF-250). Owner: frontend build speed.

## Problem

A `molt build` whose inputs did not change still runs the whole pipeline to
compute the backend cache key from lowered IR. Measured on a warm checkout,
`super_comprehension_frame_ownership.py`, native, dev, backend cache hit:
32-44 s wall (`backend_prepare` 10-16 s, `module_graph` 7-9 s, the rest in
analysis, lowering, sealing and linking). Differential suites rerun unchanged
programs constantly.

## Prior art

- Cargo: per-unit fingerprints plus rustc dep-info; a no-op build costs
  0.1-0.3 s.
- ninja: `.ninja_log` and depfiles; a no-op build stats each edge's inputs and
  outputs. `ninja -d explain` names the first changed input.
- Go: action IDs hash every input; file hashes are cached by
  (path, size, mtime, inode) (`cmd/go/internal/cache`).
- Bazel, Buck and Salsa: dependencies are recorded where they are read, not
  enumerated afterwards.
- git: the racy-git rule; a file changed within one timestamp tick of the
  index cannot be trusted by stat alone.

## Design

**Key.** SHA-256 over the schema, every `build()` argument except `verbose`,
the working directory, the interpreter and its version, the host (system,
release, machine, and the CPU model for non-portable builds, which tune for
the host CPU), and the full environment except terminal and session
identifiers. The output path and the output directory are keyed: building one
program to two names, or under two output directories, produced binaries that
differed in 49 and 48 bytes (the linker signs the binary under its file name;
the debug map names object files under the output directory).

**Two modules.** `molt.input_report` is the reporting side: the token and
probe primitives, the audit hook, worker logs and the `report_*` functions.
Lowering code imports it, so it is part of the frontend's semantic
fingerprint; it is a leaf that imports no other Molt module, and a test keeps
the recorder out of that fingerprint. `molt.build_record` is the recorder: the
ledger, the key, the record, the check and trimming. Only `molt build`
imports it.

**Ledger.** One build records into one ledger. Only the first build of a
process records: process-global caches can serve a later build without a read
to report, whether or not the earlier build was eligible. Inputs reach the
ledger two ways:

- Authorities report when they read: source snapshots (token from `fstat`
  before and after the read, plus the digest), the import topology (listed
  directories, lookups with their resolved target), runtime source trees,
  backend sources, Cargo resource custodies, Cargo configuration lookups,
  frontend data files behind process-global caches (on every use), project-root
  probes, lockfile gate inputs, rustup selection files, link tools, and the
  macOS `xcode-select` link. Every executable search on the native build
  path goes through `toolchain_identity.executable_candidates`, which reports
  each searched directory and the selected entry: a program added to an
  earlier directory, or a retargeted link, selects another executable.
- A `sys.addaudithook` observer sees every file the process opens, every
  directory it lists or creates and every program it starts. Process-pool
  workers log their facts to a directory the recording process merges; a
  forked child drops the copied ledger and logs its own. A bytecode read stands
  for its source. A path the build wrote, and anything under a directory it
  created, is its own product. Reads under named product roots (the Molt
  cache, Cargo target and build state roots, the backend daemon's socket
  directory, the artifact root under the output directory) are not inputs. A
  source checkout, a user directory, the temporary directory and the shared
  scratch root are never excluded: projects live there too. A path that no
  longer exists becomes an absence probe plus its parent directory.

**Refusals.** No record for non-native targets, directory outputs, requests
that publish more than one binary (JSON results, diagnostics, IR, linked or
split output, snapshots, audit logs), non-default codegen backends, BOLT,
file-naming inputs whose loaders do not report (type facts, PGO profiles,
runtime feedback, capability manifests, sysroots, capability files), `ty`
under the trust policy, external static packages, and any ledger refusal,
racy input, or input that changed during the build. A failed record never
fails the build; `--verbose` prints why no record was written and why a
record did not match.

**Racy rule.** `file_hashing.DIGEST_RACY_WINDOW_NS` (2 s) over
`content_change_time_ns` (ctime on POSIX, NTFS ChangeTime on Windows; none
fails closed), measured from the start of the recording process
(`molt.process_birth`: `/proc/self/stat`, `proc_pidinfo`, `GetProcessTimes`).
A long-lived process, such as the batch build server, may hold bytes it read
long before its build began; any input changed since it started refuses the
record. An `rsync -a` keeps an old mtime but cannot keep ctime.

**Check.** Stat every recorded directory, probe and file, then the output.
A changed token on a file with a recorded digest falls back to a digest
compare. A hit when the output still holds the binary the build published
does nothing else: there is no artifact store and no copy.

**Trimming.** A hit refreshes its record's mtime at most once an hour; records
unused for a week are deleted, at most once an hour.

## Measured

- No-change rebuild through `python -m molt.cli`: 0.43-0.46 s, from 32-44 s.
  Before the dispatcher imported each command lazily it was 0.87-1.02 s, and
  copying the binary from a store on another volume cost 1.5 s more.
- The record holds 75,312 files (69,329 under the Xcode SDK, from the C
  toolchain custody), 17,162 directories and 745 probes: 26.5 MB of JSON.
- Edits to a runtime Rust source, a stdlib module, the intrinsics manifest,
  rustup settings, a Molt CLI module and the checkout's `.cargo/config.toml`,
  and a new `.cargo/config.toml` in an ancestor, each make the record miss.

## Limits

- Inputs on a network filesystem whose clock is behind this host's can pass
  the racy rule; git, ninja, make and Cargo share this limit.
- A process that writes the output while the build that publishes it runs
  can leave a record of the wrong bytes, as with make and ninja.

## Review history

The first prototype collected inputs at the end of the build; an adversarial
review found 13 gaps, which the read-time ledger closed. A second review found
six ways to serve a stale binary: excluding the custody root and the output's
parent dropped reads under the checkout (`.cargo/config.toml`); tokens taken
at record time could postdate the bytes a long-lived process held; forked
workers lost their reports; tool selection recorded only the resolved
program; stat probes raise no audit event; and an artifact store that shared
an inode with the output could be corrupted in place. Each has a regression
test. The review also showed the output name and directory change the binary,
which removed the artifact store.

## Next

1. `runtime/molt-launcher` (the native `molt`) checks the record in parallel
   before it starts Python, keyed by the invocation; the hit then costs tens of
   milliseconds.
2. A compact record, and C toolchain custody keyed by the SDK's own identity
   instead of 69,329 files.
3. Stage-level cutoff: the runtime and backend identity checks (13 s of a warm
   build) reuse the ledger to prove by stat that nothing changed.
4. The runtime build stops running Python: checked-in Unicode tables per
   Unicode version and errno constants per target.
