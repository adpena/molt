//! Native object inspection and final-link fixtures shared by backend tests.

use std::collections::BTreeSet;

pub(super) struct NativeObjectSymbols {
    pub(super) defined: BTreeSet<String>,
    pub(super) undefined: BTreeSet<String>,
}

pub(super) fn native_object_symbols(bytes: &[u8]) -> NativeObjectSymbols {
    use object::{BinaryFormat, Object, ObjectSymbol};
    let object = object::File::parse(bytes).expect("parse native object");
    let mut symbols = NativeObjectSymbols {
        defined: BTreeSet::new(),
        undefined: BTreeSet::new(),
    };
    for symbol in object.symbols() {
        let name = symbol.name().expect("native symbol name");
        let name = if object.format() == BinaryFormat::MachO {
            name.strip_prefix('_').unwrap_or(name)
        } else {
            name
        };
        if symbol.is_undefined() {
            symbols.undefined.insert(name.to_owned());
        } else if symbol.is_definition() {
            symbols.defined.insert(name.to_owned());
        }
    }
    symbols
}

#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}
#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) mod native_object_execution;
