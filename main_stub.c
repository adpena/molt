
#include <stdio.h>
#include <stdlib.h>
extern void molt_main();
extern int molt_json_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_msgpack_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_cbor_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern long molt_get_attr_generic(void* obj, const char* attr, long len);
extern void* molt_alloc(long size);
extern long molt_block_on(void* task);
extern long molt_async_sleep(void* obj);
extern void molt_spawn(void* task);
extern void* molt_chan_new();
extern long molt_chan_send(void* chan, long val);
extern long molt_chan_recv(void* chan);
extern void molt_print_obj(unsigned long long val);
int main() {
    molt_main();
    return 0;
}
