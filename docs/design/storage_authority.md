# Molt storage authority (HF-582)

`molt.custody_layout.StorageClass` is the closed state identity. Its exhaustive
`storage_rule` owns default volume assignment, producer and retention. `molt.dx`
resolves `StorageLocation`, and `storage_path` projects its path. The existing
checkout-state table is a projection of these rules, not a second inventory.
`molt.disk_capacity` remains the admission boundary, `molt.checkout_state`
owns lifetime locks and last-use records, and `tools/molt_dev.py storage` is
the operator entrypoint. Source custody and toolchain custody do not change
when a capacity-selected output volume changes.

## Placement and retention

| State | Placement | Lifetime |
| --- | --- | --- |
| Cargo outputs, product store | Existing DX target and product selectors | Existing build-output retention and 25 GiB admission floor |
| Compile cache | Artifact root `.molt_cache`; unbound SDK user-cache selection has checkout custody | Existing entry locks and last-use byte/age budget |
| uv, pip, Ruff, sccache caches | Artifact root provider directories | Provider protocols; never unlink uv internals |
| Development environments | Artifact root existing checkout-addressed layout | Existing shared-use locks and last-use policy |
| Browser/Edgebox package environments | Existing checkout-provisioning paths | Lock-file producer admission, shared use through child closure, last-use budget |
| Controller source and environment | Artifact root `molt-state/controllers/<repository>/<tree-key>` | Current plus three recent versions; older versions idle a day, exclusive source-use lock required |
| Landing sandbox | Artifact root `worktrees/landing-sandbox-<repository>` | Queue owns its worktree; runner and integration locks fence migration |
| Lane payload | Artifact root `evidence/lanes/<repository>/<checkout>/<task>` | Durable; checkout keeps coordination/report/index |
| Gate receipts and streams | Artifact root `evidence/integration/<repository>` | Durable; archival compaction retains an index |
| Coordination | Existing checkout `logs/agents` and Git control records | Durable, small records; no age-based evidence deletion |
| Scratch/temp | Existing per-checkout run scratch root | Existing last-use/shared-use protocol for regenerable scratch |
| Provisioned executable toolchains | Canonical `MOLT_TARGET_ROOT` | Current pinned versions and live users stay; attested old versions retire by last use |
| Guard/pytest run control | Existing artifact-root temp control namespaces | Durable diagnostics under existing guard custody |
| Benchmark reports | Artifact root `bench/results` | Durable; source baselines remain source inputs |
| Backend sockets | Existing short host socket selector when required by OS path limits | Existing daemon liveness; no payload-cache exception |
| User installation | Existing selected installation root | User owned; never automatic deletion |
| External provider homes | User home defaults; existing Cargo/rustup and npm configuration selects actual paths | Provider owns configuration, payload and collection; inventory reports sizes |
| Replaced controller, Git sandbox, family sandbox, receipt and lane layouts | Their original named coordinates | Last use plus native/lifetime/queue locks; archive signal before retiring raw storage or Git registration |
| Replaced Molt user cache and host temp | Original user-cache selector and explicitly inventoried host temp | Thirty days for old cache, six hours for old temp, with a two-hour minimum grace; lossless archive and index before raw retirement |

The existing daily collector admits auxiliary retention under its own lock;
compile-cache retention keeps its existing provider cadence. No second
scheduler or admission stamp is introduced. Toolchain providers publish owner
and use records only after exact validation, and hold the same external
lifetime locks during download, extraction, publication and read use.
Controller preparations receive their producer record before extraction.
Unpublished stages use the same source-use lock, and never displace recent
complete versions in retention. Unknown stages remain for reconciliation.
Shared locks cover use, including read-only use, through child closure.
Retirement takes the matching exclusive lock and rechecks the use record.
Fresh provider cache roots receive provenance at their creation boundary;
existing unrecognized payload is never adopted. Owner disappearance is not the
only retirement trigger. Missing or malformed
provenance does not authorize adoption as disposable state. Byte budgets are
soft while current versions, recent users or locks protect data; tools report
residual storage instead of violating liveness.

Evidence compaction creates a gzip tar archive without following links,
hashes each regular payload from the archive against its source, publishes the
durable external manifest and checkout index, then removes the represented
source payload. Small selected coordination/report files remain. Interruption
can retain duplicate bytes, but every removed payload has a recovery index.
Later compactions point to their previous external manifest, so the recovery
chain keeps earlier archives discoverable. Historical soft retirements of
evidence no longer expire into hard deletion. Retired guard markers also keep
every record; moving them out of the active namespace bounds active-reader
cost without deleting history or scanning it on each retirement.
Generic cleanup preserves evidence, unrecognized provider caches and selected
user installations; an extra path cannot escape its declared build outputs.
Explicit checkout retirement holds registry/package leases, refuses live
coordination and guarded source, verifies an archive when ignored/WIP payload
exists, and publishes a recovery index in the surviving common Git directory.
Raw source stays until Git removal succeeds. No test applies a migration to
the running repository.
Coordination lifetime locks for a linked checkout live in the surviving family
root, or its common Git directory for an isolated checkout, rather than
creating state inside a checkout being removed. Package
environments use the same checkout-state lock namespace and descriptor handoff
as Python environments.

The volume assignments are artifact, checkout custody, user home and historical host temp. Checkout custody
retains the existing executable-toolchain, user-installation and short host
socket selectors; moving outputs never silently reassigns those owners.
Existing build-output age/pressure policies keep their own execution mechanism;
an absent byte limit is explicit, rather than an unenforced invented budget.
Controllers and the single queue sandbox use version/lifetime limits rather
than an invented byte cap. Native sccache owns its cache and per-probe temp
cleanup. Its shared server outlives run scratch, so startup temp stays beside
its cache when the socket fits. A new deep cache records its short artifact-root
placement once in a durable cache-adjacent index, under an OS lock. Later
clients read that same assignment even when their run context changes.
Unassigned existing cache/temp payload and live or uncertain native endpoints
block assignment; the operator reconciles those preimages explicitly. This
record assigns placement and never authorizes deleting native provider data.

## Toolchain volume decision

The 2026-10-29 slot-06 measurement found both external Samsung SSDs negotiated
at 480 Mb/s. A 191,343,864-byte Clang 22.1.8 executable took 5.83 s to copy.
The first `--version` invocation took 36 ms on root and 1.27 s on the data
volume. Nine interleaved warm object compilations had medians of 23.1 ms
and 23.2 ms. This is a first-execution observation after copying and warm
steady-state evidence, not a controlled cold-cache benchmark. Keep executable
toolchains on root on this host; repeat the decision after fixing the link.
The raw commands, results and hardware report belong in lane evidence.

## Migration and operations

`storage migrate --dry-run` inventories only the old Molt producers. Applying
the reviewed plan is an integrator action, separate from deploying new path
selectors. It never enumerates other projects, Codex worktrees, VM disks or
snapshots. A locked landing stays. Unregistered temp/cache preimages require
custody reconciliation; names alone do not prove they are safe to retire.
Each replaced layout has its own `ReplacedStorageClass` in the same closed
storage enumeration. Migration consumes those typed source selectors; it has
no second table of former locations. A standalone clone's former sandbox stays
outside the source checkout, just as the original producer placed it. Selecting
a replaced coordinate grants no new write custody. The daily collector uses the same finite inventory and
last-use policy for repository-owned preimages. Historical user-cache and
host-temp preimages require an explicit operator scope (`storage migrate`, or
`storage trim --legacy-temp-root`); an arbitrary SDK/test checkout cannot adopt
an account's global history. Old temp names select signal for preservation,
not disposable data. Full recursive metadata observes payload reads and writes;
held native locks, guards, sockets and recent use retain the source. Raw bytes
are collected only after lossless archive verification. Source indices remain
durable in surviving Git custody, allowing even empty legacy temp directories
to leave the host temp namespace. Empty directories are not mistaken for
already-indexed evidence. Native daemon fixture `moltd-*` paths share this
legacy class; unknown producer prefixes and links are reported and retained.
Custom historical temp roots must be supplied explicitly.

A superseded registered queue sandbox must be detached, clean of source changes,
idle for a day, and free of runner/integration and lifetime locks. Its complete
signal is archived and indexed in surviving Git custody before Git releases the
worktree registration. The complete tree must still match the archived snapshot
before removal; new or changed signal preserves the source. Dirty, locked, branch-holding and unregistered sources
stay. The existing relocation primitive still supports selected clean worktree
moves, including verified cross-volume publication; superseded registration
retirement is a distinct operation. Controller environments regenerate at their
selected location, since moving a venv would leave absolute script paths stale.

The integrator's `MOVED.txt` protocol is accepted as an existing transfer: its
single destination must be the named artifact-volume lane directory and every
listed payload must exist. Planning reports `already-moved` and preserves that
index. Missing destinations or listed entries remain unknown/protected; no
fallback destination is invented.

Lane handoff and retirement archive unknown ignored signal into the canonical
lane payload class using `task_artifacts_dir`. The archive is independently
verified before raw removal. Source recovery indices move into common Git
storage alongside a batch index, so even an ignored file whose JSON sidecar
would be untracked leaves a clean slot. Git can name an ignored child and its
whole directory; the harvest reduces these to disjoint maximal roots. Unknown
signal no longer blocks a handoff merely because no producer can regenerate it.
An ignored symlink is archived as its namespace entry through the same
compactor, without following or adopting the target. Ordinary owned-root
compaction retains its stricter no-link boundary. Replaced-layout pointers
remain protected preimages; they never grant collection custody over a provider.
Live or malformed lock custody still requires reconciliation.

The autouse owning-test check observes actual Python file mutations against
typed claims, including code bypassing `storage_path`; an unknown artifact-root
sibling fails. `ty` exhaustiveness rejects an added state identity without its
policy. Typed selectors admit deliberate per-call overrides, and a caught
violation still fails the enclosing check. This is a Python development check,
not an OS sandbox for external
native tools or emitted user programs.

Landing gates receive `RunContext.canonical_env`, including `UV_CACHE_DIR`,
`MOLT_CACHE`, provider caches and scratch `TMPDIR`/`TMP`/`TEMP`. Temp selection
replaces an inherited system-temp value. Pytest's early bootstrap uses the same
scratch selection, materializes its owned root, and updates `tempfile.tempdir`
even if a preceding import cached the host default. This applies to xdist
workers without changing product-store fixture scope. Their interpreter
and bytecode policy still come from the existing gate/image owner.
Actual gate preparation materializes the admitted environment snapshot after
capacity checks, including provider provenance, without reselecting its image
or roots. A dry run and a capacity refusal do not create output directories.
The SDK and DX use the same memoized user-state selector; no second cache
default lives in the inventory. Installation home keeps its original user
custody rather than following a new binding for regenerable cache.

## Upstream decisions reused

* [Cargo's 1.78 development report](https://blog.rust-lang.org/inside-rust/2024/03/26/this-development-cycle-in-cargo-1.78/)
  records stabilization of last-use tracking. Cargo's
  [GC implementation](https://doc.rust-lang.org/stable/nightly-rustc/cargo/workspace/gc/index.html)
  and [cache configuration](https://doc.rust-lang.org/cargo/reference/config.html#cache)
  separate use tracking, deletion eligibility and `cache.auto-clean-frequency`.
  Tracking began in 1.78; automatic cleanup/configuration did not all stabilize
  in that release. Molt reuses last use and a bounded collection cadence.
* [Go's build cache](https://go.dev/src/cmd/go/internal/cache/cache.go) coarsens
  read-use timestamp updates and trims periodically. Molt retains its existing
  one-hour stamps and two-hour grace; old creation time alone is not last use.
* [Bazel disk-cache GC](https://bazel.build/remote/caching#disk-cache)
  combines disk-cache size and age limits. Molt budgets regenerable payload
  while keeping current/live roots; durable evidence has no deletion budget.
* [uv cache safety and pruning](https://docs.astral.sh/uv/concepts/cache/#clearing-the-cache)
  require provider-owned pruning. Molt invokes `uv cache prune` and reports a
  remaining over-budget cache instead of manipulating uv's internal objects.

Failed guard scratch is evidence, even when its raw retention cache expires.
The generation lifetime lock pins a lossless, independently verified archive of
payload and terminal records; its durable external index is published before
raw reclamation. New allocations capture their evidence placement at admission.
Legacy failures lacking that receipt remain untouched for explicit reconciliation.
An unavailable archive leaves raw bytes intact. Successful scratch remains
regenerable. Sccache server leftovers have a separate durable evidence class:
native per-probe cleanup does not grant Molt permission to delete crash signal.

Recovery-index names are reserved durable control coordinates. Collectors skip
them, and the compactor refuses to treat an index itself as raw payload.
After verification and publication, represented source subtrees use the
canonical sealed-tree deletion boundary; directory seals may be opened within
exclusive custody, while external link targets and file modes stay unchanged.
An unavailable archive defers the soft raw-evidence budget and preserves its
signal without invalidating a command whose process closure is established.

## Shared-provider decision (integrator, 2026-10-29)

`external-provider` is one closed typed storage class: default volume is the
user's home, owner is the native provider, and retention is
`ExternalProviderRetention`. Cargo home, rustup home and npm cache are its
members. Cargo/rustup retain their existing canonical selectors. npm resolves
its own configuration through an offline `npm config get cache` probe, including
npmrc and environment precedence. An unavailable provider, failed configuration
probe or incomplete size observation is reported as unknown, without inventing
a path. Member sizes are observations, not Molt-reclaimable bytes.

Molt never redirects, migrates, adopts or prunes shared provider homes. They
contain compiler inputs, credentials, configuration and installed commands used
by other projects; redirecting them would change input and tool custody.
Observing the class also grants no Python storage-write claim over the user's
home. This decision closes the ownership enumeration, without asserting a
finite Molt-enforced byte or age budget for provider state.

[Cargo home](https://doc.rust-lang.org/cargo/guide/cargo-home.html) documents
configuration, credentials and installed commands alongside cached dependencies.
The current [Cargo cache policy](https://doc.rust-lang.org/cargo/reference/config.html#cache)
tracks last use and normally checks daily on build/fetch commands. Downloaded
cache data expires after three unused months; locally regenerable data after
one month. Deletion is disabled with `--offline`/`--frozen`, and provider/user
configuration can disable automatic collection. These rules cover Cargo's
global dependency cache, not installed commands, credentials or build outputs.

[npm's cache contract](https://docs.npmjs.com/cli/v11/commands/npm-cache/)
states that `npm cache verify` checks index/data integrity and collects unneeded
data. npm does not automatically remove unused cache data by age, so no such
bound is claimed; provider/user invocation owns collection. The
[rustup installation contract](https://rust-lang.github.io/rustup/installation/index.html)
separately owns installed toolchains and configuration under `RUSTUP_HOME`.
Toolchain removal is the provider/user's
[`rustup toolchain uninstall`](https://rust-lang.github.io/rustup/examples.html) action;
Cargo's cache GC is not a rustup toolchain collector.

A failed scratch generation publishes a blocked state before its payload move
and remains blocked through archival. Only a durable verified archive grants
retained/reclaimed state, while the parent still holds the lifetime lease.
Older collectors therefore cannot retire newly produced unarchived failures.
Interrupted transitions and absent-target diagnostic records use the same
publication gate. An archive failure leaves blocked evidence for reconciliation;
ordinary retention verifies and reuses the archive before removing raw bytes.
