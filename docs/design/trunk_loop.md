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

The canonical checkout is a Git object donor: a run never resets or moves it.
Only provision moves it, to the commit it provisions, and never over tracked
edits ([Provision before testing](#provision-before-testing)). A new detached
worktree is allocated under its custody root for each run. A controller-created
bundle carries the commits the host lacks, with bounded Git-transfer semantics.
When the checkout already contains the subject and the controller, no bundle is
made; git refuses an empty one. Staging and
worktree identities are retained in a local run record before launch. No source,
toolchain, environment or seal is placed on bat00's exFAT volume. Existing
`E:\\Molt\\trunk` and the hand-written scratch runner are donors for evidence only;
the integrator reconciles and retires them after landing.

## Provision before testing

`tools/trunk_loop.py provision --host NAME --commit COMMIT` automates the
[documented local install](../getting-started.md#local-repo-workflow) at
`COMMIT`. The controller resolves `COMMIT` to a full id and refuses it unless an
`origin` branch contains it, since the host fetches from origin. The host then
fetches and checks out that commit before `uv sync` and doctor. A fetch alone
leaves the checkout at whatever commit it held: on 2026-11-04 the macmini's
doctor judged pre-fix code and reported an error that main had already removed.
The farm checkout is tool-owned. Provision refuses a checkout with tracked edits
(exit 2 on POSIX), because doctor would judge them instead of `COMMIT`, and it
never discards them.
uv owns bootstrap Python and dependencies; the inventory cannot select a
hand-made interpreter. Host-side Python entry points use
`uv run --locked python -B` in the selected checkout. The existing immutable
development-image publisher still owns proof environments and their admission.
Provisioning requires only what the farm cells need. The commands farm hosts run
(`portability.queue.*`, `portability.cargo-custody.*`) declare Python, uv, Rust
and Node; none declares system CMake. Cargo builds AWS-LC with its `cc` builder:
`molt.cargo_execution_policy` refuses AWS-LC's CMake builder and the controls that
would need it. Commands that need the GitHub runner image's toolchains
(`hosted-cmake`, `hosted-clang-cl`, as `portability.headers.windows` does) run on
hosted runners, never on the farm; a test refuses any farm assignment that
declares a `hosted-*` toolchain. So the host prerequisites are git, curl and
clang (the compiler doctor requires); macOS ships curl, so provision checks only
the Command Line Tools there. Provision then runs
`molt doctor --json --strict`: a JSON error report cannot count as a successful
install merely because the report-only doctor mode exits zero. Doctor reports
system CMake and Ninja as warnings, not errors. Only building LLVM from source
(`tools/bootstrap_llvm.py`) uses them, and it refuses at the point of use with
the fix. Doctor's `native` backend readiness needs Cargo and a C compiler, not
CMake. Source-built extensions use the environment's attested Ninja
(`source_extension_producer`, pinned through `uv.lock`), not a system one.

On 2026-11-04 the Intel Mac mini showed why a package manager is the wrong
dependency here. Homebrew made Intel macOS a Tier 3 configuration with no
prebuilt bottles, so `brew install cmake` could not succeed, and nothing needed
CMake anyway.

System packages stay an operator step, except on a WSL gate guest whose
inventory record carries `install_system_packages = true`, the owner's per-host
approval (molt-gate, 2026-11-04). There provision installs the missing packages
of `trunk_loop.WSL_GUEST_PACKAGES` as root inside the guest
(`wsl.exe --user root`) before the user-level install, and reports what it
installed. A second run installs nothing.

On POSIX hosts, a prerequisite that is still missing is named with its own fix,
and provision exits 2. It never prints a blanket list that asks for an install
the host already has. On macOS the Command Line Tools provide the compiler and
git. Their presence test is `xcode-select -p`, because `/usr/bin/cc` and
`/usr/bin/git` are shims that exist without them. Their installer needs the
owner's GUI consent, so it stays an operator step. On Windows provision checks
git and the MSVC Build Tools through the same vswhere query that builds use
(`molt.platform_toolchain.visual_studio_installation`), and like it requires
exactly one selected installation. Builds construct their own MSVC environment,
so no developer shell and no `cl.exe` on `PATH` is needed. Windows farm hosts are
x86_64 only: the query names the x64 component, and an ARM64 host would need the
ARM64 component in `config/llvm_toolchain_arches.toml`, so provision refuses it.

Provision installs the pinned uv (`tool_releases` `uv.version`) into its own
directory, `<custody_root>/bootstrap/uv/<version>/`, through the official
installer with `UV_UNMANAGED_INSTALL` set to that directory. That mode writes only
the directory: it edits no `PATH` or shell profile and installs no self-updater.
It installs whenever that directory's uv is absent or reports another version,
on POSIX and Windows alike, and every run re-reads `uv --version`. So an
interrupted install converges on the next run. The installer is the release's
own versioned script (`astral.sh/uv/<version>/install.*`), fetched over HTTPS.
The POSIX script checks the archive's SHA-256 only when `sha256sum` exists, and
the PowerShell script does not check it; this is the same trust the documented
local install grants. The toolchain contract admits only the pinned version,
and an older uv cannot install the pinned Python patch release: on 2026-11-04 the
macmini's uv 0.10.9 failed with "No interpreter found for Python 3.12.15".

The farm never uses the user's own uv (`~/.local/bin`). Other projects on a farm
host run that binary, and Windows cannot replace a running `uv.exe`: on
2026-11-04 bat00's long-running `fleet` agent held uv 0.13.1 open, the installer
failed, and the worker then failed the contract `^uv 0\.13\.6\b`. Replacing that
binary would also change another project's tool. A new pin installs beside the
old one, so provisioning never replaces a running uv. `tools/trunk_loop.py::bootstrap_uv`
is the one authority for the path. Every farm command names it: provision, remote
commands, the POSIX, CIM and WSL worker launches, and the worker's own children. A
CIM-created Windows process inherits the user's registry `PATH`, not the SSH
session's, so a bare `uv` there would select the user's binary. Every farm shell
(provision, remote commands, launches) and the worker put the pinned directory
first on `PATH`, then `~/.cargo/bin`: proof commands run `uv` by name, and rustup
installs with `--no-modify-path`, so a CIM worker's registry `PATH` may lack
Cargo. Before its first test the worker refuses unless `PATH` selects the pinned
uv (`require_pinned_uv`), naming the provision command to run.

Each run's controller names its own pin, and a worker lives for one run, so a
pin change reaches the next run with no restart. The WSL guest installs under its
own Linux custody root, never under `/mnt/c`. The cost of isolation is about
40 MB per host per pin, even where the user's uv already matches. Old pin
directories stay until an operator removes them.

`molt-gate` is Linux x86_64 behind bat00's Windows SSH endpoint. Scripts enter
`wsl.exe -d molt-gate -- bash -s` through stdin, avoiding Windows re-quoting of
POSIX scripts. Binary files travel through tar on stdin and literal `cat`
collection, under the same guarded SSH owner. Its worker runs under its own
`wsl.exe` client ([WSL instance lifetime](#wsl-instance-lifetime)); the declared
`vmIdleTimeout=-1` must already match before provisioning.

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
is fabricated. The [durable-identity contract](durable_file_identity.md) retains
udev as the persistent Linux custody prerequisite; HF-761's review rejected
`O_PATH` ioctls and a truncated statfs ID as replacements for that authority.
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
A WSL guest uses a third launcher, `wsl-session`; the inventory admits it exactly
for hosts with `wsl_distribution`.

### WSL instance lifetime

In WSL 3.0.3 a distribution instance lives only while a Windows client process
is registered with it. Linux processes, sessions and systemd units do not hold it.

- Each `wsl.exe` call creates or opens the instance and registers the calling
  Windows process with the instance's lifetime manager, unless the idle timeout
  is negative
  ([LxssUserSession.cpp, lines 2774-2792](https://github.com/microsoft/WSL/blob/3.0.3/src/windows/service/exe/LxssUserSession.cpp#L2774-L2792)).
- When the last registered client exits, the manager arms a timer for that
  timeout; a new client cancels it. When the timer fires with no client, the
  termination callback runs
  ([Lifetime.cpp, lines 106-116, 212-228 and 242-298](https://github.com/microsoft/WSL/blob/3.0.3/src/windows/service/exe/Lifetime.cpp#L106-L298)).
- The callback stops the instance when no client is registered, with a
  non-forced request
  ([LxssUserSession.cpp, lines 3714-3735](https://github.com/microsoft/WSL/blob/3.0.3/src/windows/service/exe/LxssUserSession.cpp#L3714-L3735)).
  Guest init refuses that request only for an in-use Plan 9 file. With systemd
  as boot init it runs `systemctl poweroff`, then `reboot(RB_POWER_OFF)`
  ([init.cpp, lines 2822-2906](https://github.com/microsoft/WSL/blob/3.0.3/src/linux/init/init.cpp#L2822-L2906)).
- The timeout is `.wslconfig` `[general] instanceIdleTimeout`, default 15,000 ms
  ([WslCoreConfig.h, lines 275 and 340](https://github.com/microsoft/WSL/blob/3.0.3/src/windows/common/WslCoreConfig.h#L275-L340));
  each WSL2 instance receives it at creation
  ([WslCoreVm.cpp, line 1337](https://github.com/microsoft/WSL/blob/3.0.3/src/windows/service/exe/WslCoreVm.cpp#L1337)).
  `[wsl2] vmIdleTimeout` (lines 255 and 339) governs only the utility VM.

This explains the 2026-11-04 loss. A guest-side new session held nothing, so the
instance stopped about 15 s after a collector probe's `wsl.exe` exited (it
polled every 20 s), and the next probe booted a new instance. The worker is the
held object, so the worker owns the hold. CIM creates
`wsl.exe -d <distro> --exec /bin/sh <stage>/launch.sh` outside the SSH job.
`--exec` splits the rest of the command line with `CommandLineToArgvW` and runs
no shell
([WslClient.cpp, lines 1658-1663 and 1822-1842](https://github.com/microsoft/WSL/blob/3.0.3/src/windows/common/WslClient.cpp#L1658-L1842)).
The launcher admits only words that need no quoting, and the POSIX script
crosses as a stage file through the binary tar transfer. The script redirects
stdin to `/dev/null` and its output to `worker.log`, publishes the worker's
identity, then `exec`s the worker. The registered client therefore lives exactly
as long as the worker; after the worker exits, the normal idle timeout applies.
No `.wslconfig` key changes: `instanceIdleTimeout=-1` would keep every
distribution alive with no worker, and the source shows that a held client is
enough.

### Worker standard streams

A worker owns real standard streams before it loads repository authorities:
stdin reads the null device, and stdout and stderr append to its stage
`worker.log`. The POSIX and WSL launchers get this from
`exec </dev/null >>worker.log 2>&1`. A Windows worker is created by CIM
`Win32_Process.Create`, so it starts without valid standard handles, and
`os.dup2` rebinds only the C runtime descriptors. A `subprocess` child started
without explicit stdio still duplicates the invalid OS handles and fails with
WinError 6. On 2026-11-04 the bat00 worker lost `sys.stderr` this way and vanished
without a receipt. So `bind_standard_streams` also points the OS standard handles
at the rebound descriptors (`SetStdHandle` with `msvcrt.get_osfhandle`) and
rebuilds `sys.stdin`, `sys.stdout` and `sys.stderr`. This matches CPython's
`subprocess`, which reads `GetStdHandle` for any stream the caller leaves unset.
PowerShell redirection stays excluded because it re-encodes output as UTF-16.
`bind_standard_streams` is the one authority for a detached Windows Python worker.
A sibling such as `tools/calib_daemon.py`, which uses bare `dup2`, must call it
before it can run under CIM. The log descriptor is inheritable, so a grandchild
that outlives the worker (a build-cache daemon, for example) keeps `worker.log`
open. Stage retention must tolerate that. `worker.log` is diagnostic bytes in each
writer's own encoding, not decoded evidence.

### Worker liveness

Every launcher acknowledges the worker's identity: pid, the start stamp that
the OS stored at process creation, and on Linux the PID-namespace instance
(kernel boot id and PID 1's start ticks). A restarted WSL distro has a new init
under the same VM kernel, so its instance differs. macOS uses `ps` `lstart`;
Windows uses the CIM `CreationDate`. These stamps are stored, never recomputed
from the clock. The boot times that `kern.boottime` and `LastBootUpTime` report
move with clock steps, so the collector uses them only as diagnostics. Each
collector round trip probes that identity first and reads `terminal.json`
second. A receipt that is still absent after the worker was seen gone can never
appear. In that case the collector pulls `worker.log` beside the run record,
records `state = "vanished"` with the observation, and fails at once. Its
message names the pid, the instance restart time or the process change, and
the retained stage. A launch that was never acknowledged has no identity; it
fails as unknown and never waits. A live worker without a receipt stays
unknown (exit 3 with `--no-wait`).

### Process and evidence custody

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

Run-record schema v2 removes the inventory interpreter field. Schema v3 adds
the acknowledged worker identity. A retained record stays with the exact
controller of its own version, named in that record; no later schema silently
reinterprets its host contract.

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
`c527e1af3aaf` and its owning design remain preserved for Stage 2; importing its
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
does not reinstall prerequisites or clone another checkout. Fake-tool runs cover
the macOS host with and without the Command Line Tools (no package manager, no
CMake), the per-tool report, and the pinned uv's installation into its own
directory with its convergence check. Each fake host also holds a user uv first
on the shell's `PATH`; the tests fail if provisioning runs or rewrites it. They
also require the checkout of the provisioned commit before `uv sync` and doctor.
The provision script, remote commands and the WSL launch must put the pinned
directory first on `PATH`, never `~/.local/bin`. Provision must refuse tracked
edits and an unpublished commit before touching the host. The worker's
`require_pinned_uv` must refuse a `PATH` whose first uv is any other binary. A doctor test proves missing
CMake and Ninja are warnings, not errors. A child-process
test proves the standard-stream contract on POSIX: the worker's and a
default-stdio grandchild's output reach `worker.log`, and stdin reads EOF. The
Windows `SetStdHandle` path is covered only for call order by an injected fake.
The real proof is a live bat00 `portability.queue.windows` run whose CIM worker
spawns default-stdio children and reaches a terminal receipt.

Unit controls must reject corrupt, truncated or substituted archives without
network access or publication; execute a real POSIX detached child across parent
exit; and verify Windows CIM command quoting and failure acknowledgement against
its public API. The real bat00 disconnect experiment is the Windows lifetime
oracle. Liveness tests run the real launch script and probe on the local POSIX
OS: the published identity equals a later probe while the worker lives, and it
is gone after exit. Fake transports check that a restarted distro, an absent
process, a zombie, a reused pid, or a missing identity fails the collector on
its first round trip, with retained evidence and no sleep. They also check that
a published receipt wins over an exited worker. The live oracle on molt-gate
launches `portability.queue.linux`, ends the launching SSH session, waits past
the instance idle timeout with no other client, and collects the terminal
receipt. Both named proof cells must return source-bound receipts, even when the
tests are red. No broader suite or repeated matching PASS is required here.
