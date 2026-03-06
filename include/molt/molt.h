#ifndef MOLT_C_API_MOLT_H
#define MOLT_C_API_MOLT_H

/* Stable libmolt extension ABI: versioned by MOLT_C_API_VERSION. */

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define MOLT_C_API_VERSION 1u

typedef uint64_t MoltHandle;

typedef struct MoltBufferView {
  uint8_t *data;
  uint64_t len;
  uint32_t readonly;
  uint32_t reserved;
  int64_t stride;
  uint64_t itemsize;
  MoltHandle owner;
} MoltBufferView;

uint32_t molt_c_api_version(void);

int32_t molt_init(void);
int32_t molt_shutdown(void);
int32_t molt_gil_acquire(void);
int32_t molt_gil_release(void);
int32_t molt_gil_is_held(void);

void molt_handle_incref(MoltHandle handle);
void molt_handle_decref(MoltHandle handle);
MoltHandle molt_none(void);
MoltHandle molt_bool_from_i32(int32_t value);
MoltHandle molt_int_from_i64(int64_t value);
int64_t molt_int_as_i64(MoltHandle value_bits);
MoltHandle molt_float_from_f64(double value);
double molt_float_as_f64(MoltHandle value_bits);

int32_t molt_err_set(MoltHandle exc_type_bits, const uint8_t *message_ptr,
                     uint64_t message_len);
int32_t molt_err_format(MoltHandle exc_type_bits, const uint8_t *message_ptr,
                        uint64_t message_len);
int32_t molt_err_clear(void);
int32_t molt_err_pending(void);
MoltHandle molt_err_peek(void);
MoltHandle molt_err_fetch(void);
int32_t molt_err_restore(MoltHandle exc_bits);
int32_t molt_err_matches(MoltHandle exc_type_bits);
MoltHandle molt_builtin_class_lookup(MoltHandle name_bits);

MoltHandle molt_object_getattr(MoltHandle obj_bits, MoltHandle name_bits);
MoltHandle molt_object_getattr_bytes(MoltHandle obj_bits,
                                     const uint8_t *name_ptr, uint64_t name_len);
MoltHandle molt_object_setattr(MoltHandle obj_bits, MoltHandle name_bits,
                               MoltHandle val_bits);
int32_t molt_object_setattr_bytes(MoltHandle obj_bits, const uint8_t *name_ptr,
                                  uint64_t name_len, MoltHandle val_bits);
int32_t molt_object_hasattr(MoltHandle obj_bits, MoltHandle name_bits);
MoltHandle molt_object_call(MoltHandle callable_bits, MoltHandle args_bits,
                            MoltHandle kwargs_bits);
MoltHandle molt_object_repr(MoltHandle obj_bits);
MoltHandle molt_object_str(MoltHandle obj_bits);
MoltHandle molt_object_get_iter(MoltHandle obj_bits);
int32_t molt_iterator_next(MoltHandle iter_bits, MoltHandle *out_value);
int32_t molt_object_truthy(MoltHandle obj_bits);
int32_t molt_object_equal(MoltHandle lhs_bits, MoltHandle rhs_bits);
int32_t molt_object_not_equal(MoltHandle lhs_bits, MoltHandle rhs_bits);
int32_t molt_object_contains(MoltHandle container_bits, MoltHandle item_bits);
MoltHandle molt_capsule_new(uintptr_t pointer_bits, const uint8_t *name_ptr,
                            uint64_t name_len, uintptr_t destructor_bits);
const uint8_t *molt_capsule_get_name_ptr(MoltHandle capsule_bits,
                                         uint64_t *out_len);
uintptr_t molt_capsule_get_pointer(MoltHandle capsule_bits,
                                   const uint8_t *name_ptr, uint64_t name_len);
int32_t molt_capsule_is_valid(MoltHandle capsule_bits, const uint8_t *name_ptr,
                              uint64_t name_len);
uintptr_t molt_capsule_get_context(MoltHandle capsule_bits);
int32_t molt_capsule_set_context(MoltHandle capsule_bits, uintptr_t context_bits);
uintptr_t molt_capsule_import(const uint8_t *name_ptr, uint64_t name_len);
int32_t molt_type_ready(MoltHandle type_bits);

MoltHandle molt_module_create(MoltHandle name_bits);
MoltHandle molt_module_import(MoltHandle name_bits);
MoltHandle molt_module_get_dict(MoltHandle module_bits);
int32_t molt_module_capi_register(MoltHandle module_bits, uintptr_t module_def_ptr,
                                  uint64_t module_state_size);
uintptr_t molt_module_capi_get_def(MoltHandle module_bits);
uintptr_t molt_module_capi_get_state(MoltHandle module_bits);
int32_t molt_module_state_add(MoltHandle module_bits, uintptr_t module_def_ptr);
MoltHandle molt_module_state_find(uintptr_t module_def_ptr);
int32_t molt_module_state_remove(uintptr_t module_def_ptr);
int32_t molt_module_add_object(MoltHandle module_bits, MoltHandle name_bits,
                               MoltHandle value_bits);
int32_t molt_module_add_object_bytes(MoltHandle module_bits,
                                     const uint8_t *name_ptr, uint64_t name_len,
                                     MoltHandle value_bits);
MoltHandle molt_module_get_object(MoltHandle module_bits, MoltHandle name_bits);
MoltHandle molt_module_get_object_bytes(MoltHandle module_bits,
                                        const uint8_t *name_ptr,
                                        uint64_t name_len);
int32_t molt_module_add_type(MoltHandle module_bits, MoltHandle type_bits);
int32_t molt_module_add_int_constant(MoltHandle module_bits, MoltHandle name_bits,
                                     int64_t value);
int32_t molt_module_add_string_constant(MoltHandle module_bits,
                                        MoltHandle name_bits,
                                        const uint8_t *value_ptr,
                                        uint64_t value_len);
MoltHandle molt_cfunction_create_bytes(MoltHandle self_bits,
                                       const uint8_t *name_ptr,
                                       uint64_t name_len,
                                       uintptr_t method_ptr,
                                       uint32_t method_flags,
                                       const uint8_t *doc_ptr,
                                       uint64_t doc_len);
int32_t molt_module_add_cfunction_bytes(MoltHandle module_bits,
                                        const uint8_t *name_ptr,
                                        uint64_t name_len,
                                        uintptr_t method_ptr,
                                        uint32_t method_flags,
                                        const uint8_t *doc_ptr,
                                        uint64_t doc_len);

MoltHandle molt_number_add(MoltHandle a_bits, MoltHandle b_bits);
MoltHandle molt_number_sub(MoltHandle a_bits, MoltHandle b_bits);
MoltHandle molt_number_mul(MoltHandle a_bits, MoltHandle b_bits);
MoltHandle molt_number_truediv(MoltHandle a_bits, MoltHandle b_bits);
MoltHandle molt_number_floordiv(MoltHandle a_bits, MoltHandle b_bits);
MoltHandle molt_number_long(MoltHandle obj_bits);
MoltHandle molt_number_float(MoltHandle obj_bits);

int64_t molt_sequence_length(MoltHandle seq_bits);
MoltHandle molt_sequence_getitem(MoltHandle seq_bits, MoltHandle key_bits);
int32_t molt_sequence_setitem(MoltHandle seq_bits, MoltHandle key_bits,
                              MoltHandle val_bits);
MoltHandle molt_sequence_to_list(MoltHandle seq_bits);
MoltHandle molt_sequence_to_tuple(MoltHandle seq_bits);

MoltHandle molt_mapping_getitem(MoltHandle mapping_bits, MoltHandle key_bits);
int32_t molt_mapping_setitem(MoltHandle mapping_bits, MoltHandle key_bits,
                             MoltHandle val_bits);
int64_t molt_mapping_length(MoltHandle mapping_bits);
MoltHandle molt_mapping_keys(MoltHandle mapping_bits);
MoltHandle molt_tuple_from_array(const MoltHandle *items, uint64_t len);
MoltHandle molt_list_from_array(const MoltHandle *items, uint64_t len);
MoltHandle molt_dict_from_pairs(const MoltHandle *keys, const MoltHandle *values,
                                uint64_t len);

int32_t molt_buffer_acquire(MoltHandle obj_bits, MoltBufferView *out_view);
int32_t molt_buffer_release(MoltBufferView *view);

MoltHandle molt_bytes_from(const uint8_t *data, uint64_t len);
const uint8_t *molt_bytes_as_ptr(MoltHandle bytes_bits, uint64_t *out_len);
MoltHandle molt_string_from(const uint8_t *data, uint64_t len);
const uint8_t *molt_string_as_ptr(MoltHandle string_bits, uint64_t *out_len);

MoltHandle molt_bytearray_from(const uint8_t *data, uint64_t len);
uint8_t *molt_bytearray_as_ptr(MoltHandle bytearray_bits, uint64_t *out_len);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* MOLT_C_API_MOLT_H */
