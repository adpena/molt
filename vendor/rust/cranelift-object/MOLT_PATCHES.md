# Maintained cranelift-object 0.137.0 source

Published package: https://static.crates.io/crates/cranelift-object/cranelift-object-0.137.0.crate

Archive SHA-256: `78f98e6d6b502fa10bd1bb95776544033169d0c1860d6fe1ada03109ea500eb4` (verified against the crates.io
registry checksum before path replacement, 2026-10-20). The complete published package is retained, except its
library `Cargo.lock`: Molt's workspace lock is the dependency selection
authority. Upstream license files are unchanged: LICENSE.

`ObjectModule::process_reloc` maps `Aarch64AdrPrelPgHi21` and
`Aarch64AddAbsLo12Nc` to COFF's `IMAGE_REL_ARM64_PAGEBASE_REL21` and
`IMAGE_REL_ARM64_PAGEOFFSET_12A`, respectively. LLVM 22.1.8's
`AArch64WinCOFFObjectWriter::getRelocType` owns the matching mapping.
The regression compiles an actual colocated symbol with a nonzero offset,
then checks the emitted COFF page relocations and LLVM's byte-addend vector.
It materializes the address with `symbol_value`: 0.137 removed the `global_value`
instruction, which 0.133's legalizer lowered to the same `symbol_value` for a non-TLS
symbol, and it finalizes with `module.target_config()` as the upstream 0.137 tests do.

This patch requires the maintained `object` writer selected by the root
workspace patch table. It does not map GOT-load relocations to direct-symbol
relocations: those operations have different semantics. The native target
constructor selects non-PIC COFF addressing; ELF and Mach-O keep their PIC
policy. No extra CLIF scan or function-reference duplication is needed.

The published 0.137.0 `Cargo.toml` already uses the underscore lint spellings that Cargo's
[manifest lint normalization](https://github.com/rust-lang/cargo/pull/17051) requires, so the
manifest is unmodified; `Cargo.toml.orig` remains the unmodified archive source.

The root `Cargo.toml` selects this source through `[patch.crates-io]`.
Retire the local patch only after an upstream version implements this complete
contract and the actual native COFF consumer and addend controls pass against
that version. The compiler source payload includes this directory and the
canonical Cargo source closure includes its bytes when the locked dependency
graph selects this package.
