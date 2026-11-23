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
| Native proof-supervisor store | Artifact root `proof-supervisor/<implementation-source-key>` | Thirty days since last use; existing checkout-state lifetime locks protect provisioning and readers |
| Compile cache | Artifact root `.molt_cache`; unbound SDK user-cache selection has checkout custody | Existing entry locks and last-use byte/age budget |
| Hot state (HF-942) | `~/.molt/hot-state/<artifact-root key>`, named by the artifact root's `hot-state.v1.json` and bound only when home is another volume; mirrors the artifact root's relative paths | Build and scratch admission within the configured mirror require 50 GiB of raw free space. The record's byte budget governs hot compile-cache entries. Checkout Cargo and guard scratch use their existing retention rules. |
| uv, pip, Ruff, sccache caches | Artifact root provider directories | Provider protocols; never unlink uv internals |
| Development environments | Artifact root existing checkout-addressed layout | Existing shared-use locks and last-use policy |
| Browser/Edgebox package environments | Existing checkout-provisioning paths | Lock-file producer admission, shared use through child closure, last-use budget |
| Controller source and environment | Artifact root `molt-state/controllers/<repository>/<tree-key>` | Current plus three recent versions; older versions idle a day, exclusive source-use lock required |
| Landing sandbox | Artifact root `worktrees/landing-sandbox-<repository>` | Queue owns its worktree; runner and integration locks fence migration |
| Lane payload | Artifact root `evidence/lanes/<repository>/<checkout>/<task>` | Durable; checkout keeps coordination/report/index |
| Gate receipts and streams | Artifact root `evidence/integration/<repository>` | Durable; archival compaction retains an index |
| Coordination | Existing checkout `logs/agents` and Git control records | Durable, small records; no age-based evidence deletion |
| Scratch/temp | Existing per-checkout run scratch root | Existing last-use/shared-use protocol for regenerable scratch |
| Provisioned executable toolchains | Canonical `MOLT_TARGET_ROOT`: the repository's custody (a linked worktree's main checkout, `custody_layout.repository_custody_root`), never the volume that holds the worktree | Current pinned versions and live users stay; attested old versions retire by last use |
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

Eligible regenerable state is physically deleted under its existing exclusive
retirement custody. Cleanup does not rename it into another retention queue or
move it into Finder Trash. Its last-use window and live-reader checks still
apply; there is no additional seven-day delay after those checks admit deletion.
The existing daily collector and pressure pass share this policy. Pressure
collection includes the historical `retired/` backlog and checks actual free
space after each deletion against the configured target. Payload-size estimates
do not establish reclaimed capacity, and deletion does not require an extra
recursive size census.

Historical retirement records have a strict reader, with no current writer.
An explicit bounded, resumable backlog pass uses the same typed ownership,
guard, lock and identity checks as automatic collection:

```sh
python tools/disk_guard.py --root "$MOLT_EXT_ROOT" --reclaim-retired --dry-run --json
python tools/disk_guard.py --root "$MOLT_EXT_ROOT" --reclaim-retired --apply --json
```

Preview is the default; `--max-roots` and `--budget-seconds` bound a pass between
deletions. A deletion already admitted, or a required historical activity walk,
can finish after that deadline. Applied passes retain the existing rotation
cursor so unknown or busy entries do not prevent later eligible roots from being
examined. Unproven records, durable evidence and live state remain reported and
protected. Integration evidence keeps its own named retention policy.

The canonical deletion primitive checks fresh native mount information before
unsealing or recursively deleting a directory. A mounted root or descendant,
including a same-device Linux bind mount, or an uncertain inspection refuses
deletion without changing the tree. File and symlink leaves retain their existing
semantics. This adds a mount-table query rather than another payload walk or
process; it is an admission fence, not a lease against a privileged actor changing
the mount namespace afterward. Linux uses `mountinfo`, macOS uses `getfsstat64`
and descriptor-derived path spellings, and Windows uses Python's volume APIs.

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
Path projection and inventory do not refuse a path for startup socket capacity.
`cargo_execution_policy.sccache_auto_selection_allowed` owns that decision: an
unfittable generated cache under `auto` declines with one diagnostic and exports
neither `SCCACHE_DIR`, a wrapper nor an endpoint. Omitting that default preserves
caller intent when the environment is normalized again or enters a daemon digest.
An explicit `MOLT_USE_SCCACHE=1`, explicit cache or sccache wrapper instead raises
`SccacheSocketPathError` before launch; server preparation (`create=True`) checks
the same capacity before writing placement state. A shorter artifact root or an
explicit short `SCCACHE_DIR` restores caching without moving provider data to root.

## Native proof-supervisor decision (HF-651)

`proof-supervisor` is a checkout-state storage class. Its source key names the
implementation checkout, never the admitted guest source, result directory or
payload's Cargo environment. DX consumes the parent's captured artifact root;
the existing no-overlap and native path-budget checks still apply. The store
contains Cargo bootstrap output, tool-identity probes, the immutable executable
CAS and provisioning state. Historical execution evidence keeps its own CAS
copy and does not depend on this regenerable store.

The existing `cargo-target` class selects user/compiler build outputs through
Cargo target and session selectors; `toolchains` selects pinned installed tools
in repository custody. Neither describes source-owned control-plane bootstrap
state on the selected artifact volume. A separate class preserves those
boundaries instead of redirecting a guest's outputs or changing toolchain
custody. Its key is the existing DX checkout component, so it reuses the
checkout-state owner, shared-use, last-use and exclusive-retirement protocol
and daily collector, with no second retention registry or guard exception.
The execution owner retains the store through generation copying and tool
probes; provisioning and generation reads also retain it independently.
Preexisting stores without bound producer provenance remain protected. The
former 24-hex private namespace is no longer produced or consumed; this change
does not authorize deleting its contents. Warm provisioning reuses no-follow
metadata in the existing CAS materializer instead of restating directory
existence and type through separate filesystem calls; directory publication
still fsyncs every new entry and its parent.

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
Managed campaigns with shared Cargo gates default to one persistent checkout
target on the artifact volume. Dependent gates see earlier build outputs at
that same path after the read-only checks and throughout the campaign. An
explicit Cargo target wins. Interactive DX builds continue to use the hot
target when its capacity permits.
The SDK and DX use the same memoized user-state selector; no second cache
default lives in the inventory. Installation home keeps its original user
custody rather than following a new binding for regenerable cache.

### Backend compilation artifacts

The selected compile cache holds per-binary and shared producer namespaces
under `backend-tir-v1/<identity>/`. On first use, the producer stages the
functions directory, disposition tag and trim lock together. The existing
`molt-artifact-publish` owner publishes the complete directory exclusively.
Concurrent writers validate the winner. An interrupted stage leaves the
final name available. Reusing this native publication primitive expands
its downstream crate bound from four to six and keeps the workspace maximum
at 29. Its installed layout contract supplies the tag,
directory name and envelope version to Rust and to the scheduled cache pruner.
Explicitly placed caches keep their caller-selected directory and ordinary
cache tag. A caller that places an explicit cache in the managed tree retains
custody of that preimage.

The scheduled pruner enforces the managed cache's per-namespace and shared
limits and reclaims immutable envelopes after five idle days. It checks the
managed tag, key, kind and declared size. A compiler takes one nonblocking
shared lock for a cache consumer. Scheduled eviction takes the same lock
exclusively and rechecks the observed generation, activity and poison state
before unlinking a file. Successful readers own the copied bytes.
Poison sidecars carry evidence that the compiler produced two different
payloads for the same key. They continue to reject resident values and fresh
publications across capacity trims. Even a zero payload budget preserves a
compact poison record. A reader whose limit excludes a larger incumbent takes
a cache miss and leaves the incumbent available to other readers.

Control files and empty namespace directories retain their generation.
Scheduled eviction opens the published lock directly. Compiles perform
bounded cache reads and publication without directory sweeps.
Historical top-level hash directories, operator data, links and unpublished
stages stay with their original owners. Shared artifacts fill once in the
managed layout when a new producer first needs them. Source builds schedule
retention after publication. The installed compiler dispatches the same pruner after publication in a separate process group.

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

Failed guard scratch payloads are regenerable (HF-694). Closure and exact
payload identity, not a successful exit, grant retirement. The existing
newest-first policy keeps at most three failed payloads and 2 GiB; excess is
deleted directly after last use. A dead guard follows the same path without
archival. The retired guard marker's `temporary_artifacts` outcome and the
landing's existing bounded stream tails carry the failure result; deleting a
scratch generation does not delete the landing receipt. No tool reads a
retained payload. A failed deletion keeps the payload retained and indexed,
records its attempt count with a bounded error sample, and is retried by at
most three guard exits, then
reported as an error until explicit cleanup retries it. Errors in the
shared root, `pending/` and `removing/` namespaces are diagnostic whether
raised or returned, and cannot change that child's exit or become
infrastructure failure. A run whose own index the shared namespace refuses
deletes its payload at once. Current-run custody failures remain failures.

The removed inline compactor traversed, compressed, reread and hashed the whole
scratch tree before a failed guard could exit, then wrote an unbounded file
manifest that its 16 MiB reader could refuse forever. No such archive or source
recovery index is produced now. Explicit dry-run/apply collection in
`molt.temporary_artifacts` resolves old indexed archives and the old
producer's two archive block reasons under the same custody and retention
rules, outside guard exit. A legacy block is a retained failure only after its
terminal receipt, payload identity and allocation receipt verify; evidence in
the record decides, not the owner's schema. Cleanup uses the bounded source
index for coordinates without reading or copying archive content. Unknown
coordinates stay protected and are reported. Every other unindexed block is
held and listed with its bytes, never deleted. The
`scratch-evidence` storage class remains for the distinct legacy-temp
migration producer; it is not guard scratch. Sccache server leftovers retain
their separate evidence contract.

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

Failed scratch uses the same typed lifecycle as successful scratch: terminal
publication, retiring, retained, reclaiming and reclaimed, with blocked state
reserved for interrupted or invalid custody. Archive availability grants no
state transition. Reclamation still refuses identity drift and crash residue.

## Compile cache: keys on recorded reads, immutable records (2026-11-20)

Measured problem (2026-11-19, macOS arm64, data volume): the shared source-record
store held 139.7 GiB of live rows plus a 59.9 GB WAL against the 30 GiB
compile-cache budget: 4,582,015 import-scan, 85,551 module-lowering, 19,516
intrinsic-facts and 19,066 module-analysis rows. Pruning backs off on
`SQLITE_BUSY`, and with lanes running continuously the store is always busy, so
the budget never wins. Checkpoints starve because some reader always holds a
snapshot, so the WAL never resets ([SQLite WAL: checkpoint
starvation](https://www.sqlite.org/wal.html#avoiding_excessively_large_wal_files)).
In one oracle cache, 6,754 import-scan rows held 1,734 distinct results, and 53%
of payload bytes copied the key's namespace premise.

Design, in three slices that land separately:

1. A query's key holds exactly what it reads. The import scan reads its premise
   only through the binding policy and the `__all__` check, so
   `_BINDING_POLICY_FIELDS` (`python_namespace_graph`) drives both the policy and
   the scan view; a premise field stays out of the key until the policy reads
   it. HF-926's read log already cuts off a rescan whose answers hold
   (rustc red/green, Salsa backdating). Rows do not copy their key.
2. Records become immutable content-addressed files (temp file plus rename)
   with a small action index from key to result hash (Go `GOCACHE`, Bazel
   AC/CAS). Readers take no lock and open no database; publication is
   idempotent; equal results share one file. Retention touches a hit at most
   hourly and unlinks by age and size at background I/O priority, so deletion
   never waits for a reader and the budget holds.
3. Module lowering replaces hand-scoped inputs (`_ScopedLoweringInputs`) with
   recorded reads and moves its payloads to the same files; the SQLite store
   then has no consumer and is deleted.

Slice 1 result (interleaved discovery A/B against 207478f220, fresh cache per
run, 2 runs): `import asyncio` import-scan bytes fell from 56.29 MB to 25.07 MB
(-55%), `import unittest` from 52.48 MB to 28.51 MB (-46%); rows (6,265 and
6,596) and distinct results (3,170 and 2,224) did not change; graph and scan
authority dumps were byte-identical in every cold, warm and one-module-edit
phase. Warm discovery CPU fell from 14.1/17.3 s to 10.6/9.2 s (asyncio) and
from 10.9/9.9 s to 7.1/9.1 s (unittest); cold CPU stayed within run spread.
The first 100 programs of `tests/differential/basic` (native, one cold and one
warm run per side, not interleaved) kept the same 98 passes and the same two
failures with identical Molt output; cold wall time was 1,947 s on main and
1,583 s with the change, warm 879 s and 652 s. A whole `molt build` of the
asyncio program did not change measurably (one run each): cold 450/440 s wall, steady no-change 0.41 s, one-module edit
96/102 s. Two costs remain for slices 2 and 3: the first no-change rebuild
after a cold build takes 31-35 s, and a one-line edit of a user module 96-102 s.

Slice 2 (`molt.cli.frontend_record_files`) stores immutable results under
`frontend_records/v1/blobs` and action indexes under `frontend_records/v1/index`.
A frontend operation packs up to 4 MiB of distinct results into a blob, then
publishes the sorted `.idx2` index atomically. Each index row supplies the
result hash and its extent. An HF-926 alias points to the same extent. Readers
continue to admit the original standalone `.idx` rows. A reader orders files
by publication time and seeks for the requested key, reading at most two rows
per step. It checks the requested extent against its result hash, then checks
the exact input identity. On a miss it refreshes the index inventory when the
directory generation changes. The existing retention sweep compacts more than
64 index files. An admitted result renews its blob's mtime at most hourly.
This retains the other members of that bounded blob during the same window.
Eviction lists each blob and index under the compile-cache budget and sweeps
at background disk priority (`setiopolicy_np` throttle, `ioprio_set` idle class,
`THREAD_MODE_BACKGROUND_BEGIN`). The SQLite table's `CHECK` admits only
`intrinsic-facts` and `module-lowering`; the schema change moves it to a new
namespace, so the old store, with its millions of import-scan rows, retires
through the existing superseded-namespace path.

Why index files and not one action file per key: HF-F267 moved these records
from one JSON file each into SQLite because 10,937 file reads cost 1.03 s
against 0.40 s. Warm reads of 6,955 real asyncio import-scan payloads (78 MB,
loaded host) cost 3.9-5.3 us each from bare SQLite, 19-25 us from one file per
key, 39-52 us from an action file plus a blob, and 14.3 us from a loaded index
plus one blob `os.read`; the store's real keyed read was 24 us (HF-F267).

Slice 2 result (interleaved discovery A/B against 132ef07f42, fresh cache, 2
runs): `import asyncio` stored 21.37 MB in 3,168 blobs and 2 index files against
25.09 MB in 6,259 rows; `import unittest` 20.80 MB in 2,220 blobs against
28.55 MB in 6,585 rows (aliases and equal results share blobs). Warm and
one-edit discovery CPU stayed within run spread (asyncio warm 11.0/9.4 s on main,
10.7/9.4 s with the change); graph and authority dumps were byte-identical in
every phase.

On 2026-11-21 the shared index had 1.71 million rows across nine files. A
three-file application requested 133 distinct addresses. Bounded reads
returned identical digests to a full parse in 14–18 ms versus 1.73–1.83 s in
the isolated lookup comparison, with about 25 MiB versus 1.1 GiB peak RSS.
Two interleaved native dev edits per variant used the same cache, code, target
and output. The only switched component was the index reader. Import discovery
fell from 1.36–1.39 s to 121–125 ms. The full diagnosed build fell from
3.12–3.34 s to 1.84–1.86 s. Each build lowered one changed module, reused eight,
and produced stdout identical to managed CPython 3.12.16. This is a local
macOS arm64 result. [Git's pack index](https://git-scm.com/docs/gitformat-pack)
uses the same ordered-search principle over fixed-width IDs.

Prior art: rustc incremental queries (dep graph, red/green), Salsa (tracked
reads, revisions, backdating), Zig incremental compilation (InternPool,
fine-grained dependencies), Swift's request evaluator, Buck2 DICE, Go
`GOCACHE` (action ID to output ID, content-addressed files, hourly mtime),
Bazel AC plus CAS, uv's content-addressed store with clonefile/hardlink,
ccache and the Nix store.

## Hot state and capacity (HF-942, 2026-11-20)

Measured: the external data volume was 96% full (81-92 GiB free of 1.8 TiB),
about 900 ops/s at saturation, a 100-byte read plus a one-line append took
17.8 ms (median) and up to 0.53 s, and creating a lock in the compile cache's
lock directory took 76.7 ms (median, 157k entries). Every regenerable class
had `max_bytes: null`, so nothing reacted to bytes or free space.

1. Locks: the publication, compile and checkout-state families name one of
   4,096 stripes (`default_paths.lock_stripe`) instead of one file per key,
   so the directories stay bounded. No path takes two keys of one family at
   once, so a stripe can only serialize two holders. The daily cache
   retention and `storage trim` unlink a retired per-key file only while
   holding it exclusively, after a day idle, and only if the name still names
   the held inode. A lane on an older commit can still name a per-key lock in
   the transition; checkout-state retirement also requires the root's last
   use to be old, so the lock is a second fence there, not the only one.
2. Placement: `default_paths.hot_state` reads one record in the artifact root
   (`storage migrate` writes it when home is another volume). Lock
   directories, the file-digest store and the frontend record files resolve
   below it under their artifact-relative paths (`hot_path`), so placement
   changes a prefix and never a name. The development write check
   (`dx.storage_write_check`) admits each artifact claim at the same relative
   path below the hot root. Without a record (tests, plain clones,
   one volume) nothing moves. Guard scratch (`tmp/gs/v2`: each guarded run's
   receipts and its TMPDIR lease target) moves the same way
   (`temporary_artifacts.scratch_root`). A guarded no-op's finalization took
   a median 20.9 ms (p90 55.6, max 89.9) on the data volume and 9.4 ms (p90
   11.5, max 12.4) on the internal disk, 25 interleaved pairs under live gate
   load. An explicit `MOLT_SCRATCH_STORAGE` root keeps its placement. Live
   scratch is transient and retained failures stay bounded (3 generations,
   2 GiB); `storage trim` retires terminal generations left on the former
   root. The integrator owns the Python build environments' placement.
   Reusable checkout Cargo targets move the same way
   (`dx.cargo_target_dir_for_artifact_root`), beside the kache store, so a
   restore clones instead of copying: a full molt-runtime dev-fast restore
   into a fresh target took 8.9-11.6 s on the internal disk and 73.8-200.1 s
   on the data volume (kache's no-copy total rose 1.9 GiB for the two
   internal restores). Explicitly pinned Cargo sessions stay on the artifact
   volume. Generated run IDs keep the reusable target. A managed landing
   batch with shared Cargo writers checks the default hot target first. When
   headroom is short, the bounded pressure reclaimer protects the checkout's
   warm target and reclaims eligible artifacts before capacity is measured
   again. If space is still short, the batch selects its checkout target on
   the artifact volume. Each batch makes a fresh selection and pins it for
   its children. Explicit targets retain their placement. Gates without shared
   Cargo writers project the path without a Cargo capacity check.
   Managed retirement and
   orphan-daemon cleanup see the checkout's targets on both volumes, including
   any previously placed sessions. Live stamps touch only the selected target,
   its enclosing checkout target when pinned, and its control. The disk guard
   governs a root's hot mirror with the
   root (`reclaim_roots`). A target's toolchain partitions
   (`.molt-toolchains/<digest>`, one per external-input identity) other than
   its newest retire after 7 idle days under the target's exclusive lock.
   Under the standing volume free-space floor, the existing collector uses
   the one-day idle window. It refreshes partition activity and volume pressure
   under that lock before removal. Partition and quarantine planning share one
   Cargo target inventory and reuse partition activity for candidate ordering.
   Slot-07's target held 9 partitions, 22.9
   GiB of its 32.0 GiB. The product store (97.1 GiB) stays on the data volume
   until it has a byte bound on the hot volume.
3. Capacity: every Molt volume keeps 20% free (`disk_capacity.FREE_SPACE_FLOOR`).
   Below it, checkout state (the disk guard's daily GC, reason
   `free-space-floor`) and the regenerable classes `storage trim` sizes give up
   entries a day idle by last use, rechecked after each removal; the day keeps
   a volume shared with other data from making every lane rebuild cold. The
   disk guard's absolute headroom stays the emergency trigger. Admission of
   build and scratch output within the configured hot mirror requires 50 GiB
   of raw free space. Cargo resolves the artifact binding from its working
   directory, including direct builds without an exported artifact root.
   Guard scratch carries its selected artifact root from placement into
   admission, using one binding lookup. Before a managed gate starts, a
   measured shortfall on the selected hot output volume invokes one pressure
   pass through `disk_guard.ensure_free`. The admission boundary measures the
   selected outputs again and starts the gates only when they pass. Explicit
   external destinations are admitted as selected. Detached collection also
   measures the bound hot mirror against its raw reserve independently of
   artifact volume headroom. Each pressure pass retires eligible state on its
   measured device and checks live guard custody across the whole family.
   After result publication, a measured hot shortfall can wake the existing
   detached worker for pressure only. Daily maintenance and pressure-only
   passes share a lock, and pressure attempts have a 15-minute cooldown
   without advancing the daily stamp.
   Already admitted writers can continue allocating. The compile-cache cap
   covers its own entries. Lane start only reports a volume below its
   floor (one statfs): a synchronous trim there stalled lane starts. Checkout
   state carries no byte budget because sizing it walks hundreds of GB (the
   `target/checkouts` census did not finish in 10 minutes); the floor is its
   cheap capacity control.
   Bound on the integrator's host 2026-11-21 while lanes built. The same
   probe build went from 23.3-35.9 s warm to 26.2 s once cold, then 10.2-11.0 s
   warm (backend_prepare 13.2 -> 5.6 s, link 4.6 -> 1.0 s).
4. Admission: differential and Cargo job counts scale by
   `20 ms / measured latency` of a 100-byte read plus a one-line append in the
   run's temporary directory (`resource_pressure.disk_admitted_jobs`).
