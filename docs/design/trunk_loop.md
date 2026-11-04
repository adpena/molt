# Trunk proof on the farm

Stage 1, 2026-10-29. Integration owner: Claude integrator. The worker owns
farm proofs; landing, ledger edits and lane retirement remain with the integrator.

The 2026-11-03 owner correction prioritizes Linux provisioning, then opt-in
remote execution of the existing landing gates. The queue, runner lock,
`last-green`, results and push remain on the Mac; no GitHub self-hosted runner
is registered. Linux execution does not establish Mac or Windows qualification.

## Decision and boundary

`tools/trunk_loop.py run --host bat00 --commit COMMIT --cell ID --cell ID`
transfers an exact Git commit, admits the selected proof commands' prerequisites,
launches a detached host worker and collects its evidence. `collect` resumes
observation after controller interruption. Stage 1 ends with the two named Windows
cells and disconnect survival proved; continuous scheduling and failure routing
are Stage 2, not implicit claims of this implementation.

`tools/cross_hosts.toml` remains the only host inventory. Its typed host records
declare OS, shell, custody root, checkout, launcher and permitted proof commands.
The smoke runner and trunk runner share SSH transport and nonce-owned staging.
No task is a shell command supplied by an operator: the selected commit's
`tools/proof_plan.toml` owns argv, dependencies, budgets and toolchain closure.
Commands run serially on a host; a failed cell retains its result and does not
authorize changing workers, deadlines or selecting another workload.

The canonical checkout is a Git object donor, never reset or moved. A new
detached worktree is allocated under its custody root for each run, using a
controller-created bundle and bounded Git-transfer semantics. Staging and
worktree identities are retained in a local run record before launch. No source,
toolchain, environment or seal is placed on bat00's exFAT volume. Existing
`E:\\Molt\\trunk` and the hand-written scratch runner are donors for evidence only;
the integrator reconciles and retires them after landing.

## Provision before testing

`tools/trunk_loop.py provision --host NAME` automates the
[documented local install](../getting-started.md#local-repo-workflow).
uv owns bootstrap Python and dependencies; the inventory cannot select a
hand-made interpreter. Host-side Python entry points use
`uv run --locked python -B` in the selected checkout. The existing immutable
development-image publisher still owns proof environments and their admission.
Provisioning requires CMake among its system prerequisites and runs
`molt doctor --json --strict`: a JSON error report cannot count as a successful
install merely because the report-only doctor mode exits zero.

`molt-gate` is Linux x86_64 behind bat00's Windows SSH endpoint. Scripts enter
`wsl.exe -d molt-gate -- bash -s` through stdin, avoiding Windows re-quoting of
POSIX scripts. Binary files travel through tar on stdin and literal `cat`
collection, under the same guarded SSH owner. POSIX workers create a new
session; the declared `vmIdleTimeout=-1` must already match before provisioning.

The inventory declares the WSL VHDX's C: identity, 80 GiB disk budget, 24 GiB
memory, 4 GiB swap and a minimum 20 GiB proof-build headroom. Admission verifies
the distribution's registered VHDX path and reads free space through `/mnt/c`,
never the virtual ext4 free-space report. A mismatched Windows profile stops
before installation. Under the owner's 2026-11-02 approval, `provision` caps
`molt-gate` at 80 GiB, trims its filesystem and compacts its VHDX. Root used bytes
(including Cargo/uv caches) plus 20 GiB build headroom must fit the cap. Only
`molt-gate` is stopped. With the correct block-device size a subsequent run
performs no disk mutation; `run` admits but never changes the cap. The declared
`vmIdleTimeout=-1` retains WSL's internal attachment after distro termination.
Disk maintenance releases the idle utility VM only after `--list --running
--quiet` proves no distribution is running; it refuses if another distro is
active. Neither
`.wslconfig` nor sparse mode is changed.
A pending marker beside the registered VHDX survives an interrupted shrink or
compaction. It is removed only after successful compaction, so a resumed
provision cannot mistake a resized disk for completed disk maintenance.

The reviewed [WSL 3.0.3 resize implementation, lines 1813-1870](https://github.com/microsoft/WSL/blob/3.0.3/src/windows/service/exe/LxssUserSession.cpp#L1813-L1870)
checks that the VHD is detached, resizes ext4 first when shrinking, ejects it,
then shrinks the VHDX. Windows' unsafe-size flag follows the successful ext4
resize: Windows cannot itself validate ext4's safe minimum. The
[client, lines 902-976](https://github.com/microsoft/WSL/blob/3.0.3/src/windows/common/WslClient.cpp#L902-L976)
routes `--resize` and `--compact` separately.
[Compaction, lines 1873-1922](https://github.com/microsoft/WSL/blob/3.0.3/src/windows/service/exe/LxssUserSession.cpp#L1873-L1922)
uses the registered disk and offline discard before VHD compaction. Provisioning
also runs `fstrim -av` as root before stopping and compacting the distro.
Unknown WSL versions are refused before disk mutation. This implementation
supports shrinking, so no used-bytes-only substitute is needed.

## Opt-in remote landing gates

Start the Mac landing runner with `MOLT_LANDING_GATE_HOST=molt-gate` to execute
its selected gate cohorts on the admitted Linux host. The default remains Mac
execution until the owner has qualified ten green farm landings; no automatic
promotion or second landing counter is introduced. Queue records, locking,
candidate construction, merge, push and lane retirement stay with the Mac.

The Mac serializes the selected `SelectedGate` values and their toolchain union.
The trunk runner transfers the exact commit, then the Linux worker prepares
those gates through the normal `GateConfig` and `run_selected_gates` authorities.
A missing remote prerequisite refuses the run; it cannot silently drop a gate
or retry it locally. The worker has no publication operation. The collected
receipt binds the commit, tree, clean state, exact request digest, admitted
toolchains and all selected outcomes. The Mac verifies that binding and checks
its own source again before its existing push boundary. Candidate failures stay
candidate failures; unavailable hosts, unknown completion and incomplete receipts
stay infrastructure failures. Transport logs, per-gate streams and executor
receipts remain in the trunk run's evidence custody. This opt-in changes the
execution platform to Linux; it does not prove platform-specific Mac obligations.

Proof workers select one host-owned `cache/cargo-workspace` Cargo target across
run worktrees. Existing guarded-controller cache retention remains enabled;
its budgets cover classified evictable entries, not a hard quota on all Cargo
outputs. Reuse and retention qualification on the live hosts remain separate
from this path selection. Existing hand-made environments remain in custody
until the replacement passes on their host.

The selected command/dependency closure supplies required toolchains. Existing
`ProofPlan`, `toolchain_env.selected_toolchains`, `molt.checkout_provisioning`
(which owns locked `uv sync`) and `proof_plan.admit_toolchains` retain admission.
Pinned releases use `molt.tool_releases`, not a second downloader or installer.
Unsupported or absent providers fail provisioning before any test starts.
Persistent Linux volumes also need the unique filesystem UUID mapping required
by [durable file identity](durable_file_identity.md). WSL images without
`/dev/disk/by-uuid` cannot run guarded doctor/proof commands. Under the
2026-11-04 correction, provisioning declares `[boot] systemd=true` in
`molt-gate`'s `/etc/wsl.conf`, preserving other sections and publishing only
changed bytes. This follows Microsoft's
[documented WSL systemd mechanism](https://learn.microsoft.com/windows/wsl/systemd#how-to-enable-systemd).
It restarts only `molt-gate` when the declaration changes or PID 1 has not yet
become systemd; it never restarts another distribution. An active farm-worker
lock refuses the restart. Provisioning and proof-run admission wait for udev
and require the root filesystem's real `/dev/disk/by-uuid/<UUID>` block-device
mapping. A missing or mismatched mapping refuses with its path before install
or proof staging. The systemd/udev packages are host prerequisites; no UUID link
is fabricated and no durable-identity contract is changed (HF-761 owns the
udev-free Linux provider).
The inventory also selects the two short, disjoint fixture-output roles that
`ci.yml`'s portability setup supplies. Each run exclusively claims a child and
admits it through `proof_queue_pkg.cargo_output_layout.declare_root` before tests.
Neither the toolchain closure nor DX's general scratch defaults declare these
native fixture roots; their absence is harness failure, not a product red.
This does not claim whole-suite product preparation (HF-586); future cells must
declare their actual product dependencies in the proof plan before selection.

Non-bootstrap tools use **controller-seeded pinned archives** where the earlier
farm runs established incomplete host certificate chains. uv is installed by
its official host installer and is never seeded. The controller caches each selected
host asset by pinned SHA-256 and size, transfers it, and the host imports it
through the same size/digest/publication boundary as an HTTPS download. No
certificate checks are disabled or system settings changed. This removes the
farm's dependency on its incomplete OpenSSL chain construction for github.com.
An explicit host download remains available through the existing verified HTTPS
path and exact pin; it is not a retry after a failed seed. OS verification would
repair chain building for unrestricted host downloads, but requires a separately
admitted dependency before bootstrap and does not provide offline input custody.

## Execution and evidence

One launcher interface uses a new session on POSIX and
`Invoke-CimMethod Win32_Process.Create` on Windows. CIM starts outside the SSH
session's job, so closing or killing the SSH client cannot terminate the worker.
The farm worker reuses `tools.detached_execution._exec_wait_rc` for proof-driver
waits and managed-state leases. Its unresolved-child outcome retains the host
lease and publishes no terminal completion, matching that authority.
The worker redirects binary stdout/stderr directly to `run.log`, writes `cmd`,
per-cell `rc`, JUnit and the canonical proof receipt, then publishes terminal
status last. The controller records launcher acknowledgement separately from
completion; missing terminal evidence is unknown, never success. Collection
retains original bytes and hashes in
`<artifact-root>/evidence/trunk-loop/<commit>/<host>/<cell>/<run>/`, preserving
older results. Recoverable records name exact remote allocations. Nothing
deletes resources while process closure is unknown.

Run-record schema v2 removes the inventory interpreter field. Retained v1
records remain with the exact v1 controller named in their record; v2 does not
silently reinterpret their host contract.

JUnit and pytest duration reporting are diagnostic additions through the pytest
environment; they do not change selectors, workers or timeouts. Canonical proof
receipts remain the source of guard outcomes. Stage 2 will consume
`ci_triage --junit --json` as its only JUnit failure parser, adopt HF-422's
completion/guard/policy/parent-key corrections, and propose `ledger_edit.py`
commands plus owning-lane input files. There is no automatic revert.

## Reconciliation and prior art

The parked postsubmit donor `37e82b522c34` supplies useful exact-source and
single-runner concepts; its revert policy and landing-policy edits conflict with
the owner-approved fix-forward workflow and are not imported. The triage donor
`b3e790d89462` and its owning design remain preserved for Stage 2; importing its
second JUnit parser or uncorrected review-v2 behavior would duplicate authority.

- [LUCI Swarming bots](https://chromium.googlesource.com/infra/luci/luci-py/+/HEAD/appengine/swarming/doc/Bot.md)
  separate task custody from execution and retain a content-addressed cache
  across tasks. Stage 1 borrows that separation without deploying a service.
- [Chromium sheriffs](https://www.chromium.org/developers/tree-sheriffs/) and
  [Sheriff-o-Matic](https://chromium.googlesource.com/chromium/src.git/+/28cd42b49b5f93a409d6daf04e8258d569e8ff47/docs/sheriff.md#sheriff_o_matic)
  group actionable failures for owners; proposed ledger edits serve that role.
- [Observed inputs and content identity](observed_inputs.md) owns the
  postsubmit role and its prior art (TAP, LLVM). Here a red keeps its commit and
  parent identity instead of blaming the latest author, and no culprit claim
  follows from a single failing run.
- [Rust bors](https://forge.rust-lang.org/infra/docs/bors.html) separates merge
  ownership; [crater](https://github.com/rust-lang/crater) compares explicit
  experiments and records platform limitations. The farm does not own landing.
- [Bazel repository cache and distdir](https://bazel.build/run/build#repository-cache)
  reuse hash-identified downloads, as the seeded archives do.
- The owner's [Spawn write-up](https://x.com/jsnnsa/status/2115623382721126826)
  (2026-10-29 dispatch: trunk every 15 minutes, fix-forward, judges) informs
  Stage 2's bounded cadence and routing. The original URL was located through a
  secondary index, but its body was inaccessible to the research client; those
  details remain owner-provided requirements, not independently verified claims.

## Proof obligations

Provisioning tests use fake transports for each inventoried OS, reject settings
drift and insufficient physical headroom before installation, and execute the
POSIX script twice against fake tools to check that an existing installation
does not reinstall prerequisites or clone another checkout.

Unit controls must reject corrupt, truncated or substituted archives without
network access or publication; execute a real POSIX detached child across parent
exit; and verify Windows CIM command quoting and failure acknowledgement against
its public API. The real bat00 disconnect experiment is the Windows lifetime
oracle. Both named proof cells must return source-bound receipts, even when the
tests are red. No broader suite or repeated matching PASS is required here.
