use super::super::event_commands::{
    TK_EVENT_SUBST_FIELD_COUNT, flatten_event_subst_arg, normalize_event_subst_bool_field,
    normalize_event_subst_delta_field, normalize_event_subst_int_field,
};
use super::super::parsing::alloc_tuple_bits;
use crate::bridge::decode_value_list;
use molt_runtime_core::prelude::{MoltObject, obj_from_bits};

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_event_subst_parse(_widget_path_bits: u64, event_args_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(raw_args) = decode_value_list(obj_from_bits(event_args_bits)) else {
            return MoltObject::none().bits();
        };
        let args: Vec<u64> = raw_args.into_iter().map(flatten_event_subst_arg).collect();
        if args.len() != TK_EVENT_SUBST_FIELD_COUNT {
            return MoltObject::none().bits();
        }

        let payload = [
            normalize_event_subst_int_field(args[0]),
            normalize_event_subst_int_field(args[1]),
            normalize_event_subst_bool_field(args[2]),
            normalize_event_subst_int_field(args[3]),
            normalize_event_subst_int_field(args[4]),
            normalize_event_subst_int_field(args[5]),
            normalize_event_subst_int_field(args[6]),
            normalize_event_subst_int_field(args[7]),
            normalize_event_subst_int_field(args[8]),
            normalize_event_subst_int_field(args[9]),
            args[10],
            normalize_event_subst_bool_field(args[11]),
            args[12],
            normalize_event_subst_int_field(args[13]),
            args[14],
            args[15],
            normalize_event_subst_int_field(args[16]),
            normalize_event_subst_int_field(args[17]),
            normalize_event_subst_delta_field(args[18]),
        ];

        match alloc_tuple_bits(
            _py,
            &payload,
            "failed to allocate tkinter event substitution tuple",
        ) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
