# HF-761 Linux persistent identity and coherent renumbering proofs

Owner: lane/hf761-linux-identity. Parent owns integration, ledger and retirement.
Base: dd02641179d7e54b7f3cf9d763a182c16a27d232. Scope is host tooling,
not emitted-program semantics. This selects the brief's explicitly permitted
udev prerequisite outcome; it does not claim udev-free container support.

## Evidence and premise

The recorded portability.queue.linux failures occur after
`_renumber_stat_devices`, in validation of already-published current records.
The retained JUnit identifies tmp roots under /home/adpena/Molt/tmp/gs/v2,
not a separate test filesystem. Live inventory transport on 2026-11-04 reports
Linux 6.18.54.1-microsoft-standard-WSL2, /tmp and /home/adpena/Molt on the same
ext4 /dev/sdd root, UUID 7030c244-806d-42ea-8bd3-a926bfe6be10, with that real
/dev/disk/by-uuid link present. No mapping is fabricated.

`_renumber_stat_devices` wraps os.stat, os.lstat and os.fstat, adding 1000 to
st_dev, but does not renumber st_rdev on block nodes. Production correctly
compares the held file's filesystem device to a UUID block node's st_rdev.
Thus the fixture describes an impossible partial renumbering: mount and held
file devices change while the underlying block-device identity does not.
Three consumers use this helper: temporary artifacts, Cargo-root declarations,
and toolchain cache stat tokens. The artifact-root test's separate replay
helper already preserves its real UUID mapping; it has different semantics
(targeted inode/device substitution) and is not replaced.

History inspection found the existing typed authority and versioned records;
there is no lane WIP or alternate provider to harvest. Preserve them.

## Prior art and decision

- Linux open(2) explicitly says ioctl on O_PATH returns EBADF; O_PATH supports
  fstatfs without read access and permits direct symlink custody:
  https://man7.org/linux/man-pages/man2/open.2.html
- Linux 6.9 fs/ioctl.c uses fdget (excluding path-only handles) before dispatch
  to FS_IOC_GETFSUUID. Its UAPI fsuuid2 carries up to 16 UUID bytes:
  https://github.com/torvalds/linux/blob/v6.9/fs/ioctl.c
  https://github.com/torvalds/linux/blob/v6.9/include/uapi/linux/fs.h
- ext4 statfs uses uuid_to_fsid, which XOR-folds the two UUID halves into 64
  bits. Distinct 128-bit UUIDs with the same delta in both halves collide:
  https://github.com/torvalds/linux/blob/v6.6/fs/ext4/super.c
  https://github.com/torvalds/linux/blob/v6.6/include/linux/statfs.h
- statfs(2) gives no cross-filesystem durable f_fsid guarantee:
  https://man7.org/linux/man-pages/man2/statfs.2.html
- Existing farm provisioning already requires systemd/udev and a real root
  mapping (docs/design/trunk_loop.md and tools/trunk_loop.py).

Choose the existing full filesystem UUID provider with an explicit udev host
prerequisite. The proposed ioctl cannot replace the metadata-only authority
for unreadable leaves or symlinks. Reopening files/ancestors would add access
requirements and race/cross-mount obligations. A generic f_fsid identity would
weaken the full UUID substitution boundary, change persisted volume identities,
and require a family-wide schema migration. Neither is warranted to repair an
incoherent test fixture. This is a capability restriction, not a fallback.
A future udev-free provider needs a separately proven full-UUID, metadata-only
API across its declared filesystem/kernel cells.

## Changes and invariants

1. Keep DurableFileIdentity, LINUX_UUID, wire spelling, O_PATH/O_NOFOLLOW,
   symlink fences, metadata-only capture, and boot-scoped tmpfs/ramfs unchanged.
2. Name udev and the required unique /dev/disk/by-uuid mapping in both missing
   directory and missing/ambiguous UUID diagnostics. Do not guess UUIDs, shell
   out, cache device mappings, retry with a weaker identity, or alter records.
3. Make the shared renumber fixture also shift st_rdev for actual block nodes.
   All other st_rdev values stay unchanged; preserve nanosecond timestamps and
   inode/type fields. The existing real UUID reader remains under test.
4. Extend the existing block-device UUID test to exercise capture after this
   coherent renumbering, and strengthen missing-provider diagnostics. Include
   an unrelated block node and ambiguity refusal; retain substitution tests.
5. Update docs/design/durable_file_identity.md, onboarding prerequisites and
   the farm design's stale promise of an owned udev-free provider. README links
   to the canonical prerequisite; no duplicated support matrix.

No new identity kind, schema, registry, generated table, gate or runtime cost.
No compiler/backend/crate changes, so native/WASM/cargo proofs are unaffected.
Extra discovery/write paths: shared fixture/test owner tests/test_dx_run_context.py,
its direct reader tests/test_toolchain_identity.py, docs/getting-started.md,
README.md and docs/design/trunk_loop.md. Ledger stays parent-reserved.

## Proof plan

Observe old-behavior failures on molt-gate or reuse the retained named JUnit
trace; probe O_PATH ioctl directly on the live kernel without elevated access.
Run the affected UUID/volatile/substitution tests plus both named regressions
on Mac and molt-gate, and the shared helper's toolchain consumer. Use the slot
image locally and host-owned managed Python remotely, at <=4 pytest workers.
Plain landing-runner environment: unset CARGO_TARGET_DIR and
UV_PROJECT_ENVIRONMENT, keep TMPDIR on the lane data-volume scratch.
Run the mandatory static-only command (25 gates) and precheck-only once;
reuse matching static receipts if the canonical controller does so. Stop on
failures, classify and fix in scope. Broaden only for a newly exposed obligation.
Record commands, source/env identities, actual results and unverified cells.

## Review record

Two sequential independent `codex exec -s read-only`, gpt-6.1-sol/high reviews
received only this file and their role on 2026-11-04. Design review: no blocking
findings, low-severity request for explicit equality/persisted-record oracles.
Implementation-readiness review: medium-severity same oracle gap; no other
blocking findings. Both are resolved by the acceptance criteria below. Full
reviews are retained in the parent lane artifacts as review-design.txt and
review-readiness.txt. No reviewer ran proofs or inspected implementation.

Acceptance criteria (resolution for both findings):

- The existing block-device UUID test captures its UUID before renumbering,
  then renumbers the mounted-file device and block-node rdev and asserts the
  same UUID afterward through the real reader. It must execute without a Linux
  skip (its boundary inputs can be exercised on Mac too), preserve inode and
  mutation clocks, keep unrelated nodes unrelated, and still reject ambiguity.
- `tests/test_temporary_artifacts.py::test_v2_pending_survives_simulated_renumbering`
  publishes before renumbering, then asserts no errors, one retained record,
  byte-equivalent owner JSON and preserved payload afterward.
- `tests/tools/test_proof_queue_output_layout.py::test_current_cargo_root_survives_device_renumbering`
  declares a real root before renumbering and validates the identical persisted
  declaration afterward. Also capture DurableFileIdentity before renumbering
  and assert exact equality after it, including UUID and file ID.
- `tests/test_toolchain_identity.py::test_digest_reuse_survives_device_renumbering`
  captures/flushes before renumbering and requires the unchanged digest without
  rehashing afterward; Windows' pre-existing exclusion remains unchanged.
- Existing artifact-root different-volume substitution tests remain required;
  a new UUID must refuse custody even with the original file ID. No acceptance
  assertion is weakened. The live Linux receipt must name both regressions and
  the block-device case as passed, not skipped.
