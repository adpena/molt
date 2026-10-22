# Linux VM apparatus

Status: implemented (HF-463). Authority: [`tools/linux_vm.toml`](../../tools/linux_vm.toml).
Its one consumer: [`tools/linux_vm.py`](../../tools/linux_vm.py). Runbook:
[OPERATIONS.md, "Linux VMs (Lima)"](../OPERATIONS.md#linux-vms-lima).

Molt reproduces Linux failures on the macOS host in two Lima VMs under the
`vz` driver: `molt-linux` (aarch64) and `molt-x86` (x86_64 through Rosetta).
They were made by hand, and no tool owned them. Three defects followed:

- **Pinned host files.** A guest pins every host file it has looked at
  through its virtiofs share. When the host deletes such a file, its space
  stays allocated until the guest evicts the inode. On 2026-10-22 the
  `molt-linux` VM process held 802 files under `~/Molt` and 167 deleted files;
  on 2026-10-17 `molt-x86` held 188 files of worktrees in the Trash, and the
  VM had to stop before the Trash could be emptied.
- **Drift.** The notes said both VMs mount `~/Molt` read-only; `molt-linux`
  mounted the whole home.
- **Port leaks.** Lima forwards every guest port to the host by default. In
  HF-F251 a guest sccache server took over the host's sccache port. Since
  their boot on 2026-10-21 the two hostagents had forwarded 13 and 832 guest
  ports to 127.0.0.1 on the Mac.

## How each mount type holds host files

- **virtiofs (Apple's server).** The Virtualization.framework process keeps
  one host file open for each inode the guest knows, until the guest sends
  `FUSE_FORGET`. An Apple engineer: "Unless the guest sends `FUSE_FORGET`,
  there is no harm in keeping them around for performance"
  ([Apple Developer Forums 741572](https://developer.apple.com/forums/thread/741572)).
  The share has no cache or handle option: Lima builds it from a location and
  a read-only flag only
  ([lima v2.2.1 `pkg/driver/vz/vm_darwin.go`](https://github.com/lima-vm/lima/blob/v2.2.1/pkg/driver/vz/vm_darwin.go#L674-L684)).
- **The guest side.** Linux sends `FORGET` "when the kernel removes an inode
  from its internal caches" ([libfuse `fuse_lowlevel.h`](https://github.com/libfuse/libfuse/blob/master/include/fuse_lowlevel.h)).
  A deletion on the host does not tell the guest, so the inode stays cached
  until memory reclaim or `drop_caches` evicts it. `vfs_cache_pressure` only
  weights reclaim; an idle guest does not reclaim.
  `echo 2 > /proc/sys/vm/drop_caches` frees "reclaimable slab objects
  (includes dentries and inodes)" and is "a non-destructive operation"
  ([kernel vm sysctl](https://docs.kernel.org/admin-guide/sysctl/vm.html)):
  an open file keeps its inode.
- **Linux virtiofsd** has the same class (an `O_PATH` descriptor per inode)
  and avoids it with `--inode-file-handles`
  ([virtiofsd README](https://gitlab.com/virtio-fs/virtiofsd)). libkrun's
  server has it too, with no fix yet
  ([libkrun #505](https://github.com/libkrun/libkrun/issues/505)). Apple's
  server offers no such mode.
- **reverse-sshfs.** SFTP names files by path and holds a host handle only
  while the guest has the file open, so it pins nothing else. But the host's
  `sftp-server -R` is not confined to the share: "A compromised `sshfs`
  process in the guest may have access to unexposed host directories"
  ([Lima mounts](https://lima-vm.io/docs/config/mount/)). Lima's hostagent
  does not reconnect a mount whose SSH connection drops
  ([`pkg/hostagent/mount.go`](https://github.com/lima-vm/lima/blob/v2.2.1/pkg/hostagent/mount.go)).
- **9p** is QEMU-only; the vz driver rejects it
  ([`vz_driver_darwin.go`](https://github.com/lima-vm/lima/blob/v2.2.1/pkg/driver/vz/vz_driver_darwin.go#L276-L277)).

Other products: Docker Desktop defaults to VirtioFS, which "has reduced the
time taken to complete filesystem operations by up to 98%" against its gRPC
FUSE share ([Docker settings](https://docs.docker.com/desktop/settings-and-maintenance/settings/)),
and its synchronized file shares copy files into the VM, so they hold no host
handle. In the Apple thread a Docker user reports that only a VM restart clears
the handles. Colima and Rancher Desktop run on Lima and share its mount types.

## Options

| Option | Pinning | Cost |
|---|---|---|
| virtiofs, mount narrowed to the custody root, release on deletion (chosen) | Removed outside the custody root. Inside it, released when Molt deletes a checkout and on demand. | Per lane retirement: about 0.13 s with VMs running, 0.29 s when one drops its caches (measured below). |
| reverse-sshfs | Only files a guest process has open. | The guest can read every host file the user can. The mount dies with its SSH connection until the VM restarts. Slower: Docker measured up to 98% slower filesystem operations on its FUSE share; not measured here, because this change may run no guest command but `drop_caches`. |
| No mount: the host pushes commits over Lima's SSH and copies files in | None. | Three documented workflows change: fetching from `<root>/molt-src`, copying worktree files, running scripts staged in `<root>/tmp`. Whether a guest clone depends on the mount (a remote or `alternates` path) is unknown; this change may not inspect the guests. |
| QEMU with 9p | Only open files (9p fids name paths). | A new VM each; no Rosetta, which `molt-x86` needs; slower than vz. |
| A guest timer that drops caches, or a high `vfs_cache_pressure` | A timer bounds a pin's life; the sysctl does not bound it on an idle guest. | A timer drops the guest's own build caches on a schedule, whether or not the host deleted anything. |

The chosen option removes by construction what the other costs allow: pins
outside the custody root and every undeclared port forward. It measures and
releases the pins it cannot remove. The no-mount option removes them all and
is the next step if the workflows move to host-pushed transfers.

## Definition

`tools/linux_vm.toml` holds each VM's type, architecture, resources, image,
Rosetta, containerd, Lima parameters, mounts and port forwards. A mount names
a closed source, `custody-root`: `molt.custody_layout.custody_root` of the
family's main checkout (`<root>` for `<root>/molt-src`), so the file holds no
personal path and every checkout of a family renders the same mount. Port
forwards list only what a guest may expose; the renderer appends the rule
`{guestIP: 0.0.0.0, proto: any, ignore: true}`. Lima checks rules in order and
its last built-in rule forwards every guest port on 127.0.0.1
([`default.yaml`](https://github.com/lima-vm/lima/blob/v2.2.1/templates/default.yaml));
a rule whose guest IP is `0.0.0.0` matches every guest address
([`pkg/portfwd/forward.go`](https://github.com/lima-vm/lima/blob/v2.2.1/pkg/portfwd/forward.go)),
so the appended rule matches first. Lima's SSH forward is not a rule and
stays.

The rendered `lima.yaml` declares every key that decides what a rule forwards,
so Lima's filled configuration equals the rendering and `check` compares the
two without copying Lima's defaults. One default is read, not declared:
`limactl list` omits an undeclared `mountType`, which the vz driver fills with
virtiofs.

## Commands

- `check` compares `limactl list --json` with each definition, field by field,
  and exits 1 on drift. A running VM whose `lima.yaml` is newer than its
  hostagent's `ha.pid` runs an older configuration: drift `restart_pending`.
  One `lsof` call reports what each VM process holds. It runs nothing in a
  guest.
- `release` runs `echo 2 > /proc/sys/vm/drop_caches` in each running guest
  and reports deleted files and bytes before and after. A VM process is
  matched to its instance by the disk image it holds open. Only numbered
  descriptors count, so the process's own mappings do not.
- Lane retirement (`cleanup_worktree`, `drift_harvest --prune`) runs the
  release just before `git worktree remove`, while the checkout exists. After
  the removal the process can import nothing from the checkout and can launch
  no guarded command, whose guard runs from the checkout's sources. A VM drops
  its caches only when it mounts the checkout or a state root and its process
  holds files there, or deleted files, such as packs that `git gc` replaced.
  With no live hostagent the release reads one directory and spawns nothing.
  `tools/disk_guard.py` spawns nothing by contract, and on the measured host
  its roots lie under an artifact root on a data volume, outside every VM
  mount, so retention gets no release.
- `apply` validates the rendering with `limactl validate`, refuses without
  `--confirm-restart` or while the guest runs any process of the guest user
  outside its session and daemon set, or `apt`, `dpkg` or `cloud-init`. It
  stops the VM, installs the rendering (keeping `lima.yaml.molt-previous`),
  starts the VM and checks it again. It refuses a vz/arch change, which only a
  new VM can make.

## Measured costs

macOS arm64, Lima 2.2.1, 2026-10-22, Molt's guarded command executor:

- `check`: 0.24-0.30 s wall for both VMs, including Python startup.
- A drop on `molt-linux` took 68 ms and freed 45 of 47 deleted files
  (2.60 MB to 17 KB); open files under its mounts fell from 633 to 8. The
  host closed the files at once: samples 0.2 s to 2 s later were the same.
  The two deleted files left (17 KB of Git objects) are still in use in the
  guest; a later `release` (74 ms per VM) kept them, as it must.
- One guarded command: `limactl list` 50 ms, `lsof` 81 ms, a guest drop 70 ms.
  `limactl shell` reuses Lima's SSH control master; the drop took 50-60 ms
  unguarded, not the 1-2 s once assumed.
- Lane retirement: no live hostagent 0.011 ms; VMs running but not mounting
  the checkout 51 ms; mounting it with nothing held about 131 ms; with a drop
  294 ms.

## Not verified

- `apply` on the real VMs, and its restart time. The integrator runs it after
  checking that no one uses the VMs.
- Whether a guest workflow reads host paths outside the custody root, which
  `molt-linux` could read until `apply` narrows its mount.
- Whether the guests' clones depend on the mount.
- Hosts without Lima (Linux, Windows) run no VM; the tool reports nothing to
  check.
