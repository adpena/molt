// Opcode stdlib implementation.
// Extracted from functions.rs for tree shaking.

use crate::*;
use molt_obj_model::MoltObject;
use super::functions::*;


#[unsafe(no_mangle)]

pub(crate) fn opcode_num_popped_312(opcode: i64, oparg: i64) -> Option<i64> {
    match opcode {
        0 => Some(0),                 // CACHE
        1 => Some(1),                 // POP_TOP
        2 => Some(0),                 // PUSH_NULL
        3 => Some(1),                 // INTERPRETER_EXIT
        4 => Some(1 + 1),             // END_FOR
        5 => Some(2),                 // END_SEND
        9 => Some(0),                 // NOP
        11 => Some(1),                // UNARY_NEGATIVE
        12 => Some(1),                // UNARY_NOT
        15 => Some(1),                // UNARY_INVERT
        17 => Some(0),                // RESERVED
        25 => Some(2),                // BINARY_SUBSCR
        26 => Some(3),                // BINARY_SLICE
        27 => Some(4),                // STORE_SLICE
        30 => Some(1),                // GET_LEN
        31 => Some(1),                // MATCH_MAPPING
        32 => Some(1),                // MATCH_SEQUENCE
        33 => Some(2),                // MATCH_KEYS
        35 => Some(1),                // PUSH_EXC_INFO
        36 => Some(2),                // CHECK_EXC_MATCH
        37 => Some(2),                // CHECK_EG_MATCH
        49 => Some(4),                // WITH_EXCEPT_START
        50 => Some(1),                // GET_AITER
        51 => Some(1),                // GET_ANEXT
        52 => Some(1),                // BEFORE_ASYNC_WITH
        53 => Some(1),                // BEFORE_WITH
        54 => Some(2),                // END_ASYNC_FOR
        55 => Some(3),                // CLEANUP_THROW
        60 => Some(3),                // STORE_SUBSCR
        61 => Some(2),                // DELETE_SUBSCR
        68 => Some(1),                // GET_ITER
        69 => Some(1),                // GET_YIELD_FROM_ITER
        71 => Some(0),                // LOAD_BUILD_CLASS
        74 => Some(0),                // LOAD_ASSERTION_ERROR
        75 => Some(0),                // RETURN_GENERATOR
        83 => Some(1),                // RETURN_VALUE
        85 => Some(0),                // SETUP_ANNOTATIONS
        87 => Some(0),                // LOAD_LOCALS
        89 => Some(1),                // POP_EXCEPT
        90 => Some(1),                // STORE_NAME
        91 => Some(0),                // DELETE_NAME
        92 => Some(1),                // UNPACK_SEQUENCE
        93 => Some(1),                // FOR_ITER
        94 => Some(1),                // UNPACK_EX
        95 => Some(2),                // STORE_ATTR
        96 => Some(1),                // DELETE_ATTR
        97 => Some(1),                // STORE_GLOBAL
        98 => Some(0),                // DELETE_GLOBAL
        99 => Some((oparg - 2) + 2),  // SWAP
        100 => Some(0),               // LOAD_CONST
        101 => Some(0),               // LOAD_NAME
        102 => Some(oparg),           // BUILD_TUPLE
        103 => Some(oparg),           // BUILD_LIST
        104 => Some(oparg),           // BUILD_SET
        105 => Some(oparg * 2),       // BUILD_MAP
        106 => Some(1),               // LOAD_ATTR
        107 => Some(2),               // COMPARE_OP
        108 => Some(2),               // IMPORT_NAME
        109 => Some(1),               // IMPORT_FROM
        110 => Some(0),               // JUMP_FORWARD
        114 => Some(1),               // POP_JUMP_IF_FALSE
        115 => Some(1),               // POP_JUMP_IF_TRUE
        116 => Some(0),               // LOAD_GLOBAL
        117 => Some(2),               // IS_OP
        118 => Some(2),               // CONTAINS_OP
        119 => Some(oparg + 1),       // RERAISE
        120 => Some((oparg - 1) + 1), // COPY
        121 => Some(0),               // RETURN_CONST
        122 => Some(2),               // BINARY_OP
        123 => Some(2),               // SEND
        124 => Some(0),               // LOAD_FAST
        125 => Some(1),               // STORE_FAST
        126 => Some(0),               // DELETE_FAST
        127 => Some(0),               // LOAD_FAST_CHECK
        128 => Some(1),               // POP_JUMP_IF_NOT_NONE
        129 => Some(1),               // POP_JUMP_IF_NONE
        130 => Some(oparg),           // RAISE_VARARGS
        131 => Some(1),               // GET_AWAITABLE
        132 => Some(
            (if (oparg & 0x01) != 0 { 1 } else { 0 })
                + (if (oparg & 0x02) != 0 { 1 } else { 0 })
                + (if (oparg & 0x04) != 0 { 1 } else { 0 })
                + (if (oparg & 0x08) != 0 { 1 } else { 0 })
                + 1,
        ), // MAKE_FUNCTION
        133 => Some((if oparg == 3 { 1 } else { 0 }) + 2), // BUILD_SLICE
        134 => Some(0),               // JUMP_BACKWARD_NO_INTERRUPT
        135 => Some(0),               // MAKE_CELL
        136 => Some(0),               // LOAD_CLOSURE
        137 => Some(0),               // LOAD_DEREF
        138 => Some(1),               // STORE_DEREF
        139 => Some(0),               // DELETE_DEREF
        140 => Some(0),               // JUMP_BACKWARD
        141 => Some(3),               // LOAD_SUPER_ATTR
        142 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 3), // CALL_FUNCTION_EX
        143 => Some(0),               // LOAD_FAST_AND_CLEAR
        144 => Some(0),               // EXTENDED_ARG
        145 => Some((oparg - 1) + 2), // LIST_APPEND
        146 => Some((oparg - 1) + 2), // SET_ADD
        147 => Some(2),               // MAP_ADD
        149 => Some(0),               // COPY_FREE_VARS
        150 => Some(1),               // YIELD_VALUE
        151 => Some(0),               // RESUME
        152 => Some(3),               // MATCH_CLASS
        155 => Some((if (oparg & 0x04) == 0x04 { 1 } else { 0 }) + 1), // FORMAT_VALUE
        156 => Some(oparg + 1),       // BUILD_CONST_KEY_MAP
        157 => Some(oparg),           // BUILD_STRING
        162 => Some((oparg - 1) + 2), // LIST_EXTEND
        163 => Some((oparg - 1) + 2), // SET_UPDATE
        164 => Some(1),               // DICT_MERGE
        165 => Some(1),               // DICT_UPDATE
        171 => Some(oparg + 2),       // CALL
        172 => Some(0),               // KW_NAMES
        173 => Some(1),               // CALL_INTRINSIC_1
        174 => Some(2),               // CALL_INTRINSIC_2
        175 => Some(1),               // LOAD_FROM_DICT_OR_GLOBALS
        176 => Some(1),               // LOAD_FROM_DICT_OR_DEREF
        237 => Some(3),               // INSTRUMENTED_LOAD_SUPER_ATTR
        238 => Some(0),               // INSTRUMENTED_POP_JUMP_IF_NONE
        239 => Some(0),               // INSTRUMENTED_POP_JUMP_IF_NOT_NONE
        240 => Some(0),               // INSTRUMENTED_RESUME
        241 => Some(0),               // INSTRUMENTED_CALL
        242 => Some(1),               // INSTRUMENTED_RETURN_VALUE
        243 => Some(1),               // INSTRUMENTED_YIELD_VALUE
        244 => Some(0),               // INSTRUMENTED_CALL_FUNCTION_EX
        245 => Some(0),               // INSTRUMENTED_JUMP_FORWARD
        246 => Some(0),               // INSTRUMENTED_JUMP_BACKWARD
        247 => Some(0),               // INSTRUMENTED_RETURN_CONST
        248 => Some(0),               // INSTRUMENTED_FOR_ITER
        249 => Some(0),               // INSTRUMENTED_POP_JUMP_IF_FALSE
        250 => Some(0),               // INSTRUMENTED_POP_JUMP_IF_TRUE
        251 => Some(2),               // INSTRUMENTED_END_FOR
        252 => Some(2),               // INSTRUMENTED_END_SEND
        253 => Some(0),               // INSTRUMENTED_INSTRUCTION
        _ => None,
    }
}


pub(crate) fn opcode_num_pushed_312(opcode: i64, oparg: i64) -> Option<i64> {
    match opcode {
        0 => Some(0),                                            // CACHE
        1 => Some(0),                                            // POP_TOP
        2 => Some(1),                                            // PUSH_NULL
        3 => Some(0),                                            // INTERPRETER_EXIT
        4 => Some(0),                                            // END_FOR
        5 => Some(1),                                            // END_SEND
        9 => Some(0),                                            // NOP
        11 => Some(1),                                           // UNARY_NEGATIVE
        12 => Some(1),                                           // UNARY_NOT
        15 => Some(1),                                           // UNARY_INVERT
        17 => Some(0),                                           // RESERVED
        25 => Some(1),                                           // BINARY_SUBSCR
        26 => Some(1),                                           // BINARY_SLICE
        27 => Some(0),                                           // STORE_SLICE
        30 => Some(2),                                           // GET_LEN
        31 => Some(2),                                           // MATCH_MAPPING
        32 => Some(2),                                           // MATCH_SEQUENCE
        33 => Some(3),                                           // MATCH_KEYS
        35 => Some(2),                                           // PUSH_EXC_INFO
        36 => Some(2),                                           // CHECK_EXC_MATCH
        37 => Some(2),                                           // CHECK_EG_MATCH
        49 => Some(5),                                           // WITH_EXCEPT_START
        50 => Some(1),                                           // GET_AITER
        51 => Some(2),                                           // GET_ANEXT
        52 => Some(2),                                           // BEFORE_ASYNC_WITH
        53 => Some(2),                                           // BEFORE_WITH
        54 => Some(0),                                           // END_ASYNC_FOR
        55 => Some(2),                                           // CLEANUP_THROW
        60 => Some(0),                                           // STORE_SUBSCR
        61 => Some(0),                                           // DELETE_SUBSCR
        68 => Some(1),                                           // GET_ITER
        69 => Some(1),                                           // GET_YIELD_FROM_ITER
        71 => Some(1),                                           // LOAD_BUILD_CLASS
        74 => Some(1),                                           // LOAD_ASSERTION_ERROR
        75 => Some(0),                                           // RETURN_GENERATOR
        83 => Some(0),                                           // RETURN_VALUE
        85 => Some(0),                                           // SETUP_ANNOTATIONS
        87 => Some(1),                                           // LOAD_LOCALS
        89 => Some(0),                                           // POP_EXCEPT
        90 => Some(0),                                           // STORE_NAME
        91 => Some(0),                                           // DELETE_NAME
        92 => Some(oparg),                                       // UNPACK_SEQUENCE
        93 => Some(2),                                           // FOR_ITER
        94 => Some((oparg & 0xFF) + (oparg >> 8) + 1),           // UNPACK_EX
        95 => Some(0),                                           // STORE_ATTR
        96 => Some(0),                                           // DELETE_ATTR
        97 => Some(0),                                           // STORE_GLOBAL
        98 => Some(0),                                           // DELETE_GLOBAL
        99 => Some((oparg - 2) + 2),                             // SWAP
        100 => Some(1),                                          // LOAD_CONST
        101 => Some(1),                                          // LOAD_NAME
        102 => Some(1),                                          // BUILD_TUPLE
        103 => Some(1),                                          // BUILD_LIST
        104 => Some(1),                                          // BUILD_SET
        105 => Some(1),                                          // BUILD_MAP
        106 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 1), // LOAD_ATTR
        107 => Some(1),                                          // COMPARE_OP
        108 => Some(1),                                          // IMPORT_NAME
        109 => Some(2),                                          // IMPORT_FROM
        110 => Some(0),                                          // JUMP_FORWARD
        114 => Some(0),                                          // POP_JUMP_IF_FALSE
        115 => Some(0),                                          // POP_JUMP_IF_TRUE
        116 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 1), // LOAD_GLOBAL
        117 => Some(1),                                          // IS_OP
        118 => Some(1),                                          // CONTAINS_OP
        119 => Some(oparg),                                      // RERAISE
        120 => Some((oparg - 1) + 2),                            // COPY
        121 => Some(0),                                          // RETURN_CONST
        122 => Some(1),                                          // BINARY_OP
        123 => Some(2),                                          // SEND
        124 => Some(1),                                          // LOAD_FAST
        125 => Some(0),                                          // STORE_FAST
        126 => Some(0),                                          // DELETE_FAST
        127 => Some(1),                                          // LOAD_FAST_CHECK
        128 => Some(0),                                          // POP_JUMP_IF_NOT_NONE
        129 => Some(0),                                          // POP_JUMP_IF_NONE
        130 => Some(0),                                          // RAISE_VARARGS
        131 => Some(1),                                          // GET_AWAITABLE
        132 => Some(1),                                          // MAKE_FUNCTION
        133 => Some(1),                                          // BUILD_SLICE
        134 => Some(0),                                          // JUMP_BACKWARD_NO_INTERRUPT
        135 => Some(0),                                          // MAKE_CELL
        136 => Some(1),                                          // LOAD_CLOSURE
        137 => Some(1),                                          // LOAD_DEREF
        138 => Some(0),                                          // STORE_DEREF
        139 => Some(0),                                          // DELETE_DEREF
        140 => Some(0),                                          // JUMP_BACKWARD
        141 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 1), // LOAD_SUPER_ATTR
        142 => Some(1),                                          // CALL_FUNCTION_EX
        143 => Some(1),                                          // LOAD_FAST_AND_CLEAR
        144 => Some(0),                                          // EXTENDED_ARG
        145 => Some((oparg - 1) + 1),                            // LIST_APPEND
        146 => Some((oparg - 1) + 1),                            // SET_ADD
        147 => Some(0),                                          // MAP_ADD
        149 => Some(0),                                          // COPY_FREE_VARS
        150 => Some(1),                                          // YIELD_VALUE
        151 => Some(0),                                          // RESUME
        152 => Some(1),                                          // MATCH_CLASS
        155 => Some(1),                                          // FORMAT_VALUE
        156 => Some(1),                                          // BUILD_CONST_KEY_MAP
        157 => Some(1),                                          // BUILD_STRING
        162 => Some((oparg - 1) + 1),                            // LIST_EXTEND
        163 => Some((oparg - 1) + 1),                            // SET_UPDATE
        164 => Some(0),                                          // DICT_MERGE
        165 => Some(0),                                          // DICT_UPDATE
        171 => Some(1),                                          // CALL
        172 => Some(0),                                          // KW_NAMES
        173 => Some(1),                                          // CALL_INTRINSIC_1
        174 => Some(1),                                          // CALL_INTRINSIC_2
        175 => Some(1),                                          // LOAD_FROM_DICT_OR_GLOBALS
        176 => Some(1),                                          // LOAD_FROM_DICT_OR_DEREF
        237 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 1), // INSTRUMENTED_LOAD_SUPER_ATTR
        238 => Some(0),                                          // INSTRUMENTED_POP_JUMP_IF_NONE
        239 => Some(0), // INSTRUMENTED_POP_JUMP_IF_NOT_NONE
        240 => Some(0), // INSTRUMENTED_RESUME
        241 => Some(0), // INSTRUMENTED_CALL
        242 => Some(0), // INSTRUMENTED_RETURN_VALUE
        243 => Some(1), // INSTRUMENTED_YIELD_VALUE
        244 => Some(0), // INSTRUMENTED_CALL_FUNCTION_EX
        245 => Some(0), // INSTRUMENTED_JUMP_FORWARD
        246 => Some(0), // INSTRUMENTED_JUMP_BACKWARD
        247 => Some(0), // INSTRUMENTED_RETURN_CONST
        248 => Some(0), // INSTRUMENTED_FOR_ITER
        249 => Some(0), // INSTRUMENTED_POP_JUMP_IF_FALSE
        250 => Some(0), // INSTRUMENTED_POP_JUMP_IF_TRUE
        251 => Some(0), // INSTRUMENTED_END_FOR
        252 => Some(1), // INSTRUMENTED_END_SEND
        253 => Some(0), // INSTRUMENTED_INSTRUCTION
        _ => None,
    }
}


pub(crate) fn opcode_is_noarg_pseudo_312(opcode: i64) -> bool {
    matches!(opcode, 256..=259)
}


pub(crate) fn opcode_stack_effect_pseudo_312(opcode: i64) -> Option<i64> {
    match opcode {
        256 => Some(1),  // SETUP_FINALLY (max jump/non-jump)
        257 => Some(2),  // SETUP_CLEANUP (max jump/non-jump)
        258 => Some(1),  // SETUP_WITH (max jump/non-jump)
        259 => Some(0),  // POP_BLOCK
        260 => Some(0),  // JUMP
        261 => Some(0),  // JUMP_NO_INTERRUPT
        262 => Some(1),  // LOAD_METHOD
        263 => Some(-1), // LOAD_SUPER_METHOD
        264 => Some(-1), // LOAD_ZERO_SUPER_METHOD
        265 => Some(-1), // LOAD_ZERO_SUPER_ATTR
        266 => Some(-1), // STORE_FAST_MAYBE_NULL
        _ => None,
    }
}


#[inline]
pub(crate) fn opcode_is_noarg_312(opcode: i64) -> bool {
    opcode < 90 || opcode_is_noarg_pseudo_312(opcode)
}


#[inline]
pub(crate) fn opcode_stack_effect_core_312(opcode: i64, oparg: i64) -> Option<i64> {
    if let Some(effect) = opcode_stack_effect_pseudo_312(opcode) {
        return Some(effect);
    }
    let popped = opcode_num_popped_312(opcode, oparg)?;
    let pushed = opcode_num_pushed_312(opcode, oparg)?;
    if popped < 0 || pushed < 0 {
        return None;
    }
    pushed.checked_sub(popped)
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_opcode_payload_312_json() -> u64 {
    crate::with_gil_entry!(_py, {
        email_quopri_alloc_str(_py, OPCODE_PAYLOAD_312_JSON)
    })
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_token_payload_312_json() -> u64 {
    crate::with_gil_entry!(_py, { email_quopri_alloc_str(_py, TOKEN_PAYLOAD_312_JSON) })
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_token_payload_312() -> u64 {
    crate::with_gil_entry!(_py, {
        let parsed: JsonValue = match serde_json::from_str(TOKEN_PAYLOAD_312_JSON) {
            Ok(value) => value,
            Err(err) => {
                let msg = format!("invalid token payload json: {err}");
                return raise_exception::<u64>(_py, "RuntimeError", msg.as_str());
            }
        };
        match token_payload_json_value_to_bits(_py, &parsed) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_opcode_metadata_payload_314_json() -> u64 {
    crate::with_gil_entry!(_py, {
        email_quopri_alloc_str(_py, OPCODE_METADATA_PAYLOAD_314_JSON)
    })
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_opcode_get_specialization_stats() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_opcode_stack_effect(opcode_bits: u64, oparg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let opcode_obj = obj_from_bits(opcode_bits);
        let Some(opcode) = to_i64(opcode_obj) else {
            let msg = format!(
                "'{}' object cannot be interpreted as an integer",
                type_name(_py, opcode_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };

        let oparg_obj = obj_from_bits(oparg_bits);
        let opcode_noarg = opcode_is_noarg_312(opcode);
        if oparg_obj.is_none() {
            if opcode_noarg {
                return match opcode_stack_effect_core_312(opcode, 0) {
                    Some(effect) => MoltObject::from_int(effect).bits(),
                    None => raise_exception::<_>(_py, "ValueError", "invalid opcode or oparg"),
                };
            }
            return raise_exception::<_>(
                _py,
                "ValueError",
                "stack_effect: opcode requires oparg but oparg was not specified",
            );
        }
        if opcode_noarg {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "stack_effect: opcode does not permit oparg but oparg was specified",
            );
        }

        let Some(oparg) = to_i64(oparg_obj) else {
            let msg = format!(
                "'{}' object cannot be interpreted as an integer",
                type_name(_py, oparg_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };

        let Some(effect) = opcode_stack_effect_core_312(opcode, oparg) else {
            return raise_exception::<_>(_py, "ValueError", "invalid opcode or oparg");
        };
        MoltObject::from_int(effect).bits()
    })
}

