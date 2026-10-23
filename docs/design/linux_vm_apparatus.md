# Linux VM apparatus

Status: implemented (HF-463, HF-471); both VMs run their definitions since
2026-10-22. Authority: [`tools/linux_vm.toml`](../../tools/linux_vm.toml).
Its one consumer: [`tools/linux_vm.py`](../../tools/linux_vm.py). Runbook:
[OPERATIONS.md, "Linux VMs (Lima)"](../OPERATIONS.md#linux-vms-lima).

Molt reproduces Linux failures on the macOS host in two Lima VMs under the
`vz` driver: `molt-linux` (aarch64) and `molt-x86` (x86_64 through Rosetta).
They were made by hand, and no tool owned them. Three defects followed:

- **Pinned host files.** A guest pins every host file it has looked up
  through its virtiofs share. When the host deletes such a file, its space
  stays allocated while the VM process holds it. On 2026-10-22 the
  `molt-linux` VM process held 802 files under `~/Molt` and 167 deleted files;
  on 2026-10-17 `molt-x86` held 188 files of worktrees in the Trash, and the
  VM had to stop before the Trash could be emptied.
- **Drift.** The notes said both VMs mount `~/Molt` read-only; `molt-linux`
  mounted the whole home.
- **Port leaks.** Lima forwards every guest port to the host by default. In
  HF-F251 a guest sccache server took over the host's sccache port. Since
  their boot on 2026-10-21 the two hostagents had forwarded 13 and 832 guest
  ports to 127.0.0.1 on the Mac.

The fix: the definitions mount no host path, so a guest pins nothing by
construction; `push` sends commits over Lima's SSH. For a VM that still
mounts host paths (one not yet applied, or one made by hand), `release` frees
its pins with the one guest action that works, and lane retirement runs it.

## Mechanism

Measured on 2026-10-22: macOS 26.4 (Darwin 25.4.0), Lima 2.2.1. `molt-linux`
runs Ubuntu 26.04, kernel 7.0.0-38 (aarch64); `molt-x86` runs Ubuntu 24.04,
kernel 6.8.0-147 (an aarch64 guest with Rosetta). Probe files lived under
`~/Molt/tmp`; `lsof -n -P -p <VM pid>` (with `-a +L1` for deleted files)
counted the VM process's descriptors. The logs are under
`/Volumes/VertigoDataTier/Molt/tmp/vm-fix-hf463/` on the measuring host.

**The host side.** Apple's server opens one host descriptor for each inode
the guest looks up (`stat`, `cat`, and the entries an `ls` returns), and a
second one while a guest process has the file open. It closes the lookup
descriptor after the guest forgets the inode (`FUSE_FORGET`). An Apple
engineer: "Unless the guest sends `FUSE_FORGET`, there is no harm in keeping
them around for performance"
([Apple Developer Forums 741572](https://developer.apple.com/forums/thread/741572)).

**The guest side.** A FUSE inode leaves the guest at its last reference:
`fuse_super_operations` sets `.drop_inode = generic_delete_inode` (Linux v6.8,
`fs/fuse/inode.c:1132`; renamed `inode_just_drop` in v7.0, `:1225`). Eviction
queues one forget when the inode has lookups (`fuse_evict_inode`, v6.8
`:146-161`, v7.0 `:158-176`), and virtio-fs sends each forget as its own
request on the `hiprio` queue (`fs/fuse/virtio_fs.c`, v6.8 `:974-1005` and
`:411-461`; v7.0 `:1234-1256` and `:617-665`). The page cache does not keep a
FUSE inode alive, so a file the guest read behaves like one it only stat'ed.
Only a dentry, an open file or a mapping keeps one. `drop_caches` with 2
frees every unused dentry, so the guest forgets every inode no process uses
([kernel vm sysctl](https://docs.kernel.org/admin-guide/sysctl/vm.html)). A
lookup of a host-deleted path forgets too, once the entry's timeout has passed:
`fuse_dentry_revalidate` then sends `LOOKUP`, gets `ENOENT` and invalidates the
dentry (`fs/fuse/dir.c`, v6.8 `:195-255`, v7.0 `:384-447`).

**The server's batch.** The guest does its part, and the server holds the
result back:

- The guest forgets. After `stat` of 300 files and one drop, the guest's
  inode count (`/proc/sys/fs/inode-state`) returned to its baseline (129,582,
  then 129,931, then 129,584).
- The device completes each forget at once: the guest's `virtio4-hiprio`
  interrupt count rose by 1 for each single forget.
- The server closes forgotten descriptors only at every 64th forget, and then
  closes all that are pending. One forget per step (a new file, `stat`, drop):
  on `molt-linux` the host closed descriptors at steps 16, 80 and 144; on
  `molt-x86` at steps 55 and 119. Each first step closed the earlier tail.
  The v3 review measured that a directory's forget counts toward the batch
  like a file's, and that the count is per VM process, not per mount.
- Up to 63 forgotten descriptors stay open with no time limit. The review's
  probes stayed held for over 10 minutes and through 7 drops: each drop added
  fewer than 64 forgets.

Each guest action, tested on its own probe files after aligning the server's
batch to zero pending (`controls.py`; read, stat and list probes, deleted on
the host before the action):

| Guest action | Guest inodes evicted | Host descriptors closed |
|---|---|---|
| `echo 1 > drop_caches` | none | none |
| `echo 2 > drop_caches` | 21,142 (`molt-linux`), 3,660 (`molt-x86`) | none |
| `echo 3 > drop_caches` | yes | none |
| `stat` of the deleted paths | after the entry timeout (about 1 s) | none |
| `vfs_cache_pressure` 10000 for 5 s | none: an idle guest does not reclaim | none |
| drop, `stat` of 64 forget-set files, drop | yes | every probe, on both kernels |

By access kind, on both kernels, one release (the last row) freed every
deleted file the guest had read, stat'ed, listed, or mapped and unmapped. A
file a guest process held open or mapped kept two descriptors; the first
release after that process exited freed it.

The two Git objects `molt-linux` kept after the earlier single drops were not
"still in use in the guest", as this document said before: no guest process
held them (the review read `/proc/*/fd` and `map_files` as root). They were
forgotten and waiting in the server's batch; a later run of probe forgets
freed them. The earlier "45 of 47 freed" came from a drop that happened to
carry enough forgets past a batch boundary; it did not reproduce.

## Options

| Option | Pins | Measured cost |
|---|---|---|
| No mount; `push` sends commits over Lima's SSH (chosen) | None, by construction. | 30 commits: 0.19-0.20 s to stream their pack (`git bundle`) into the guest, against 0.27-0.36 s for the guest to build the same pack from the mounted repository. A full tree (`git archive`, 181 MB): 0.56-0.61 s over SSH, 0.61-0.65 s through the mount. `git push --dry-run` over Lima's SSH config: 20-40 ms. Retirement after `apply`: 0.4-0.9 ms, no process. Agents run `push` instead of fetching from the host path. |
| Read-only mount, `release` on retirement and on demand | Until a release runs. Deletions outside retirement (`git gc` of the 644 MiB of packs, the Trash) stay pinned until the next one. One archive and one bundle read through the mount left 221 `molt-src` files open in each VM process. | Retirement 85-90 ms when no VM mounts the path, 213 ms when one does with nothing held, 565-622 ms with a release of both VMs (host load 7-9). Each release drops the guest's caches twice (21,142 and 3,660 inodes evicted per drop) and leaves up to 63 forget-set descriptors open. It depends on the server's undocumented batch size. The guest can read the whole custody root. |
| reverse-sshfs | Only files a guest process has open: SFTP names files by path. | The host's `sftp-server` is not confined to the share: "A compromised `sshfs` process in the guest may have access to unexposed host directories" ([Lima mounts](https://lima-vm.io/docs/config/mount/)). Lima's hostagent does not reconnect a mount whose SSH connection drops ([`pkg/hostagent/mount.go`](https://github.com/lima-vm/lima/blob/v2.2.1/pkg/hostagent/mount.go)). |
| QEMU with 9p | Only open files (9p fids name paths). | A new VM each; no Rosetta, which `molt-x86` needs; the vz driver rejects 9p ([`vz_driver_darwin.go`](https://github.com/lima-vm/lima/blob/v2.2.1/pkg/driver/vz/vz_driver_darwin.go#L276-L277)). |
| A guest timer, or `vfs_cache_pressure` | A drop alone frees nothing (above). | Drops a guest's caches on a schedule for no gain. |

What the guests need from the host, read on 2026-10-22: commits (both
`molt-main` clones fetch from `<root>/molt-src`), lane trees (the runbook's
`git archive` of a lane), and an occasional script. No guest dotfile, symlink
or venv names a host path, and every guest `alternates` file points inside
the guest. Guest clones that name a host path as a remote (`molt-main`,
`molt`, `deps-latest`, `molt-abi-*` on `molt-linux`; `molt-main`,
`deps-latest` on `molt-x86`) fetch nothing after `apply`; `push` replaces
them.

Prior art:

- Docker Desktop shares bind mounts through VirtioFS, and its synchronized
  file shares keep "a synchronized cache of the host files on an ext4
  filesystem within the Docker Desktop VM" for large repositories
  ([Docker docs](https://docs.docker.com/desktop/features/synchronized-file-sharing/)).
  A copy holds no host handle; `push` is the same model, with Git as the sync.
- Lima makes mounts optional: a config that declares `mounts: []` gets none
  (checked with `limactl validate --fill`), and `limactl copy` moves single
  files ([Lima mounts](https://lima-vm.io/docs/config/mount/),
  [`limactl copy`](https://lima-vm.io/docs/reference/limactl_copy/)). Colima
  and Rancher Desktop run on Lima and share its mount types.
- Linux `virtiofsd` holds an `O_PATH` descriptor per inode, the same class,
  and avoids it with `--inode-file-handles`
  ([virtiofsd README](https://gitlab.com/virtio-fs/virtiofsd);
  [virtio-fs list, 2021](https://listman.redhat.com/archives/virtio-fs/2021-August/003821.html)).
  libkrun's server has it too ([libkrun #505](https://github.com/libkrun/libkrun/issues/505)).
  Apple's server has no such mode, and it adds the 64-forget batch.

The trade: a guest no longer sees a host change until someone pushes it. In
exchange, no host deletion is ever pinned, retirement spawns nothing, the
guest cannot read host files, and nothing depends on Apple's undocumented
batch.

## Definition

`tools/linux_vm.toml` (schema 3) holds each VM's type, architecture,
resources, image, Rosetta, containerd, Lima parameters, port forwards and the
guest clone that `push` updates, and for all VMs the guest executables that
`apply` counts as idle and the maintenance process names that make a guest
busy. The schema has no mount: the rendering declares `mounts: []`,
and Lima's own fill (`limactl validate --fill`) keeps it empty, without the
home and `/tmp/lima` mounts of Lima's template. Port forwards list only what a
guest may expose; the renderer appends the rule `{guestIP: 0.0.0.0, proto:
any, ignore: true}`. Lima checks rules in order and its last built-in rule
forwards every guest port on 127.0.0.1
([`default.yaml`](https://github.com/lima-vm/lima/blob/v2.2.1/templates/default.yaml));
a rule whose guest IP is `0.0.0.0` matches every guest address
([`pkg/portfwd/forward.go`](https://github.com/lima-vm/lima/blob/v2.2.1/pkg/portfwd/forward.go)),
so the appended rule matches first. Lima's SSH forward is not a rule and
stays.

The rendered `lima.yaml` declares every key that decides what a rule forwards,
so Lima's filled configuration equals the rendering and `check` compares the
two without copying Lima's defaults.

## Commands

- `check` compares `limactl list --json` with each definition, field by field,
  and exits 1 on drift: any mount is drift. A running VM whose `lima.yaml` is
  newer than its hostagent's `ha.pid` runs an older configuration: drift
  `restart_pending`. For each running VM that mounts host paths, one `lsof`
  reports what its process holds. It runs nothing in a guest.
- `push <vm> [rev]` resolves `rev` (default `HEAD`) to a commit and runs `git
  push` into the guest clone as `refs/heads/host/head` (`--ref <name>`
  picks `refs/heads/host/<name>`), over Lima's SSH config
  (`<instance>/ssh.config`, host `lima-<vm>`). Git sends only the objects the
  clone lacks. Every push lands under `refs/heads/host/` and retains Git
  fast-forward protection: divergent commits are never overwritten, and a
  rejected push leaves the guest checkout unchanged. A name Git would refuse
  is refused first. The SSH command goes in
  `GIT_SSH_COMMAND`, which Git prefers to `core.sshCommand` and `GIT_SSH`, so
  a caller's own `GIT_SSH_COMMAND` cannot redirect it; `GIT_SSH_VARIANT=ssh`
  keeps OpenSSH's argument form. `--checkout` runs one guest command that
  refuses (exit 1, the reason printed) when the clone has changes (untracked
  files included), when its index lock exists, or when a guest process of
  any user has its working directory or an open file in the clone
  (`/proc/<pid>/cwd` and `/proc/<pid>/fd/*`, read as root by one `find`;
  root also expands the `/proc` globs, and `hidepid` hides no process from
  root); otherwise it checks out the exact commit, detached. The listing
  must show the target of init's `/proc/1/cwd`, which only root can read,
  and the checking shell's own `/proc/$$/cwd`: a `find` that failed, or did
  not run as root, fails the command instead of reading as "no process uses
  the clone". Without passwordless sudo it fails, and the report says so.
  It does not see a process that
  names the clone only by path and has no file open there at that moment (a
  `git -C` or `cargo --manifest-path` between two file accesses). Checks and
  checkout run in one guest command, but another session can start a process
  in the clone between them; Git's index lock still fails a concurrent Git
  command that writes the index. Without `--checkout` it prints that checkout
  command, with the commit, not the shared ref.
- `release` serves VMs that mount host paths. For each, the host creates a
  forget set under every mount (`<mount>/.molt-linux-vm-forget/00` to `63`,
  empty files, created once, never through a link: a guest with a writable
  mount could plant one), and the guest runs one command: `echo 2 >
  /proc/sys/vm/drop_caches`, `stat` of the forget sets, and the drop again.
  The first drop forgets every unused inode; the lookups give each mount 64
  fresh inodes, and the second drop forgets them, which carries every earlier
  forget through the server. Afterwards only forget-set files can be pending,
  and they are not deleted while the VM mounts them. The release reports
  deleted files and bytes before and after, from one `lsof` each. A VM process
  is matched to its instance by the disk image it holds open; only numbered
  descriptors count. The reported counts are per file (lsof's device and
  inode): a file a guest process has open holds two descriptors and counts
  once. The release record keeps every descriptor. A mount whose forget set
  cannot be written, or is a link, fails alone (`mount_errors`, exit 1); the
  release runs through the other mounts, whose
  forgets free every mount's pins because the server counts per VM process.
  Each release records the deleted files it left held, which guest processes
  hold open, in `<instance>/molt-linux-vm-release.json` with the VM process's
  pid. It never stops a VM.
- Lane retirement (`cleanup_worktree`, `drift_harvest --prune`) calls
  `release_for_paths` just before `git worktree remove`, while the checkout
  exists. After the removal the process can import nothing from the checkout
  and can launch no guarded command, whose guard runs from the checkout's
  sources. With no live hostagent it reads one directory. When every live VM
  runs its own definition (its `lima.yaml` equals the rendering, its
  hostagent started after that file, and no `$LIMA_HOME/_config/default.yaml`
  or `override.yaml` merges into it), nothing can be pinned and it spawns
  nothing. Otherwise it lists the instances; a VM that mounts a retiring path
  releases only when its process holds files under it, or holds deleted
  files other than exactly those its last release recorded. A release frees
  every deleted file no guest process holds; what it leaves, a guest process
  holds open, and a release cannot free it. So the next retirement releases
  again only for a new pin, a held file the guest closed (its open
  descriptor is gone, so its lookup can go), or a new VM process; a missing
  or unreadable record always releases. Any failure is reported and never
  stops the cleanup.
- `apply` validates the rendering with `limactl validate`, stops the VM,
  installs the rendering and starts the VM. The start is a service launch
  (`Host.start_service`): `limactl start` leaves Lima's hostagent running,
  and Lima owns its lifecycle (`limactl stop` ends it). Every other host
  command runs under Molt's guard, which closes every descendant when its
  command returns; on 2026-10-22 that closed both hostagents in the second
  their VMs booted (HF-471). The service launch runs `limactl start`
  unguarded, in its own session and process group, from `/` (the hostagent
  inherits no checkout as its working directory), with its output in
  anonymous files (a hostagent that inherited them cannot delay the
  return), with an environment that names nothing a guard retires (no guard
  token, marker or scratch: `molt.temporary_artifacts.environment_outliving_guard`,
  the backend suite daemon's authority too), and with a deadline that ends
  only `limactl` itself. The subprocess-guard audit lists this one launch
  and why (`tools/check_subprocess_guard_coverage.py`). The VM counts as
  started only when `limactl start` exits 0 and Lima's listing then shows it
  running; apply never reports success from the exit status alone. An
  enclosing Molt memory guard would close the hostagent the same way: the
  guard follows parents, not sessions. So `apply` refuses to restart a
  running VM under a live guard, before it touches the guest. It asks the
  guard authority (`molt.pytest_memory_guard_bootstrap.live_enclosing_guard_pid`):
  the guard the environment names (`MOLT_MEMORY_GUARD_ACTIVE=1` and its pid)
  counts while that process lives, from any checkout, whether it runs as
  `tools/memory_guard.py` or inside a tool's own process (as
  `harness_memory_guard` runs it). A dead guard's leftover variables do not
  block `apply`; a guard that dropped those variables is not seen. The
  narrower `outer_memory_guard_active`, which counts only this checkout's
  guard, serves test custody, where a miss starts one more guard.
- When the VM does not run after its start, `apply` restores the config it
  replaced, starts the VM again the same way and checks the listing again.
  When Lima's listing fails after the start, the VM does not count as
  started, and its hostagent's `ha.pid` says whether it runs. When `limactl
  start` failed but the VM runs (a timeout, a degraded requirement), it
  reports the failure and does not restart the VM again. It keeps the first
  config it replaced as `lima.yaml.molt-previous` and never overwrites that
  file. After a successful apply it removes the forget sets of the mounts it
  ended, unless another instance's config still mounts that location or a
  parent of it, and the VM's release record. It removes only the set's own
  empty numbered files, through directory descriptors that follow no link,
  so a set or entry a guest replaced with a link stays, with its target.
- `apply` refuses without `--confirm-restart`, refuses a vz/arch change (only
  a new VM can make one), and refuses while the guest is busy. A process is
  idle only by its executable: the kernel's path for it
  (`/proc/<pid>/exe`, listed as root by `sudo -n ps -o exe`) is one of the
  definition's `idle_guest_executables`. A name proves nothing: rootlesskit's
  child is called `exe`, as is any program that re-executes
  `/proc/self/exe`, and any program can be called `containerd`. The user's
  `sshd` and `(sd-pam)` are not dumpable, so only root can read their
  executables. The probe runs `sudo -n true` first: without passwordless
  sudo it fails and apply refuses, naming passwordless sudo; a failed `ps`
  is reported as itself.
  Busy are:
  - a process of the guest user that is not idle;
  - a process in a login session (`session-*.scope`) that is not idle,
    whatever its user, such as root work under `sudo`;
  - a process under the user's subordinate ids (`/etc/subuid`), such as a
    rootless container's non-root user; a containerd shim also counts,
    because one runs only while a container runs;
  - `apt`, `apt-get`, `dpkg` or `cloud-init` of any user (by name; a false
    alarm is safe);
  - a process whose executable the probe could not read.

  The probe's own shell, its ancestors and its descendants are excluded. An
  executable replaced on disk (`/usr/sbin/sshd (deleted)` after an upgrade)
  matches by its path. The limits: it does not see root work outside a login
  session (a `systemd-run` unit, a cron job); a guest user who runs an idle
  executable by hand (`containerd` from a shell) counts as idle; and nothing
  stops a session that starts between the probe and `limactl stop`.

## Measured costs

macOS arm64, Lima 2.2.1, 2026-10-22, host load 7-9, Molt's guarded command
executor:

- `check`: 0.8-1.3 s wall for both VMs at host load 7-9, including Python
  startup, `limactl list` and one `lsof` (the review measured 0.47 s).
- One guest release command: 60 ms wall for one `limactl shell` (unguarded),
  96-133 ms per VM through the tool.
- Retirement: no live hostagent, one directory read; every live VM on its
  definition, 0.4-0.9 ms with no process (a Lima home laid out like the
  applied VMs); a path no VM mounts, 85-90 ms; a mounted path with nothing
  held, 213 ms; a mounted path the guests had read, 565-622 ms for both VMs
  (the first call in a process adds 170-300 ms).
- Transfers, above (Options).

After `apply` (both VMs on their definitions, 2026-10-22, host load 9-12):

- `check`: 0.22-0.65 s wall for both VMs over four runs; with no mount there
  is no `lsof`.
- Retirement: 1.31 ms for the first call in a process, which reads the
  definitions and renders both VMs; no process.
- `apply`'s idle probe: 40-50 ms per guest (140 and 119 processes, 34 and
  32 executables readable as root).
- `push --checkout`'s guest checks: 50-60 ms per guest (`sudo -n true`, a
  `/proc/*/cwd` listing of 142 and 121 entries, `git status` of a clean
  clone), read-only parts measured without the checkout. The process check
  that also reads open files (the same day, read-only, run by the guests'
  `dash` and GNU `find`, globs expanded by root): 16-27 ms per guest for
  `sudo -n true` and one `find` over `/proc/*/cwd` and `/proc/*/fd` (936-940
  entries in `molt-linux`, 781-783 in `molt-x86`). Both controls passed in
  both guests: init's `/proc/1/cwd` read as `/`, and the checking shell's
  own entry was there. Asked about `/dev`, it named 32 and 30 processes;
  asked about the clone or a missing path, none.
- A boot after `apply`: 9 s (`molt-linux`) and 12 s (`molt-x86`) from the
  hostagent's start to Lima's "final requirement" (`ha.stderr.log`; the
  operator's own `limactl start`).

## Verified on the real VMs

- `apply` on 2026-10-22 installed both definitions and kept each first
  replaced config as `lima.yaml.molt-previous` (`molt-linux` mounted `~`,
  `molt-x86` mounted `~/Molt` read-only). Its start then ran under the
  guard, which closed both hostagents as they booted (HF-471); the operator
  restarted both with `limactl start`. `check` exits 0, and no guest mounts a
  host directory: `molt-x86`'s one virtiofs mount is Apple's Rosetta share
  (`/mnt/lima-rosetta`, tag `vz-rosetta`), which Rosetta needs.
- Lima 2.2.1 does not rewrite `lima.yaml` when it starts a VM: both files
  are older than their hostagents' `ha.pid` (15:10:24 against 15:12:03,
  15:09:42 against 15:11:50), so `runs_its_definition` holds for both and
  retirement spawns nothing.
- The idle rule reads both guests idle; the opt-in test
  (`MOLT_LINUX_VM_TESTS=1`) checks that every process of Lima's user units
  (the user's systemd, rootless containerd, BuildKit, stargz) runs a declared
  executable.

## Not verified

- `apply` through the service launch on the real VMs: that restarts them.
  The fake host proves the start goes through `start_service`, that a VM
  that stops after its start, or whose listing then fails while its
  hostagent is dead, gets its replaced config back, and that an enclosing
  guard refuses the restart. Real processes prove the
  service outlives its command in its own process group, outlives the
  deadline that ends the command (in its own group or the command's), and
  inherits none of the guard's token, marker or scratch
  (`MOLT_MEMORY_GUARD_ACTIVE` and `MOLT_MEMORY_GUARD_PID` stay, as for the
  backend suite daemon). The real `tools/memory_guard.py` marks a child that
  the guard authority then reports.
- A real `push` and `--checkout` into the guests: they write guest state.
  Neither guest clone has a `refs/heads/host/` branch yet. Real Git with a
  fake `ssh` proves the push, and the real checkout command, run by `dash`
  (the guests' `/bin/sh`) against a clone with a fake `/proc` listing of
  working directories and open files, proves its checks; its process check
  ran read-only in both guests (Measured costs).
- The batch size on other macOS releases. It is Apple's undocumented
  behavior; the opt-in real-VM test (`MOLT_LINUX_VM_TESTS=1`) fails if a
  release stops freeing pins (it skips while no VM mounts a host path).
- Hosts without Lima (Linux, Windows) run no VM; the tool reports nothing to
  check.
