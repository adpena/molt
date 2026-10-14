# Maintained object 0.39.1 source

Published package: https://static.crates.io/crates/object/object-0.39.1.crate

Archive SHA-256: `2e5a6c098c7a3b6547378093f5cc30bc54fd361ce711e05293a5cc589562739b` (verified against the workspace lock before
path replacement). The complete published package is retained, except its
library `Cargo.lock`: Molt's workspace lock is the dependency selection
authority. Upstream license files are unchanged: LICENSE-APACHE, LICENSE-MIT.

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

The root `Cargo.toml` selects this source through `[patch.crates-io]`.
Retire the local patch only after an upstream version implements this complete
contract and the actual native COFF consumer and addend controls pass against
that version. The compiler source payload includes this directory and the
canonical Cargo source closure includes its bytes when the locked dependency
graph selects this package.
