# Maintained object 0.40.0 source

Published package: https://static.crates.io/crates/object/object-0.40.0.crate

Archive SHA-256: `dd229a0361b9d0d4396176e02d65897f487eebeab7caa6d443855ee152ca0b9c` (verified against the
crates.io index and the workspace lock before path replacement). The complete published package
is retained, except its library `Cargo.lock`: Molt's workspace lock is the dependency selection
authority. Upstream license files are unchanged: LICENSE-APACHE, LICENSE-MIT.

Upstream 0.40.0 still lacks this contract: its `coff_adjust_addend` returns a zero offset for every
ARM64 relocation except `IMAGE_REL_ARM64_REL32`, so the generic whole-word writer would corrupt
ADRP/ADD instruction fields. The patch below was carried forward from 0.39.1 unchanged; it applies
to 0.40.0 without fuzz.

The COFF writer owns ARM64 instruction-field implicit addends for
`IMAGE_REL_ARM64_PAGEBASE_REL21` and `IMAGE_REL_ARM64_PAGEOFFSET_12A`.
`Object::coff_adjust_addend` receives the section, preserves opcode/register
bits, checks instruction bounds and the signed 21-bit ADRP byte-addend range,
and finishes the implicit encoding before the generic whole-word writer.
Zero explicit addends preserve already encoded instruction addends.
`Object::add_relocation_internal` passes the section to that authority.

The encoding follows LLVM 22.1.8's `AArch64AsmBackend::adjustFixupValue` and
`lld/COFF/Chunks.cpp::applyArm64Addr`: COFF's ADRP addend is a signed **byte**
offset, added to the symbol before page rounding. The ADD relocation carries
its low 12 bits. The upstream LLVM `MC/AArch64/coff-relocations.s` 0x12345
encoding is retained as an independent regression vector alongside boundary,
negative, register-preservation, buffer-bounds and failure-atomicity cases.

The root `Cargo.toml` selects this source through `[patch.crates-io]` for every `object ^0.40`
requirement: cranelift-object, wasmtime and Molt's own crates. `ar_archive_writer` 0.5.3 (the
latest release) still requires `object ^0.39`, so it resolves the unpatched crates.io 0.39.1; it
enables only the `read` feature and builds COFF import members from raw PE structures, and this
patch changes only `object::write`'s COFF writer.
Retire the local patch only after an upstream version implements this complete
contract and the actual native COFF consumer and addend controls pass against
that version. The compiler source payload includes this directory and the
canonical Cargo source closure includes its bytes when the locked dependency
graph selects this package.
