use super::common::{builtin_func_bits, builtin_func_bits_with_defaults_tuple};
use crate::PyToken;
use crate::*;

pub(crate) fn file_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "read" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.file_read,
                fn_addr!(molt_file_read),
                2,
                &[neg_one],
            ))
        }
        "readline" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.file_readline,
                fn_addr!(molt_file_readline),
                2,
                &[neg_one],
            ))
        }
        "readlines" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.file_readlines,
                fn_addr!(molt_file_readlines),
                2,
                &[neg_one],
            ))
        }
        "read1" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.file_read1,
                fn_addr!(molt_file_read1),
                2,
                &[neg_one],
            ))
        }
        "readall" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_readall,
            fn_addr!(molt_file_readall),
            1,
        )),
        "readinto" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_readinto,
            fn_addr!(molt_file_readinto),
            2,
        )),
        "readinto1" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_readinto1,
            fn_addr!(molt_file_readinto1),
            2,
        )),
        "write" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_write,
            fn_addr!(molt_file_write),
            2,
        )),
        "writelines" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_writelines,
            fn_addr!(molt_file_writelines),
            2,
        )),
        "flush" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_flush,
            fn_addr!(molt_file_flush),
            1,
        )),
        "close" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_close,
            fn_addr!(molt_file_close),
            1,
        )),
        "detach" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_detach,
            fn_addr!(molt_file_detach),
            1,
        )),
        "reconfigure" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_reconfigure,
            fn_addr!(molt_file_reconfigure),
            6,
        )),
        "seek" => {
            let zero = MoltObject::from_int(0).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.file_seek,
                fn_addr!(molt_file_seek),
                3,
                &[zero],
            ))
        }
        "tell" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_tell,
            fn_addr!(molt_file_tell),
            1,
        )),
        "fileno" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_fileno,
            fn_addr!(molt_file_fileno),
            1,
        )),
        "truncate" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.file_truncate,
                fn_addr!(molt_file_truncate),
                2,
                &[none],
            ))
        }
        "readable" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_readable,
            fn_addr!(molt_file_readable),
            1,
        )),
        "writable" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_writable,
            fn_addr!(molt_file_writable),
            1,
        )),
        "seekable" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_seekable,
            fn_addr!(molt_file_seekable),
            1,
        )),
        "isatty" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_isatty,
            fn_addr!(molt_file_isatty),
            1,
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_iter,
            fn_addr!(molt_file_iter),
            1,
        )),
        "__next__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_next,
            fn_addr!(molt_file_next),
            1,
        )),
        "__enter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_enter,
            fn_addr!(molt_file_enter),
            1,
        )),
        "__exit__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_exit,
            fn_addr!(molt_file_exit_method),
            4,
        )),
        "peek" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.file_peek,
                fn_addr!(molt_file_peek),
                2,
                &[neg_one],
            ))
        }
        "getvalue" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_getvalue,
            fn_addr!(molt_file_getvalue),
            1,
        )),
        "getbuffer" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_getbuffer,
            fn_addr!(molt_file_getbuffer),
            1,
        )),
        _ => None,
    }
}
