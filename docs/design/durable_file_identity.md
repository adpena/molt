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
the observed inode. Linux maps the live file's device to a **block node's rdev**
in `/dev/disk/by-uuid`; absent or ambiguous mappings on non-volatile filesystems
refuse durable custody with a remedy to provide a unique filesystem UUID mapping.
Linux first queries the held descriptor's filesystem type. For tmpfs and ramfs,
the typed `boot-scoped` volume is the kernel boot ID from
`/proc/sys/kernel/random/boot_id` plus the filesystem's `st_dev`. Its wire value
is the normalized 32-hex boot ID, a dot, and 16 hex device digits. It is valid
for one boot, matching volatile content's lifetime; the same device and inode
on a different boot cannot admit an old record. This is an explicit volatile
volume authority, never a fallback for an unidentified persistent filesystem.
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
receipts use `artifact_sync/v4/`. Cargo output declarations and transcript
identities are published in `execution-v6/` request/result pairs; Cargo payload
placement uses `molt-proof-output/v2/`. Publication journals have a `v3-` name
and retired leaves have a `v2-` name. Historical immutable evidence is not
rewritten. Only the canonical locked migrator reads an old namespace.

Canonical checkout provisioning upgrades a matching v1 binding under its
existing provisioning lock and atomically publishes v2 alongside v1. A mismatch refuses
with `artifact-root --bind`; replaced, unmounted and unhealthy roots refuse.
The guard retention sweep migrates a matching scratch owner, terminal, digest,
pending index and nonce as a single recoverable bundle after moving the held
generation into v2. A versioned namespace intent recovers either side of that
move; the generation intent holds
only new records and hashes of prior records; a locked reader finishes it before
consuming the new protocol. An unverifiable legacy pending grant is indexed
under `identity-unverifiable/`, and leaves the sweep without touching its owner,
terminal or payload. That entry has no ownership or deletion authority; normal
unowned last-use retention remains responsible for its payload.

Publication journals re-key all still-witnessed members before recovery and
refuse an unverifiable member. Mixed old/new journal copies after an interrupted
rewrite normalize to the same core before any recovery action. Retirement
names use v2 and a digest of the stable identity, keeping component length
bounded even for a long UUID. The existing exclusive retirement owner can
re-key a v1 name only while its old pair matches. Unproven leaves remain put.

Build records, source hashes, digest rows, toolchain probe keys and backend sync
receipts re-key matching generations through their existing owners; changed or
unprovable generations miss/refuse. Current Cargo-root declarations and command
transcript records use stable identities. Historical immutable proof material
is retained as evidence: migration at its reader proves the old pair before
projecting a current identity; it does not rewrite a sealed historical proof or
turn an old source receipt into qualification of new source.

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
