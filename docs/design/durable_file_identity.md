# Durable filesystem object identity

`molt.durable_file_identity` owns the exact wire shape `{kind, volume, file_id}`.
`kind` is a closed volume authority; `file_id` is 32 lowercase hex digits.
Operation-local stat fences continue to use device/inode while the handles or
operation are alive. Persistent volume identities contain no device number.

macOS obtains `ATTR_VOL_UUID` through `fgetattrlist` for an existing handle or
`getattrlist(FSOPT_NOFOLLOW)` for a metadata-only named observation. Returned
attribute masks and length must report a supported, nonzero UUID. XNU holds the
queried vnode and resolves its mount root internally for volume attributes.
Named capture fences device/inode, mode, size and both change clocks around the
query. It requires read-attributes access, preserving observations of directories
without read/search permission; it never opens their contents. The file id is
the observed inode. A combined non-root file ID/volume UUID query is unavailable:
XNU's [`ATTR_CMN_VOL_INVALID` mask](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/vfs/vfs_attrlist.c)
rejects `ATTR_CMN_FILEID` when volume attributes are requested, and volume
queries substitute the mount root. Molt therefore keeps the fenced file stat
and UUID query separate, rather than treating a root's file ID as the leaf's.
Linux maps the live file's device to a **block node's rdev**
in `/dev/disk/by-uuid`; absent or ambiguous mappings on non-volatile filesystems
refuse durable custody with a remedy to provide a unique filesystem UUID mapping.
Linux first queries the held descriptor's filesystem type. For tmpfs and ramfs,
`boot-scoped` contains the kernel boot ID plus the mount's **unique** ID from
`statx(fd, "", AT_EMPTY_PATH, STATX_MNT_ID_UNIQUE, ...)`. The ctypes layout and
mask follow [Linux v6.8's UAPI stat.h](https://github.com/torvalds/linux/blob/v6.8/include/uapi/linux/stat.h)
(256 bytes, mount ID at 0x90, mask 0x4000); flags follow that revision's
[fcntl.h](https://github.com/torvalds/linux/blob/v6.8/include/uapi/linux/fcntl.h).
The wire value is `<32-hex boot ID>.mnt-<16-hex unique mount ID>`. The old device
form is invalid. [statx(2)](https://man7.org/linux/man-pages/man2/statx.2.html)
guarantees that the unique ID, available since Linux 6.8, is not reused during
the running system's lifetime. A bind mount of the same tmpfs has a different
mount ID, so identity comparison conservatively refuses it.

Boot ID plus device is unsound: [Linux v6.8 fs/super.c](https://github.com/torvalds/linux/blob/v6.8/fs/super.c)
allocates anonymous devices with `ida_alloc_range` in `get_anon_bdev` and frees
them with `ida_free` in `free_anon_bdev`, called on unmount. A replacement tmpfs
within one boot can reuse its device and inode. No such pair is an authority.
Memory scratch is an opt-in consumer. Before allocation it requires the unique
mount-ID capability. Missing libc support, an older kernel, an unset returned
mask, or a zero mount ID raises typed `UniqueMountIdentityUnavailable`, reason
`unique-mount-id-unavailable`, with remedy `MOLT_SCRATCH_STORAGE=disk`. There is
no retry with reusable `STATX_MNT_ID`, a device number or another identity kind.
Windows uses `GetFileInformationByHandleEx(FileIdInfo)`, retaining the 64-bit
volume serial and all 128 file-id bits; directory opens use backup semantics
and reparse-point opens do not traverse the leaf. Unknown capabilities refuse.
There is no path-, volume-label-, device-, or inode-only fallback.

Queried volume strings are interned per process. Device-to-volume answers are
not cached: device numbers can be reused without a process restart. Each
capture queries the current volume authority and fences the direct node's
metadata; handle-based capture also fences the held descriptor.
macOS therefore issues one UUID query per capture; Linux enumerates the UUID
links per capture, with one followed stat per entry; Windows issues one
FileIdInfo query. This favors live mount correctness over a stale device-keyed
cache. It is a syscall/allocation description, not a measured latency claim.
Captured regular-file versions memoize their durable projection only when a
persisted token is requested; existing operation-only captures acquire no new
filesystem capability requirement.

The internal `StatToken` carries `DurableFileIdentity` and named mutation
fields. Persisted tokens decode at their IO boundary before comparison; wire
labels never decide physical equality. Volume-kind validation and equality
match the closed enum exhaustively and finish with `typing.assert_never`.

## Migration and interruption

Only explicit canonical migrators read a legacy identity. Both recorded stat
fields must match the current direct node before stable volume capture. New
readers use only the current representation. An old identity after device
renumbering cannot establish its former filesystem and never grants adoption,
movement or deletion.

Every host-state schema owns a versioned namespace. Old readers and sweepers
never encounter new records. Bindings use `artifact-root.v2.json`, guard scratch
uses `tmp/gs/v2/`, suite leases use `suite-leases/v3/`, digest rows use
`file-digests/v2.sqlite`, probes use `toolchain-facts/v2.json`, source hashes use
`source_hash_cache/v2/`, build records use `build_records/v11/`, and backend sync
receipts use `artifact_sync/v4/` and immutable sidecars use `.sync.v4.json`.
Preflight receipts use `logs/runtime_wasm_final_preflight/v4/`. Cargo output declarations and transcript
identities are published in `execution-v6/` request/result pairs; Cargo payload
placement uses `molt-proof-output/v2/`. Publication journals have a `v3-` name
and retired leaves have a `v2-` name. Historical immutable evidence is not
rewritten. Only binding provisioning and recovery-journal owners read an old
namespace through their canonical locked boundary.

Canonical checkout provisioning upgrades a matching v1 binding under its
existing provisioning lock and atomically publishes v2 alongside v1. A mismatch refuses
with `artifact-root --bind`; replaced, unmounted and unhealthy roots refuse.
Rebuildable state never migrates: guard and suite scratch, digest rows, probe
answers, source hashes, build records, sync receipts and Cargo output declarations
start cold in the current namespace. New readers do not inspect older
namespaces. HF-582's storage authority owns their retirement by last use and
held-lock exclusion; this module adds no namespace sweeper or retirement owner.
The preserved v1 scratch implementation under tests is only a provenance-pinned
oracle for concurrent version isolation.

`dx.ARTIFACT_BINDING_TRANSITIONS` owns binding events: absent binding,
exactly witnessed legacy binding, stable identity replay, and substitution.
`artifact_publication.JOURNAL_TRANSITIONS` owns open, committed, rolled-back,
recovered and refused journal outcomes. Illegal events refuse. An older journal
that cannot recover reports its exact path and a restoration/retry remedy.

[The state-machine proof](../../tests/test_durable_identity_state_machine.py)
drives reboot renumbering, substituted volumes, unique-mount remounts, concurrent
v1 scratch publication and interrupted older journals in real temporary roots.
Native stat/volume/statx answers are its substituted inputs. After every event,
the model checks refusal of substituted volumes, other-version byte custody,
and discoverable recovery or a named refusal for every older journal.

Publication journals re-key all still-witnessed members before recovery and
refuse an unverifiable member. Mixed old/new journal copies after an interrupted
rewrite normalize to the same core before any recovery action. Retirement
names use v2 and a digest of the stable identity, keeping component length
bounded even for a long UUID. The existing exclusive retirement owner can
re-key a v1 name only while its old pair matches. Unproven leaves remain put.

Current Cargo-root declarations and command transcript records use stable
identities. Immutable historical evidence stays in its original namespace; it
cannot qualify a current request without current admission.

## Primary authorities

No upstream implementation code is copied.

- [Apple XNU attributes](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/attr.h)
  defines UUID and returned-attribute masks;
  [volume query implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/vfs/vfs_attrlist.c)
  resolves the mount root and rejects dead mounts.
- [Linux filesystem magic values](https://github.com/torvalds/linux/blob/master/include/uapi/linux/magic.h)
  identify tmpfs and ramfs at the held descriptor boundary.
- [systemd boot ID](https://www.freedesktop.org/software/systemd/man/latest/sd_id128_get_boot.html)
  reads the kernel boot ID, unique for each running kernel instance; this is
  prior art for the explicit volatile-volume lifetime.
- [systemd persistent-storage rules](https://github.com/systemd/systemd/blob/main/rules.d/60-persistent-storage.rules.in)
  create filesystem by-uuid links; these differ from partition identities.
- [Microsoft FILE_ID_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info)
  defines the volume serial plus 128-bit file-id pair.
- [Git statinfo](https://github.com/git/git/blob/master/statinfo.c) persists device
  numbers but compares them only with `USE_STDEV`;
  [core.checkStat](https://git-scm.com/docs/git-config#Documentation/git-config.txt-corecheckStat)
  describes its metadata policy. Content-index reuse is not deletion custody.
- [Watchman FSEvents](https://github.com/facebook/watchman/blob/main/watchman/watcher/fsevents.cpp)
  binds old offsets to their journal UUID. That UUID is not the filesystem UUID.
