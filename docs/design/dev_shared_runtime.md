# Development executables and the shared runtime (HF-634)

Status: design and measurement work in progress; this document does not qualify
a platform or announce a change to the default linkage. The V1 ledger owns
completion. [The JIT design](dev_jit_tier.md) owns in-memory code generation.

## Decision and admission

The proposed development AOT executable contains the application, its compiled
stdlib partition and its C launch stub. It links the same runtime image that
the JIT tier already obtains from `jit_runtime_image.py`; there is one image
producer, product store, generation identity and retention owner. Release
executables retain static linkage. Profile and target select a typed static
archive or shared-image dependency in `NativeLinkPlan`, before command
rendering; neither a failed link nor a failed load selects another linkage.

The decision needs unloaded evidence. Measure a fresh executable's first start
against subsequent starts of that same inode, including output, exit status
and stderr. A first-start difference includes loader, page-cache and security
work; it does not identify a service's exclusive CPU time. Record publication
time separately: Defender can scan at file creation rather than process start.
If the first-start excess is below about 20 ms on both macOS and Windows,
security-scan savings do not justify switching. Link-time evidence remains a
separate decision for the integrator.

`tools/executable_scan_probe.py` owns paired first-load observations. It records
every sample's load before/after, publication time, executable identity, child
execution time and guard overhead separately. Each of three samples writes a
new independent inode, then starts it once and starts that same inode three
more times. Warm and first observations are interleaved rather than collected
as separate batches. Samples collected at a one-minute load of 2 or above are
not unloaded. Windows has no POSIX load average: a report with unknown load is
not certified unloaded, and needs a separately retained CPU/queue observation.
`tools/native_link_benchmark.py` remains the link measurement authority.

The full acceptance also measures three changed-source edit/run cycles for
hello and `import os, sys`, in alternating static/shared order, on macOS and
Windows, plus Linux link time through `tools/linux_vm.py`. First-load probing
of an unchanged executable alone cannot prove edit/run improvement.

## Exact dependency and launch

An image identity must cover the runtime archive digest, its admitted native
dependency closure, the image link recipe and the selected toolchain. Use the
existing store's content-derived generation coordinate in the absolute load
path. On Mach-O give the image that absolute install name; on ELF bind the
absolute dependency name rather than an ambient library search directory.
Do not use a mutable `libmolt_runtime` symlink or an environment-selected image.

A digest in a pathname prevents accidentally choosing another generation,
but does not detect replacement of the bytes at that pathname. The image
therefore also needs a generation-specific exported ABI witness, required by
the application before its runtime starts. A substituted image from another
generation fails dynamic binding with the missing witness named in the loader
diagnostic. This is a mismatch fence within the owned immutable store, not an
adversarial signature scheme. There is no per-start hash of the entire runtime.
The existing generated-object ABI witness continues to check execution mode.

Keep the application bootstrap export alive: the runtime's
`molt_isolate_bootstrap` import must bind to this application, as it binds to the
JIT runner's export today. Test isolate/thread execution as well as a hello
program; merely resolving ordinary runtime calls misses that reverse edge.

A shared dev executable is a local development artifact. Publishing it with
`--output X` is allowed, but moving it to another machine does not move its
absolute runtime dependency. Removing/evicting its generation makes it
unlaunchable. Document this alongside `--profile dev` and its output behavior;
recommend a release build for distribution. Builds and cached launches renew
the existing product-store retention; standalone execution cannot renew it.
Document the existing idle retention window, rather than inventing a second
lifetime registry. No security setting changes are required.

Windows is an architectural admission question, not just a linker flag. The
current shared-image producer explicitly rejects COFF, and the JIT design
records that a DLL cannot import an arbitrary executable's bootstrap without
an import library. A generation-independent DLL needs a supported bootstrap
registration boundary shared by AOT and JIT, or a proven import-library design
that preserves generation reuse. It also needs explicit runtime data exports,
an import library and a digest-bound DLL load path. Do not claim Windows
shared-runtime support from a Mach-O plan test, or hide missing support behind
a retry with static linkage. The integrator must reconcile that runtime/ABI
scope with this lane's link-plan scope before a Windows migration.

## Executable publication and interpreter custody

Fresh linked bytes may be renamed into place. A cache-owned immutable
executable may be hardlinked only under an ownership contract that permits
changing its link count and ctime; otherwise publish a bounded byte copy.
An APFS clone is a distinct executable inode and is not scan reuse. Never
substitute `cp -c`, `clonefile`, `fclonefileat` or a copy API that can clone in
an executable publication path. Keep block cloning available for data whose
performance contract benefits from it. Test the canonical copy boundary with
a clone operation that fails if invoked, including cross-device and read-only
sources and a native candidate published from another directory. A test of
equal bytes alone cannot distinguish a clone from a fresh write.

The current `_atomic_link_or_copy_file` falls through `_atomic_copy_file` to
`artifact_publication.staged_copy_file`, which uses `file_publication.clone_file`
without an observed source identity. Native candidate finalization can reach
that same staging path. Correct the common owner and all executable consumers
together, preserving source identity fences, failure cleanup and atomic
publication. Do not add an executable-only copy implementation beside it.

Python image sealing already writes fresh bytes in
`python_environment_location._detach_shared_installation_file`, then recreates
only image-local hardlink aliases. Its purpose is to prevent chmod of an
external uv-cache inode when the image becomes read-only. Preserve that
isolation and alias topology. The owned-interpreter test fixture copies the
whole base installation because standalone CPython resolves dependent
libraries relative to its executable. It deliberately avoids host hardlinks:
their nlink/ctime changes invalidate concurrent identity captures.

The probe's `--tree` option measures the copied interpreter plus libraries;
`--tree --seal` also measures the production detach operation against that
private copy. Neither operation touches a host interpreter's inode. Sharing
scan assessment across environment images requires an explicitly owned,
once-published read-only interpreter installation with symlinked venv
launchers, not hardlinks added during live captures. Mutation tests still need
private copies. Such a change must preserve each image's interpreter identity,
external-library closure and retirement leases; it cannot replace isolation
with an unowned host-prefix dependency.

## Costs and qualification

Record image link time, size and first load once per runtime generation, and
the executable's link time, size and first/warm start separately. Whole-archive
image linking retains runtime sections a static executable can dead-strip;
disk and mapped-image size can increase even while per-edit executable bytes
decrease. Include old generations retained during the reader grace window.

Cross-image calls introduce stubs/indirection and prevent cross-image inlining.
Measure instructions retired for a call-heavy compiled program, along with
whole-program time and memory. Separate dyld mapping/binding cost from the
image's first load. Do not infer instruction counts from wall time or from
object-file size. If hardware counters are unavailable, record that cell as
unknown. Verify release instructions and executable size before/after as well
as unchanged release link policy. Exact output/exit/stderr equivalence on the
differential smoke set is required; a recording link fixture is insufficient.

## Prior art and the security constraint

- [Chromium component builds](https://chromium.googlesource.com/chromium/src/+/main/docs/component_build.md)
  split development code into shared components to reduce relinking. Their
  explicit export/import rules and warnings about duplicate static state are
  relevant here: Molt must have one runtime instance, especially its allocator
  and singleton data, across the application and any extensions.
- [Bevy's `bevy_dylib`](https://docs.rs/bevy_dylib/latest/bevy_dylib/)
  likewise speeds incremental linking, while discouraging distribution of that
  development arrangement. Molt can reuse its existing image rather than add a
  second runtime build configuration.
- [nextest on macOS](https://nexte.st/docs/installation/macos/)
  describes first-execution XProtect delays and recommends a Developer Tools
  exemption for the invoking terminal, including multiplexer caveats.
- [Apple's Developer Tools control](https://support.apple.com/guide/mac-help/mchlc5fb7f9c/mac)
  permits selected applications to run code outside ordinary security policy.
  Molt's design must work without asking the user to enable that exemption.
- [Microsoft Defender performance mode](https://learn.microsoft.com/en-us/defender-endpoint/microsoft-defender-endpoint-antivirus-performance-mode)
  defers scans on trusted Dev Drives while retaining real-time protection on
  other volumes. That is a volume-specific user/administrator choice, not a
  compiler prerequisite. Measure the actual volume's existing configuration;
  change no protection settings to manufacture the win.
