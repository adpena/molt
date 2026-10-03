use super::common::{builtin_func_bits, builtin_func_bits_with_defaults_tuple};
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::*;

crate::builtins::methods::native_method_table!(file_method_bits, publish_file_methods, _py, name, [owner], {
    let classes = builtin_classes(_py);
    let owners = [classes.file, classes.file_io, classes.buffered_reader,
        classes.buffered_writer, classes.buffered_random, classes.text_io_wrapper,
        classes.bytes_io, classes.string_io];
    let owner = if owners.contains(&owner) { owner } else {
        let pointer = obj_from_bits(owner).as_ptr()?;
        unsafe { class_mro_view(_py, pointer).iter().copied().find(|candidate| owners.contains(candidate))? }
    };

}, {
        "read" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "read"),
                fn_addr!(molt_file_read),
                2,
                &[neg_one],
            ))
        },
        "readline" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "readline"),
                fn_addr!(molt_file_readline),
                2,
                &[neg_one],
            ))
        },
        "readlines" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "readlines"),
                fn_addr!(molt_file_readlines),
                2,
                &[neg_one],
            ))
        },
        "read1" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "read1"),
                fn_addr!(molt_file_read1),
                2,
                &[neg_one],
            ))
        },
        "readall" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "readall"),
            fn_addr!(molt_file_readall),
            1,
        )),
        "readinto" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "readinto"),
            fn_addr!(molt_file_readinto),
            2,
        )),
        "readinto1" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "readinto1"),
            fn_addr!(molt_file_readinto1),
            2,
        )),
        "write" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "write"),
            fn_addr!(molt_file_write),
            2,
        )),
        "writelines" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "writelines"),
            fn_addr!(molt_file_writelines),
            2,
        )),
        "flush" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "flush"),
            fn_addr!(molt_file_flush),
            1,
        )),
        "close" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "close"),
            fn_addr!(molt_file_close),
            1,
        )),
        "detach" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "detach"),
            fn_addr!(molt_file_detach),
            1,
        )),
        "reconfigure" if owner == builtin_classes(_py).text_io_wrapper => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "reconfigure"),
            fn_addr!(molt_file_reconfigure),
            6,
        )),
        "seek" => {
            let zero = MoltObject::from_int(0).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "seek"),
                fn_addr!(molt_file_seek),
                3,
                &[zero],
            ))
        },
        "tell" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "tell"),
            fn_addr!(molt_file_tell),
            1,
        )),
        "fileno" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "fileno"),
            fn_addr!(molt_file_fileno),
            1,
        )),
        "truncate" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "truncate"),
                fn_addr!(molt_file_truncate),
                2,
                &[none],
            ))
        },
        "readable" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "readable"),
            fn_addr!(molt_file_readable),
            1,
        )),
        "writable" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "writable"),
            fn_addr!(molt_file_writable),
            1,
        )),
        "seekable" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "seekable"),
            fn_addr!(molt_file_seekable),
            1,
        )),
        "isatty" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "isatty"),
            fn_addr!(molt_file_isatty),
            1,
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, owner, "__iter__"),
            fn_addr!(molt_file_iter),
            1,
        )),
        "__next__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, owner, "__next__"),
            fn_addr!(molt_file_next),
            1,
        )),
        "__enter__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "__enter__"),
            fn_addr!(molt_file_enter),
            1,
        )),
        "__exit__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "__exit__"),
            fn_addr!(molt_file_exit_method),
            4,
        )),
        "peek" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "peek"),
                fn_addr!(molt_file_peek),
                2,
                &[neg_one],
            ))
        },
        "getvalue" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "getvalue"),
            fn_addr!(molt_file_getvalue),
            1,
        )),
        "getbuffer" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, owner, "getbuffer"),
            fn_addr!(molt_file_getbuffer),
            1,
        )),
});
