use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_frozen_payload(machinery_bits: u64, _util_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut owned: Vec<u64> = Vec::new();
        let mut values: Vec<(&[u8], u64)> = Vec::with_capacity(3);

        let builtin_importer_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"BuiltinImporter"),
            b"BuiltinImporter",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        owned.push(builtin_importer_bits);
        values.push((b"BuiltinImporter", builtin_importer_bits));

        let frozen_importer_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"FrozenImporter"),
            b"FrozenImporter",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(frozen_importer_bits);
        values.push((b"FrozenImporter", frozen_importer_bits));

        let module_spec_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"ModuleSpec"),
            b"ModuleSpec",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(module_spec_bits);
        values.push((b"ModuleSpec", module_spec_bits));

        let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
        for (key, value_bits) in values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_typing_private_payload(typing_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut owned: Vec<u64> = Vec::new();
        let mut values: Vec<(&[u8], u64)> = Vec::with_capacity(7);

        let generic_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"Generic"),
            b"Generic",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(generic_bits);
        values.push((b"Generic", generic_bits));

        let param_spec_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"_ParamSpec"),
            b"_ParamSpec",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(param_spec_bits);
        values.push((b"ParamSpec", param_spec_bits));

        let param_spec_args_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"_ParamSpecArgs"),
            b"_ParamSpecArgs",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(param_spec_args_bits);
        values.push((b"ParamSpecArgs", param_spec_args_bits));

        let param_spec_kwargs_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"_ParamSpecKwargs"),
            b"_ParamSpecKwargs",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(param_spec_kwargs_bits);
        values.push((b"ParamSpecKwargs", param_spec_kwargs_bits));

        let type_alias_type_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"_MoltTypeAlias"),
            b"_MoltTypeAlias",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(type_alias_type_bits);
        values.push((b"TypeAliasType", type_alias_type_bits));

        let type_var_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"_TypeVar"),
            b"_TypeVar",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(type_var_bits);
        values.push((b"TypeVar", type_var_bits));

        let type_var_tuple_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"_TypeVarTuple"),
            b"_TypeVarTuple",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(type_var_tuple_bits);
        values.push((b"TypeVarTuple", type_var_tuple_bits));

        let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
        for (key, value_bits) in values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_types_payload(
    typing_bits: u64,
    abc_bits: u64,
    contextlib_bits: u64,
    _itertools_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut owned: Vec<u64> = Vec::new();
        let mut values: Vec<(&[u8], u64)> = Vec::with_capacity(11);

        let any_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"Any"),
            b"Any",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(any_bits);
        values.push((b"Any", any_bits));

        let dict_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"Dict"),
            b"Dict",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(dict_bits);
        values.push((b"Dict", dict_bits));

        let iterator_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"Iterator"),
            b"Iterator",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(iterator_bits);
        values.push((b"Iterator", iterator_bits));

        let list_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"List"),
            b"List",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(list_bits);
        values.push((b"List", list_bits));

        let mapping_bits = dict_bits;
        inc_ref_bits(_py, mapping_bits);
        owned.push(mapping_bits);
        values.push((b"Mapping", mapping_bits));

        let optional_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"Optional"),
            b"Optional",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(optional_bits);
        values.push((b"Optional", optional_bits));

        let protocol_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"Protocol"),
            b"Protocol",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(protocol_bits);
        values.push((b"Protocol", protocol_bits));

        let type_var_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"_TypeVar"),
            b"_TypeVar",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(type_var_bits);
        values.push((b"TypeVar", type_var_bits));

        let union_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"Union"),
            b"Union",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(union_bits);
        values.push((b"Union", union_bits));

        let overload_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            runtime_static_name_slot(_py, b"overload"),
            b"overload",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(overload_bits);
        values.push((b"overload", overload_bits));

        let meta_path_finder_bits = match importlib_required_attribute(
            _py,
            abc_bits,
            runtime_static_name_slot(_py, b"MetaPathFinder"),
            b"MetaPathFinder",
            "importlib.abc",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(meta_path_finder_bits);
        values.push((b"MetaPathFinder", meta_path_finder_bits));

        let suppress_bits = match importlib_required_attribute(
            _py,
            contextlib_bits,
            runtime_static_name_slot(_py, b"suppress"),
            b"suppress",
            "contextlib",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(suppress_bits);
        values.push((b"suppress", suppress_bits));

        let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
        for (key, value_bits) in values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_frozen_external_payload(
    machinery_bits: u64,
    _util_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut owned: Vec<u64> = Vec::new();
        let mut values: Vec<(&[u8], u64)> = Vec::with_capacity(16);

        let bytecode_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"BYTECODE_SUFFIXES"),
            b"BYTECODE_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(bytecode_suffixes_bits);
        values.push((b"BYTECODE_SUFFIXES", bytecode_suffixes_bits));

        let debug_bytecode_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"DEBUG_BYTECODE_SUFFIXES"),
            b"DEBUG_BYTECODE_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(debug_bytecode_suffixes_bits);
        values.push((b"DEBUG_BYTECODE_SUFFIXES", debug_bytecode_suffixes_bits));

        let extension_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"EXTENSION_SUFFIXES"),
            b"EXTENSION_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(extension_suffixes_bits);
        values.push((b"EXTENSION_SUFFIXES", extension_suffixes_bits));

        let magic_number_ptr = alloc_bytes(_py, b"\x00\x00\x00\x00");
        if magic_number_ptr.is_null() {
            for bits in owned {
                dec_ref_bits(_py, bits);
            }
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let magic_number_bits = MoltObject::from_ptr(magic_number_ptr).bits();
        owned.push(magic_number_bits);
        values.push((b"MAGIC_NUMBER", magic_number_bits));

        let optimized_bytecode_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"OPTIMIZED_BYTECODE_SUFFIXES"),
            b"OPTIMIZED_BYTECODE_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(optimized_bytecode_suffixes_bits);
        values.push((
            b"OPTIMIZED_BYTECODE_SUFFIXES",
            optimized_bytecode_suffixes_bits,
        ));

        let source_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"SOURCE_SUFFIXES"),
            b"SOURCE_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(source_suffixes_bits);
        values.push((b"SOURCE_SUFFIXES", source_suffixes_bits));

        let extension_file_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"ExtensionFileLoader"),
            b"ExtensionFileLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(extension_file_loader_bits);
        values.push((b"ExtensionFileLoader", extension_file_loader_bits));

        let file_finder_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"FileFinder"),
            b"FileFinder",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(file_finder_bits);
        values.push((b"FileFinder", file_finder_bits));

        let file_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"_FileLoader"),
            b"_FileLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(file_loader_bits);
        values.push((b"FileLoader", file_loader_bits));

        let namespace_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"NamespaceLoader"),
            b"NamespaceLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(namespace_loader_bits);
        values.push((b"NamespaceLoader", namespace_loader_bits));

        let path_finder_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"PathFinder"),
            b"PathFinder",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(path_finder_bits);
        values.push((b"PathFinder", path_finder_bits));

        let source_file_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"SourceFileLoader"),
            b"SourceFileLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(source_file_loader_bits);
        values.push((b"SourceFileLoader", source_file_loader_bits));

        let source_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"_SourceLoader"),
            b"_SourceLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(source_loader_bits);
        values.push((b"SourceLoader", source_loader_bits));

        let sourceless_file_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"SourcelessFileLoader"),
            b"SourcelessFileLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(sourceless_file_loader_bits);
        values.push((b"SourcelessFileLoader", sourceless_file_loader_bits));

        let loader_basics_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"_LoaderBasics"),
            b"_LoaderBasics",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(loader_basics_bits);
        values.push((b"_LoaderBasics", loader_basics_bits));

        let windows_registry_finder_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"WindowsRegistryFinder"),
            b"WindowsRegistryFinder",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(windows_registry_finder_bits);
        values.push((b"WindowsRegistryFinder", windows_registry_finder_bits));

        let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
        for (key, value_bits) in values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}
