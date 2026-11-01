# Dedicated multiprocessing pipes

`async_rt::process` owns child launch and descriptor transfer. Multiprocessing
uses two anonymous pipes for its framed protocol. Stdout and stderr are inherited;
worker stdin is the null device, matching [CPython's worker bootstrap](https://github.com/python/cpython/blob/3.14/Lib/multiprocessing/util.py#L501-L517).
`Process` and Pool workers share this primitive. Queue,
Pipe, Value and Array messages continue through their existing `_Hub` codec.
No transport falls back to a standard stream.

## Decision and prior art

Use selected inherited descriptors on POSIX and child-pulled, non-inheritable
handles on Windows. This follows [CPython's POSIX spawn](https://github.com/python/cpython/blob/3.14/Lib/multiprocessing/popen_spawn_posix.py)
and [Windows bootstrap](https://github.com/python/cpython/blob/3.14/Lib/multiprocessing/spawn.py).
The pipe endpoints exist before launch, have no public address, and need neither
a filesystem rendezvous nor an authentication exchange. Parent endpoints and
unrelated descriptors cannot become child IPC endpoints by fd-number reuse.

The other considered shape is an authenticated private named endpoint.
[CPython connection](https://github.com/python/cpython/blob/3.14/Lib/multiprocessing/connection.py)
places Unix endpoints in a private directory and authenticates peers.
[Servo's IpcOneShotServer](https://docs.rs/ipc-channel/latest/ipc_channel/ipc/struct.IpcOneShotServer.html)
similarly provides a named rendezvous with platform-specific socket, Mach-port
and named-pipe implementations. [CPython's forkserver](https://github.com/python/cpython/blob/3.14/Lib/multiprocessing/forkserver.py)
also uses an authenticated Unix connection and SCM_RIGHTS descriptor transfer;
Molt's spawn-only process model has no server to amortize that rendezvous.

A bounded macOS arm64 component experiment used CPython 3.12.15, the same child
program and imports, four fresh children, and one request/response per child.
The named variant used CPython's authenticated AF_UNIX Listener/Client in a
0700 temporary directory. The pipe variant passed two anonymous endpoints.
Three interleaved samples under `/usr/bin/time -l` gave:

| Shape | Retired instructions, whole process tree | Four-worker round trip, ms |
| --- | --- | --- |
| Pipes | 1,973,694,042; 1,973,258,183; 1,981,756,886 | 297.1; 318.3; 423.6 |
| Named/authenticated | 1,994,803,200; 1,990,955,084; 1,991,810,513 | 378.2; 589.8; 476.4 |

This includes interpreter startup and is a component decision aid, not a Molt
performance result. Wall time was noisy; the instruction ranges did not overlap.
Pipes avoid address permissions, authentication state and stale endpoint cleanup.
Windows pulls also avoid global inheritable-handle races and Rust's still-nightly
[`spawn_with_attributes`](https://doc.rust-lang.org/std/os/windows/process/trait.CommandExt.html#tymethod.spawn_with_attributes).
No upstream implementation was copied.

## Ownership and failure paths

One `PreparedIpc` owns all four endpoints until the existing process registry
accepts the child. POSIX changes FD_CLOEXEC only after fork, keeping exactly the
selected descriptors. The same `inheritance::configure` primitive implements
`Popen(pass_fds=...)` and `close_fds`; the parent's flags never change.
The child immediately restores CLOEXEC on its claimed endpoints.

Descriptor enumeration observes the child, after stdio redirection and creation
of Rust's exec-error pipe. Linux uses raw getdents64 on `/proc/self/fd`; macOS uses
[proc_pidinfo](https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/wrappers/libproc/libproc.c)
with storage allocated before fork. Marking other descriptors CLOEXEC preserves
the exec-error pipe until exec can report success or failure. No allocation or
shared library lock runs in the child callback. An unavailable or incomplete
descriptor-table observation refuses the spawn.
Other Unix hosts explicitly refuse descriptor-table inheritance until they have
a complete table observation; a current descriptor limit cannot bound already
open descriptors after a limit reduction.

On Windows, the child opens its parent's process with PROCESS_DUP_HANDLE and
duplicates both pipe handles without inheritance. An acknowledgement follows
successful conversion to owned CRT descriptors. The parent retains the source
handles until that acknowledgement, then releases them so EOF works. Child exit,
an invalid acknowledgement, or a bounded bootstrap timeout is a launch error;
the existing process registry terminates and joins the admitted child/workers.
Even if program stdio is closed, IPC descriptors remain outside 0, 1 and 2.

The existing bounded process stream queues and worker registry own parent I/O.
No new executor, process registry, or detached thread is introduced. The child
owns and closes its dedicated files. `_Hub` has one framing path per endpoint
kind and no borrowed standard-stream handles. Closing parent transports is
idempotent, so Pool joining and subsequent retirement cannot release a stream
reference twice.

Linked-browser qualification also requires binding non-empty callable-table
execution state before admission under the shared
[portable ABI admission contract](../spec/areas/wasm/0400_WASM_PORTABLE_ABI.md#callable-table-execution-state-admission).
Linking relocates the app's reserved region; the shared loader publishes its
resolved base through the canonical ABI export. Empty attestations need no
publication. Using the default base for a non-empty reserved region calls
unrelated table entries: the observed signature trap abandoned a Rust recursion
guard and was then diagnosed during shutdown. The correction binds the base;
the recursion-depth teardown assertion remains intact.

Every launch still passes `self_spawn::admit_child`; the multiprocessing role
remains declared by MOLT_MP_SPAWN and the frozen command-line role marker.
WASM entry points refuse dedicated IPC with a descriptor-passing process-model
diagnostic. They never reuse host stdio as an IPC implementation.

## Qualification

The 2026-10-31 native regression reproduces `ValueError: message too large` on
the old release transport. The replacement prints on both program streams and
returns the result without corrupting its channel. The CPython 3.12.15
Queue/Pipe/Pool/stdin differential and four compiled self-spawn/descriptor tests
pass on macOS arm64 and Linux aarch64. Linux additionally passes shared
Value/Array and Pool worker recycling. Runtime and both backend checks pass on
these hosts. The WASM probe executes the explicit unsupported diagnostic.

The Windows implementation is not yet qualified: bat00 is reachable, but the
canonical MSVC activation guard fails while archiving its closed job, before
compilation. The existing multiprocessing integer-codec differential also fails
on both native hosts at `2**80 + 3`: the unchanged binary pickle authority
rejects a boxed integer. These remain separate owner decisions; no transport
fallback or weakened codec oracle hides either cell.

The release four-worker Pool round trip used three interleaved before/after
runs under `/usr/bin/time -l` on macOS arm64:

| Measurement | Before | Dedicated pipes |
| --- | --- | --- |
| Retired instructions | 984,548,742; 889,571,827; 885,148,074 | 881,781,804; 884,239,551; 886,493,807 |
| First message, ms | 53.451; 57.599; 53.273 | 56.324; 55.003; 57.668 |

Median retired instructions decreased 0.60%; no instruction regression was
observed. Wall-time ranges overlap and do not establish a latency improvement.
Per-run receipts and outstanding consumer/platform blockers belong to the
private HF-627 handoff; the project ledger owns integration qualification.
