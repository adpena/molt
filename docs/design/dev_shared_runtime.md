# Development executables and the shared runtime (HF-634)

Host development executables link the runtime and compiled stdlib as shared
images. The JIT uses those same image products. Release, external native
artifacts, BOLT, explicit cross targets and Windows use static runtime linkage.
The observed cell below is macOS arm64. The ELF link plan has link-level
coverage. The [V1 ledger](../agent/V1_HANDOFF_FINDINGS.md) owns qualification,
and the [JIT design](dev_jit_tier.md) owns in-memory code generation.

## Shared runtime startup before stdlib reuse (2026-11-20)

macOS arm64, a call-heavy program (`list.append`, `len`, `hash` in a 2 M loop),
the same objects linked both ways from one dev build, load average 7-8:

| | static archive | shared image |
| --- | --- | --- |
| Executable | 27.8 MB | 2.6 MB |
| `ld` time (3 links) | 1.18-1.29 s | 0.14-0.30 s |
| First exec of a fresh write (3 samples) | 0.36-0.42 s | 0.23-0.24 s (one 0.61 s) |
| Warm exec | 0.10-0.11 s | 0.10-0.11 s |
| Instructions retired | 2.548 G | 2.583 G (+1.4%, cross-image calls) |

Output is identical. In the differential harness (8 class tests, warm, `--jobs 2`)
the link phase per test fell from 1.5-2.6 s to 0.5-1.3 s; a test whose stdlib
profile needs another runtime variant pays that variant's image link once per
runtime generation (8-10 s here). The remaining per-test cost is backend
preparation and module-graph work (1-3 s each), not linking.

Implementation: `link_pipeline._links_shared_runtime_image` selects the linkage;
`backend_output_pipeline` ensures the image through `jit_runtime_image`, the one
image producer, which names it `@rpath/<image>` (Mach-O) or its soname (ELF);
`_build_native_link_plan(runtime_image=...)` links it in place of the archive with
an rpath to the image's store entry and exports the isolate bootstrap the image
imports. Only an executable sizes the main thread's stack (`NativeLinkOutput`).

## Shared stdlib startup on macOS arm64 (2026-11-20)

Both programs produced identical stdout on every sampled JIT launch. The
baseline was `878eeb1632` with the machine-code cache. The candidate was the
HF-634 working tree on `4195ff78f2` with first-call JIT compilation. A cold
launch used an empty function-cache root with the runner and linked images
already built. Warm values are medians of three subsequent launches. Peak RSS
is in decimal MB. Host load varied across the runs.

| Program | Cold wall before → after | Cold RSS before → after | Warm wall before → after | Warm RSS before → after |
| --- | ---: | ---: | ---: | ---: |
| `import os, sys` | 28.03 → 0.29 s | 629 → 37 MB | 6.03 → 0.05 s | 471 → 34 MB |
| `os_path_basic.py` | 37.97 → 1.33 s | 715 → 48 MB | 6.22 → 0.43 s | 462 → 41 MB |

Both prepared payloads contained five local terminal bodies. The warm
`os_path_basic.py` trace reported 853 terminal operations across its five
entries. The program called three of them, with three machine-code cache hits
and zero misses. For each program, the JIT and dev AOT artifact manifests named
the same runtime and stdlib image entries. The `os_path_basic.py` stdlib image
imported 921 readiness and code-slot symbols. Both the dev executable and the
JIT data image supplied every import.

Substituting the expected stdlib generation in a copy of the JIT image exited
125. The diagnostic named the missing generation symbol and the start receipt
stayed absent.

## Development AOT relinks on macOS arm64 (2026-11-20)

Three alternating pairs used the same production app object, runtime image and
stdlib archive bytes from a single build. The second link used the stdlib
image built from that archive. Driver wall time and first exec are medians.
Each link wrote a fresh executable. Every launch matched stdout.

| Program | Executable bytes | Link driver wall | First exec | Warm instructions |
| --- | ---: | ---: | ---: | ---: |
| `import os, sys` | 21,106,904 → 262,696 | 4.21 → 0.61 s | 105.62 → 38.32 s | 341.5 → 348.4 M |
| `os_path_basic.py` | 23,301,112 → 298,680 | 1.36 → 0.09 s | 6.90 → 10.27 s | 447.4 → 456.3 M |

First exec varied from 1.00 to 142.90 s across the pairs. Warm `import os, sys`
runs were 0.02–0.03 s and warm `os_path_basic.py` runs were 0.02–0.04 s.
An earlier three-pair `import os, sys` run measured 0.66 → 0.23 s first exec and
2.52 → 0.09 s link driver wall. The first-exec series remains sensitive to host
startup stalls.

## Static release control on macOS arm64 (2026-11-20)

The baseline and candidate used trunk `f75c387743`, with the HF-634 patch
applied for the candidate. The executables used static Molt libraries and
produced the same stdout. Three subsequent starts alternated the two builds.

| Program | Baseline bytes | Candidate bytes | Warm instructions, baseline → candidate |
| --- | ---: | ---: | ---: |
| `import os, sys` | 16,835,032 | 16,818,568 | 120.59 → 120.51 M |
| `os_path_basic.py` | 17,579,840 | 17,563,360 | 158.97 → 159.22 M |

The integrated first-call candidate on `4195ff78f2` also linked both release
programs with static Molt libraries. The executables measured 16,818,576 and
17,563,368 bytes, with median warm instruction counts of 119.57 M and 159.54 M.
The earlier table controls for source by building the baseline and candidate
from the same revision.

## Differential qualification on macOS arm64 (2026-11-20–21)

With CPython 3.15.2 as the oracle, the HF-634 working tree on `4195ff78f2`
passed the same eight class cases, 40 stdlib cases and two thread cases in
native dev and release. Both thread cases also passed in JIT.

The initial dev cohort had seven mismatches whose output and status matched
the clean `f75c387743` control. Those cases cover defaultdict subclass insertion,
dataclass temporary destruction, path callbacks, frame keyword validation and
three typing behaviors. The selected set covers those stdlib families with
other CPython comparisons. The revised dev cohort passed 49 cases and
reproduced the existing HF-813 exception-state failure. Its predeclared
alternate, `sys_runtime_state.py`, passed in dev and was part of the 50-case
release run.

## Shared development stdlib links

The backend produces one position-independent stdlib archive for native dev AOT and JIT. The image store links that archive against its admitted runtime generation. The image key binds the archive bytes, runtime image, target, profile, link recipe and linker identity. A development executable links both images through rpaths to their exact store entries. Release executables, external native artifacts, BOLT and cross targets retain their static link paths.

The native loader binds the stdlib partition's application data through the same reverse-relocation boundary as the runtime's isolate bootstrap. The partition already closes over compiled functions. Its link admits only the application-owned data symbols the archive references. AOT exports those definitions from its executable. JIT preparation emits the same data definitions into an application image that the runner loads before the stdlib. Both code owners keep the module registry and callable resolver with the application. The runner passes the same readiness block to the runtime that the loaded stdlib uses.

JIT preparation runs the native type-flow pipeline with the complete program in scope. The published application carries terminal TIR for application functions and reachable program-specific clones. Canonical definitions owned by the shared archive become imports. The runner stages the remaining entries at startup and materializes each called body through the existing first-call compiler and machine-code cache.

Each linked image exports a generation-specific witness. The AOT start stub imports each witness through a non-lazy data relocation. The JIT runner checks the expected symbols before code generation and before writing a start receipt. Substitution with another generation fails with the missing witness named in the diagnostic. Builds and cached launches renew these entries through the existing product store.

On Mach-O, ld64's per-symbol `-U` allows the exact reverse imports while the runtime binds through the usual two-level namespace. On ELF, the executable admits the same typed data imports through version-script exports. The [ld64 manual](https://github.com/apple-oss-distributions/ld64/blob/main/doc/man/man1/ld-classic.1) and [GNU ld options](https://sourceware.org/binutils/docs/ld/Options.html) describe the corresponding linker controls.

## Decision and admission

The development AOT executable carries application objects and its C launch
stub. It uses the runtime and stdlib images that the JIT obtains from
`jit_runtime_image.py`. One product store owns the image generations and their
retention. Profile and target select typed archive or image dependencies in
`NativeLinkPlan` before command rendering. Release uses static linkage.

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
path. Mach-O names the image `@rpath/<generation-qualified-name>` and ELF
uses the generation-qualified SONAME. The executable places the matching
store directory on its rpath.

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
