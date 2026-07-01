#ifndef MOLT_C_API_MOLT_H
#define MOLT_C_API_MOLT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define MOLT_C_API_VERSION 1u
#define MOLT_BUFFER_MAX_NDIM 64u
#define MOLT_BUFFER_FORMAT_CAP 16u

typedef uint64_t MoltHandle;

typedef struct MoltBufferView {
  uint8_t *data;
  uint64_t len;
  uint32_t readonly;
  uint32_t ndim;
  uint64_t itemsize;
  intptr_t offset;
  MoltHandle owner;
  MoltHandle base;
  intptr_t shape[MOLT_BUFFER_MAX_NDIM];
  intptr_t strides[MOLT_BUFFER_MAX_NDIM];
  char format[MOLT_BUFFER_FORMAT_CAP];
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
MoltHandle molt_object_delattr(MoltHandle obj_bits, MoltHandle name_bits);
MoltHandle molt_object_setattr(MoltHandle obj_bits, MoltHandle name_bits,
                               MoltHandle val_bits);
int32_t molt_object_setattr_bytes(MoltHandle obj_bits, const uint8_t *name_ptr,
                                  uint64_t name_len, MoltHandle val_bits);
int32_t molt_object_hasattr(MoltHandle obj_bits, MoltHandle name_bits);
MoltHandle molt_object_call(MoltHandle callable_bits, MoltHandle args_bits,
                            MoltHandle kwargs_bits);
MoltHandle molt_object_repr(MoltHandle obj_bits);
MoltHandle molt_object_str(MoltHandle obj_bits);
int32_t molt_object_truthy(MoltHandle obj_bits);
int32_t molt_object_equal(MoltHandle lhs_bits, MoltHandle rhs_bits);
int32_t molt_object_not_equal(MoltHandle lhs_bits, MoltHandle rhs_bits);
int32_t molt_object_contains(MoltHandle container_bits, MoltHandle item_bits);
int32_t molt_type_ready(MoltHandle type_bits);

MoltHandle molt_module_create(MoltHandle name_bits);
MoltHandle molt_module_import(MoltHandle name_bits);
MoltHandle molt_module_get_dict(MoltHandle module_bits);
int32_t molt_module_capi_register(MoltHandle module_bits, uintptr_t module_def_ptr,
                                  uint64_t module_state_size);
uintptr_t molt_module_capi_get_def(MoltHandle module_bits);
uint8_t *molt_module_capi_get_state(MoltHandle module_bits);
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
typedef MoltHandle (*MoltCFunction)(MoltHandle, MoltHandle);
typedef MoltHandle (*MoltCFunctionWithKeywords)(MoltHandle, MoltHandle, MoltHandle);
MoltHandle molt_cfunction_create_bytes(MoltHandle self_bits,
                                       const uint8_t *name_ptr,
                                       uint64_t name_len,
                                       MoltCFunction method_ptr,
                                       uint32_t method_flags,
                                       const uint8_t *doc_ptr,
                                       uint64_t doc_len);
MoltHandle molt_cfunction_create_keywords_bytes(MoltHandle self_bits,
                                                const uint8_t *name_ptr,
                                                uint64_t name_len,
                                                MoltCFunctionWithKeywords method_ptr,
                                                uint32_t method_flags,
                                                const uint8_t *doc_ptr,
                                                uint64_t doc_len);
MoltHandle molt_py_cfunction_create_bytes(MoltHandle self_bits,
                                          const uint8_t *name_ptr,
                                          uint64_t name_len,
                                          uintptr_t method_ptr,
                                          uint32_t method_flags,
                                          const uint8_t *doc_ptr,
                                          uint64_t doc_len);
int32_t molt_module_add_cfunction_bytes(MoltHandle module_bits,
                                        const uint8_t *name_ptr,
                                        uint64_t name_len,
                                        MoltCFunction method_ptr,
                                        uint32_t method_flags,
                                        const uint8_t *doc_ptr,
                                        uint64_t doc_len);
int32_t molt_module_add_cfunction_keywords_bytes(
    MoltHandle module_bits,
    const uint8_t *name_ptr,
    uint64_t name_len,
    MoltCFunctionWithKeywords method_ptr,
    uint32_t method_flags,
    const uint8_t *doc_ptr,
    uint64_t doc_len);
int32_t molt_module_add_py_cfunction_bytes(MoltHandle module_bits,
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
MoltHandle molt_iter_next(MoltHandle iter_bits);
MoltHandle molt_list_append(MoltHandle list_bits, MoltHandle val_bits);

MoltHandle molt_mapping_getitem(MoltHandle mapping_bits, MoltHandle key_bits);
int32_t molt_mapping_setitem(MoltHandle mapping_bits, MoltHandle key_bits,
                             MoltHandle val_bits);
int64_t molt_mapping_length(MoltHandle mapping_bits);
MoltHandle molt_mapping_keys(MoltHandle mapping_bits);
MoltHandle molt_dict_keys(MoltHandle dict_bits);
MoltHandle molt_dict_values(MoltHandle dict_bits);
MoltHandle molt_dict_items(MoltHandle dict_bits);
MoltHandle molt_dict_getitem_borrowed(MoltHandle dict_bits, MoltHandle key_bits);
MoltHandle molt_tuple_from_array(const MoltHandle *items, uint64_t len);
MoltHandle molt_list_from_array(const MoltHandle *items, uint64_t len);
MoltHandle molt_dict_from_pairs(const MoltHandle *keys, const MoltHandle *values,
                                uint64_t len);

int32_t molt_buffer_acquire(MoltHandle obj_bits, MoltBufferView *out_view);
int32_t molt_buffer_release(MoltBufferView *view);
MoltHandle molt_memoryview_new(MoltHandle obj_bits);
MoltHandle molt_memoryview_from_buffer(const MoltBufferView *view);
int32_t molt_memoryview_check(MoltHandle obj_bits);

MoltHandle molt_bytes_from(const uint8_t *data, uint64_t len);
const uint8_t *molt_bytes_as_ptr(MoltHandle bytes_bits, uint64_t *out_len);
MoltHandle molt_string_from(const uint8_t *data, uint64_t len);
const uint8_t *molt_string_as_ptr(MoltHandle string_bits, uint64_t *out_len);

MoltHandle molt_bytearray_from(const uint8_t *data, uint64_t len);
uint8_t *molt_bytearray_as_ptr(MoltHandle bytearray_bits, uint64_t *out_len);

#ifdef __cplusplus
} /* extern "C" */
#endif

#ifdef MOLT_EXTENSION_HOST_ABI
#include <stdlib.h>
#if defined(_WIN32)
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#else
#include <dlfcn.h>
#endif

static inline void *_molt_host_abi_symbol(const char *name) {
#if defined(_WIN32)
  HMODULE handle = GetModuleHandleA(NULL);
  void *symbol = handle ? (void *)GetProcAddress(handle, name) : NULL;
#else
  void *symbol = dlsym(RTLD_DEFAULT, name);
#endif
  if (symbol == NULL) {
    abort();
  }
  return symbol;
}

#define molt_c_api_version ((uint32_t (*)(void))_molt_host_abi_symbol("molt_c_api_version"))
#define molt_init ((int32_t (*)(void))_molt_host_abi_symbol("molt_init"))
#define molt_shutdown ((int32_t (*)(void))_molt_host_abi_symbol("molt_shutdown"))
#define molt_gil_acquire ((int32_t (*)(void))_molt_host_abi_symbol("molt_gil_acquire"))
#define molt_gil_release ((int32_t (*)(void))_molt_host_abi_symbol("molt_gil_release"))
#define molt_gil_is_held ((int32_t (*)(void))_molt_host_abi_symbol("molt_gil_is_held"))
#define molt_handle_incref ((void (*)(MoltHandle))_molt_host_abi_symbol("molt_handle_incref"))
#define molt_handle_decref ((void (*)(MoltHandle))_molt_host_abi_symbol("molt_handle_decref"))
#define molt_none ((MoltHandle (*)(void))_molt_host_abi_symbol("molt_none"))
#define molt_bool_from_i32 ((MoltHandle (*)(int32_t))_molt_host_abi_symbol("molt_bool_from_i32"))
#define molt_int_from_i64 ((MoltHandle (*)(int64_t))_molt_host_abi_symbol("molt_int_from_i64"))
#define molt_int_as_i64 ((int64_t (*)(MoltHandle))_molt_host_abi_symbol("molt_int_as_i64"))
#define molt_float_from_f64 ((MoltHandle (*)(double))_molt_host_abi_symbol("molt_float_from_f64"))
#define molt_float_as_f64 ((double (*)(MoltHandle))_molt_host_abi_symbol("molt_float_as_f64"))
#define molt_err_set ((int32_t (*)(MoltHandle, const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_err_set"))
#define molt_err_format ((int32_t (*)(MoltHandle, const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_err_format"))
#define molt_err_clear ((int32_t (*)(void))_molt_host_abi_symbol("molt_err_clear"))
#define molt_err_pending ((int32_t (*)(void))_molt_host_abi_symbol("molt_err_pending"))
#define molt_err_peek ((MoltHandle (*)(void))_molt_host_abi_symbol("molt_err_peek"))
#define molt_err_fetch ((MoltHandle (*)(void))_molt_host_abi_symbol("molt_err_fetch"))
#define molt_err_restore ((int32_t (*)(MoltHandle))_molt_host_abi_symbol("molt_err_restore"))
#define molt_err_matches ((int32_t (*)(MoltHandle))_molt_host_abi_symbol("molt_err_matches"))
#define molt_builtin_class_lookup ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_builtin_class_lookup"))
#define molt_object_getattr ((MoltHandle (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_object_getattr"))
#define molt_object_getattr_bytes ((MoltHandle (*)(MoltHandle, const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_object_getattr_bytes"))
#define molt_object_delattr ((MoltHandle (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_object_delattr"))
#define molt_object_setattr ((MoltHandle (*)(MoltHandle, MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_object_setattr"))
#define molt_object_setattr_bytes ((int32_t (*)(MoltHandle, const uint8_t *, uint64_t, MoltHandle))_molt_host_abi_symbol("molt_object_setattr_bytes"))
#define molt_object_hasattr ((int32_t (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_object_hasattr"))
#define molt_object_call ((MoltHandle (*)(MoltHandle, MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_object_call"))
#define molt_object_repr ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_object_repr"))
#define molt_object_str ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_object_str"))
#define molt_object_truthy ((int32_t (*)(MoltHandle))_molt_host_abi_symbol("molt_object_truthy"))
#define molt_object_equal ((int32_t (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_object_equal"))
#define molt_object_not_equal ((int32_t (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_object_not_equal"))
#define molt_object_contains ((int32_t (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_object_contains"))
#define molt_type_ready ((int32_t (*)(MoltHandle))_molt_host_abi_symbol("molt_type_ready"))
#define molt_module_create ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_module_create"))
#define molt_module_import ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_module_import"))
#define molt_module_get_dict ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_module_get_dict"))
#define molt_module_capi_register ((int32_t (*)(MoltHandle, uintptr_t, uint64_t))_molt_host_abi_symbol("molt_module_capi_register"))
#define molt_module_capi_get_def ((uintptr_t (*)(MoltHandle))_molt_host_abi_symbol("molt_module_capi_get_def"))
#define molt_module_capi_get_state ((uint8_t *(*)(MoltHandle))_molt_host_abi_symbol("molt_module_capi_get_state"))
#define molt_module_state_add ((int32_t (*)(MoltHandle, uintptr_t))_molt_host_abi_symbol("molt_module_state_add"))
#define molt_module_state_find ((MoltHandle (*)(uintptr_t))_molt_host_abi_symbol("molt_module_state_find"))
#define molt_module_state_remove ((int32_t (*)(uintptr_t))_molt_host_abi_symbol("molt_module_state_remove"))
#define molt_module_add_object ((int32_t (*)(MoltHandle, MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_module_add_object"))
#define molt_module_add_object_bytes ((int32_t (*)(MoltHandle, const uint8_t *, uint64_t, MoltHandle))_molt_host_abi_symbol("molt_module_add_object_bytes"))
#define molt_module_get_object ((MoltHandle (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_module_get_object"))
#define molt_module_get_object_bytes ((MoltHandle (*)(MoltHandle, const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_module_get_object_bytes"))
#define molt_module_add_type ((int32_t (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_module_add_type"))
#define molt_module_add_int_constant ((int32_t (*)(MoltHandle, MoltHandle, int64_t))_molt_host_abi_symbol("molt_module_add_int_constant"))
#define molt_module_add_string_constant ((int32_t (*)(MoltHandle, MoltHandle, const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_module_add_string_constant"))
#define molt_cfunction_create_bytes ((MoltHandle (*)(MoltHandle, const uint8_t *, uint64_t, MoltCFunction, uint32_t, const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_cfunction_create_bytes"))
#define molt_cfunction_create_keywords_bytes ((MoltHandle (*)(MoltHandle, const uint8_t *, uint64_t, MoltCFunctionWithKeywords, uint32_t, const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_cfunction_create_keywords_bytes"))
#define molt_py_cfunction_create_bytes ((MoltHandle (*)(MoltHandle, const uint8_t *, uint64_t, uintptr_t, uint32_t, const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_py_cfunction_create_bytes"))
#define molt_module_add_cfunction_bytes ((int32_t (*)(MoltHandle, const uint8_t *, uint64_t, MoltCFunction, uint32_t, const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_module_add_cfunction_bytes"))
#define molt_module_add_cfunction_keywords_bytes ((int32_t (*)(MoltHandle, const uint8_t *, uint64_t, MoltCFunctionWithKeywords, uint32_t, const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_module_add_cfunction_keywords_bytes"))
#define molt_module_add_py_cfunction_bytes ((int32_t (*)(MoltHandle, const uint8_t *, uint64_t, uintptr_t, uint32_t, const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_module_add_py_cfunction_bytes"))
#define molt_number_add ((MoltHandle (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_number_add"))
#define molt_number_sub ((MoltHandle (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_number_sub"))
#define molt_number_mul ((MoltHandle (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_number_mul"))
#define molt_number_truediv ((MoltHandle (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_number_truediv"))
#define molt_number_floordiv ((MoltHandle (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_number_floordiv"))
#define molt_number_long ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_number_long"))
#define molt_number_float ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_number_float"))
#define molt_sequence_length ((int64_t (*)(MoltHandle))_molt_host_abi_symbol("molt_sequence_length"))
#define molt_sequence_getitem ((MoltHandle (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_sequence_getitem"))
#define molt_sequence_setitem ((int32_t (*)(MoltHandle, MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_sequence_setitem"))
#define molt_iter_next ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_iter_next"))
#define molt_list_append ((MoltHandle (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_list_append"))
#define molt_mapping_getitem ((MoltHandle (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_mapping_getitem"))
#define molt_mapping_setitem ((int32_t (*)(MoltHandle, MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_mapping_setitem"))
#define molt_mapping_length ((int64_t (*)(MoltHandle))_molt_host_abi_symbol("molt_mapping_length"))
#define molt_mapping_keys ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_mapping_keys"))
#define molt_dict_keys ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_dict_keys"))
#define molt_dict_values ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_dict_values"))
#define molt_dict_items ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_dict_items"))
#define molt_dict_getitem_borrowed ((MoltHandle (*)(MoltHandle, MoltHandle))_molt_host_abi_symbol("molt_dict_getitem_borrowed"))
#define molt_tuple_from_array ((MoltHandle (*)(const MoltHandle *, uint64_t))_molt_host_abi_symbol("molt_tuple_from_array"))
#define molt_list_from_array ((MoltHandle (*)(const MoltHandle *, uint64_t))_molt_host_abi_symbol("molt_list_from_array"))
#define molt_dict_from_pairs ((MoltHandle (*)(const MoltHandle *, const MoltHandle *, uint64_t))_molt_host_abi_symbol("molt_dict_from_pairs"))
#define molt_buffer_acquire ((int32_t (*)(MoltHandle, MoltBufferView *))_molt_host_abi_symbol("molt_buffer_acquire"))
#define molt_buffer_release ((int32_t (*)(MoltBufferView *))_molt_host_abi_symbol("molt_buffer_release"))
#define molt_memoryview_new ((MoltHandle (*)(MoltHandle))_molt_host_abi_symbol("molt_memoryview_new"))
#define molt_memoryview_from_buffer ((MoltHandle (*)(const MoltBufferView *))_molt_host_abi_symbol("molt_memoryview_from_buffer"))
#define molt_memoryview_check ((int32_t (*)(MoltHandle))_molt_host_abi_symbol("molt_memoryview_check"))
#define molt_bytes_from ((MoltHandle (*)(const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_bytes_from"))
#define molt_bytes_as_ptr ((const uint8_t *(*)(MoltHandle, uint64_t *))_molt_host_abi_symbol("molt_bytes_as_ptr"))
#define molt_string_from ((MoltHandle (*)(const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_string_from"))
#define molt_string_as_ptr ((const uint8_t *(*)(MoltHandle, uint64_t *))_molt_host_abi_symbol("molt_string_as_ptr"))
#define molt_bytearray_from ((MoltHandle (*)(const uint8_t *, uint64_t))_molt_host_abi_symbol("molt_bytearray_from"))
#define molt_bytearray_as_ptr ((uint8_t *(*)(MoltHandle, uint64_t *))_molt_host_abi_symbol("molt_bytearray_as_ptr"))
#endif

#endif /* MOLT_C_API_MOLT_H */
