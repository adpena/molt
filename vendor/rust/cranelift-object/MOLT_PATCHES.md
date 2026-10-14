# Maintained cranelift-object 0.133.3 source

Published package: https://static.crates.io/crates/cranelift-object/cranelift-object-0.133.3.crate

Archive SHA-256: `56734d5f5664ff733b79eb0ec4c85b663407a5ab1e8ebc37bf0a36ba5107b08f` (verified against the workspace lock before
path replacement). The complete published package is retained, except its
library `Cargo.lock`: Molt's workspace lock is the dependency selection
authority. Upstream license files are unchanged: LICENSE.

`ObjectModule::process_reloc` maps `Aarch64AdrPrelPgHi21` and
`Aarch64AddAbsLo12Nc` to COFF's `IMAGE_REL_ARM64_PAGEBASE_REL21` and
`IMAGE_REL_ARM64_PAGEOFFSET_12A`, respectively. LLVM 22.1.8's
`AArch64WinCOFFObjectWriter::getRelocType` owns the matching mapping.
The regression compiles an actual colocated symbol with a nonzero offset,
then checks the emitted COFF page relocations and LLVM's byte-addend vector.

This patch requires the maintained `object` writer selected by the root
workspace patch table. It does not map GOT-load relocations to direct-symbol
relocations: those operations have different semantics. The native target
constructor selects non-PIC COFF addressing; ELF and Mach-O keep their PIC
policy. No extra CLIF scan or function-reference duplication is needed.

The root `Cargo.toml` selects this source through `[patch.crates-io]`.
Retire the local patch only after an upstream version implements this complete
contract and the actual native COFF consumer and addend controls pass against
that version. The compiler source payload includes this directory and the
canonical Cargo source closure includes its bytes when the locked dependency
graph selects this package.
