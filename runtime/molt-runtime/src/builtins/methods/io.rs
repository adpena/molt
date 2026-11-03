use super::common::{
    builtin_func_bits, builtin_func_bits_with_defaults_tuple, builtin_variadic_func_bits,
};
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::*;

crate::builtins::methods::native_method_table!(file_method_bits, publish_file_methods, _py, name, [owner], {
    let classes = builtin_classes(_py);
    let owners = [classes.file, classes.file_io, classes.buffered_reader,
        classes.buffered_writer, classes.buffered_random, classes.text_io_wrapper,
        classes.bytes_io, classes.string_io];
    let base_only = matches!(name, crate::NameId::DunderEnter | crate::NameId::DunderExit | crate::NameId::DunderDel | crate::NameId::PrivateCheckClosed);
    if base_only && owner != classes.io_base {
        return Ok(None);
    }
    let raw_default = owner == classes.raw_io_base && matches!(name, crate::NameId::Read | crate::NameId::Readall | crate::NameId::Readinto);
    let owner = if raw_default || owners.contains(&owner)
        || (owner == classes.io_base && (base_only || matches!(name, crate::NameId::Close | crate::NameId::Flush)))
        || (name == crate::NameId::Detach && [classes.buffered_io_base, classes.text_io_base].contains(&owner))
    { owner } else {
        let Some(pointer) = obj_from_bits(owner).as_ptr() else {
            return Ok(None);
        };
        let Some(owner) = (unsafe {
            class_mro_view(_py, pointer).iter().copied().find(|candidate| owners.contains(candidate))
        }) else {
            return Ok(None);
        };
        owner
    };

}, {
        crate::NameId::Read => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Read),
                if owner == builtin_classes(_py).raw_io_base {
                    fn_addr!(crate::builtins::io::raw_io_base_read)
                } else { fn_addr!(molt_file_read) },
                2,
                &[neg_one],
            ))
        },
        crate::NameId::Readline => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Readline),
                fn_addr!(molt_file_readline),
                2,
                &[neg_one],
            ))
        },
        crate::NameId::Readlines => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Readlines),
                fn_addr!(molt_file_readlines),
                2,
                &[neg_one],
            ))
        },
        crate::NameId::Read1 => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Read1),
                fn_addr!(molt_file_read1),
                2,
                &[neg_one],
            ))
        },
        crate::NameId::Readall [if [builtin_classes(_py).raw_io_base, builtin_classes(_py).file_io].contains(&owner) ] => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Readall),
            if owner == builtin_classes(_py).raw_io_base {
                fn_addr!(crate::builtins::io::raw_io_base_readall)
            } else { fn_addr!(molt_file_readall) },
            1,
        )),
        crate::NameId::Readinto [if owner == builtin_classes(_py).raw_io_base ] => Some(builtin_variadic_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Readinto),
            fn_addr!(crate::builtins::io::raw_io_base_readinto),
        )),
        crate::NameId::Readinto => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Readinto),
            fn_addr!(molt_file_readinto),
            2,
        )),
        crate::NameId::Readinto1 => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Readinto1),
            fn_addr!(molt_file_readinto1),
            2,
        )),
        crate::NameId::Write => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Write),
            fn_addr!(molt_file_write),
            2,
        )),
        crate::NameId::Writelines => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Writelines),
            fn_addr!(molt_file_writelines),
            2,
        )),
        crate::NameId::Flush [if ![builtin_classes(_py).file_io, builtin_classes(_py).string_io].contains(&owner) ] => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Flush),
            if owner == builtin_classes(_py).io_base { fn_addr!(crate::builtins::io::io_base_flush) } else { fn_addr!(molt_file_flush) },
            1,
        )),
        crate::NameId::Close => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Close),
            if owner == builtin_classes(_py).io_base { fn_addr!(crate::builtins::io::io_base_close) } else { fn_addr!(molt_file_close) },
            1,
        )),
        crate::NameId::Detach [if owner != builtin_classes(_py).file && owner != builtin_classes(_py).file_io ] => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Detach),
            fn_addr!(molt_file_detach),
            1,
        )),
        crate::NameId::Reconfigure [if owner == builtin_classes(_py).text_io_wrapper ] => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Reconfigure),
            fn_addr!(molt_file_reconfigure),
            6,
        )),
        crate::NameId::Seek => {
            let zero = MoltObject::from_int(0).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Seek),
                fn_addr!(molt_file_seek),
                3,
                &[zero],
            ))
        },
        crate::NameId::Tell => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Tell),
            fn_addr!(molt_file_tell),
            1,
        )),
        crate::NameId::Fileno => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Fileno),
            fn_addr!(molt_file_fileno),
            1,
        )),
        crate::NameId::Truncate => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Truncate),
                fn_addr!(molt_file_truncate),
                2,
                &[none],
            ))
        },
        crate::NameId::Readable => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Readable),
            fn_addr!(molt_file_readable),
            1,
        )),
        crate::NameId::Writable => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Writable),
            fn_addr!(molt_file_writable),
            1,
        )),
        crate::NameId::Seekable => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Seekable),
            fn_addr!(molt_file_seekable),
            1,
        )),
        crate::NameId::Isatty => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Isatty),
            fn_addr!(molt_file_isatty),
            1,
        )),
        crate::NameId::DunderIter => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, owner, crate::NameId::DunderIter),
            fn_addr!(molt_file_iter),
            1,
        )),
        crate::NameId::DunderNext => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, owner, crate::NameId::DunderNext),
            fn_addr!(molt_file_next),
            1,
        )),
        crate::NameId::DunderEnter => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::DunderEnter),
            fn_addr!(molt_file_enter),
            1,
        )),
        crate::NameId::DunderExit => Some(builtin_variadic_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::DunderExit),
            fn_addr!(crate::builtins::io::io_base_exit),
        )),
        crate::NameId::DunderDel => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, owner, crate::NameId::DunderDel),
            fn_addr!(crate::builtins::io::io_base_finalize),
            1,
        )),
        crate::NameId::PrivateCheckClosed => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::PrivateCheckClosed),
            fn_addr!(crate::builtins::io::io_base_check_closed),
            1,
        )),
        crate::NameId::Peek => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Peek),
                fn_addr!(molt_file_peek),
                2,
                &[neg_one],
            ))
        },
        crate::NameId::Getvalue => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Getvalue),
            fn_addr!(molt_file_getvalue),
            1,
        )),
        crate::NameId::Getbuffer => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, crate::NameId::Getbuffer),
            fn_addr!(molt_file_getbuffer),
            1,
        )),
});
