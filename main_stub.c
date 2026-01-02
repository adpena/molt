
#include <stdio.h>
#include <stdlib.h>
extern void molt_main();
extern long molt_json_parse_int(const char* ptr, long len);
extern long molt_get_attr_generic(void* obj, const char* attr, long len);
extern void* molt_alloc(long size);
extern long molt_block_on(void* task);
extern long molt_async_sleep(void* obj);
extern void molt_spawn(void* task);
extern void* molt_chan_new();
extern long molt_chan_send(void* chan, long val);
extern long molt_chan_recv(void* chan);
void molt_print_int(long i) {
    printf("%ld\n", i);
}
int main() {
    molt_main();
    return 0;
}
