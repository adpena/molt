use super::super::event_commands::{
    TK_EVENT_SUBST_FIELD_COUNT, flatten_event_subst_arg, normalize_event_subst_bool_field,
    normalize_event_subst_delta_field, normalize_event_subst_int_field,
};
use super::super::parsing::alloc_tuple_bits;
use crate::bridge::decode_value_list;
use molt_runtime_core::prelude::{MoltObject, obj_from_bits};

pub fn molt_tk_event_subst_parse_attached_body(
    _py: &molt_runtime_core::PyToken,
    _widget_path_bits: u64,
    event_args_bits: u64,
) -> u64 {
    molt_runtime_core::with_attached_entry!(_py, _py, {
        let Some(raw_args) = decode_value_list(_py, obj_from_bits(event_args_bits)) else {
            return MoltObject::none().bits();
        };
        if raw_args.len() != TK_EVENT_SUBST_FIELD_COUNT {
            return MoltObject::none().bits();
        }
        let mut snapshots = Vec::new();
        let args: Vec<u64> = raw_args
            .iter()
            .map(|&bits| flatten_event_subst_arg(_py, bits, &mut snapshots))
            .collect();

        let payload = [
            normalize_event_subst_int_field(_py, args[0]),
            normalize_event_subst_int_field(_py, args[1]),
            normalize_event_subst_bool_field(_py, args[2]),
            normalize_event_subst_int_field(_py, args[3]),
            normalize_event_subst_int_field(_py, args[4]),
            normalize_event_subst_int_field(_py, args[5]),
            normalize_event_subst_int_field(_py, args[6]),
            normalize_event_subst_int_field(_py, args[7]),
            normalize_event_subst_int_field(_py, args[8]),
            normalize_event_subst_int_field(_py, args[9]),
            args[10],
            normalize_event_subst_bool_field(_py, args[11]),
            args[12],
            normalize_event_subst_int_field(_py, args[13]),
            args[14],
            args[15],
            normalize_event_subst_int_field(_py, args[16]),
            normalize_event_subst_int_field(_py, args[17]),
            normalize_event_subst_delta_field(_py, args[18]),
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

include!("../../attached_exports_generated/tk/callback_intrinsics/event_subst.rs");
