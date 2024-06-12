#include <fcntl.h>
#include <unistd.h>
#include <sys/mman.h>
#include <stdio.h>
#include <assert.h>
#include <inttypes.h>
#include <stdlib.h>
#include <time.h>
#include <linux/mman.h>
#include <errno.h>
#include <string.h>

typedef uint64_t v2i __attribute__ ((vector_size (16)));
typedef float v4f __attribute__    ((vector_size (16)));
typedef uint64_t v4i __attribute__ ((vector_size (32)));
typedef uint64_t v8i __attribute__ ((vector_size (64)));

uint64_t read_tsc(void)
{
    uint64_t tsc;
    asm volatile("isb; mrs %0, cntvct_el0" : "=r" (tsc));
    return tsc;
}

int main(int argc, char *argv[])
{
    int fd = 0, r;
    void *address;
    size_t size = 16ULL * 1024 * 1024 * 1024;
    uint64_t phys_addr, offset, val;
    volatile uint64_t *ptr;
    uint64_t ts1, ts2;

    fd = open("/dev/fpgamem", O_RDWR);
    assert(fd >= 0);

    phys_addr = 0;
    offset = 0;
    if (argc > 2) {
        offset = strtol(argv[2], NULL, 16);
    }
    if (argc > 3)
        val = strtol(argv[3], NULL, 16);
    printf("Mapping address: %016lx:%016lx  %016lx\n", phys_addr, size, offset);
    address = mmap((void *)0x10000000000ULL, size, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED, fd, 0); // mem
    printf("result: %s\n", strerror(errno));
    assert(address != MAP_FAILED);
    printf("Mapped address: %p\n", address);
    ptr = address;
    if (argc == 1) {
        unsigned i, j;
        j = 0;
//        ts1 = read_tsc();
//        for (i = 0; i < 256; i++) {
//            __builtin_prefetch(ptr + (i << 4), 0, 3); // L2 prefetch
//        }
//        ts2 = read_tsc();
        ts1 = read_tsc();
//        for (i = 0; i < 256; i++) {
//            __builtin_prefetch(ptr + ((i + 1) << 4), 0, 3); // L2 prefetch
//            j += ptr[i << 4];
//        }
    v4f *a, *s;
    v4f tab[8] = {{0.0, 0.0, 0.0, 0.0}, {0.0, 0.0, 0.0, 0.0}, {0.0, 0.0, 0.0, 0.0}, {0.0, 0.0, 0.0, 0.0},
                  {0.0, 0.0, 0.0, 0.0}, {0.0, 0.0, 0.0, 0.0}, {0.0, 0.0, 0.0, 0.0}, {0.0, 0.0, 0.0, 0.0}};
    a = (void *)ptr;
            for (i = 0; i < 256; i += 8)
                __builtin_prefetch(s + i, 0, 2);
            i = 0;
            for (s = a; i < 256 ; i++) {
                __builtin_prefetch(s + 128, 0, 3); // L1 prefetch
                __builtin_prefetch(s + 256, 0, 2); // L2 prefetch
                tab[0] = *s++;
                tab[1] = *s++;
                tab[2] = *s++;
                tab[3] = *s++;
                tab[4] = *s++;
                tab[5] = *s++;
                tab[6] = *s++;
                tab[7] = *s++;
                asm("" : : : "memory");
            }
            ts2 = read_tsc();
            i = tab[0][0] + tab[1][0] + tab[2][0] + tab[3][0] + tab[4][0] + tab[5][0] + tab[6][0]+ tab[7][0];
            printf("Read block %d in %lu ns\n", i, (ts2 - ts1) * 10);
    } else if (argc == 4) {
        ts1 = read_tsc();
        for (unsigned k = 0; k < 1000; k++)
        ptr[offset / 8] = val;
        ts2 = read_tsc();
        printf("Written %016lx in %lu ns\n", val, (ts2 - ts1) * 10);
    } else if (argc == 2) {
        uint64_t v1, i;
        for (;;) {
            ts1 = read_tsc();
            i = 1;
            r = 0;
            for (i = 0; i < 512; i++) {
                printf("Reading %p\n", ptr + i * 0x200000);
                ptr[i * 0x200000] = 123;
            }
            ts2 = read_tsc();
            printf("Value: %016lx in %lu ns (%lu) %d\n", v1, (ts2 - ts1) * 10, (ts2 - ts1) * 10 / i, r);
            break;
            sleep(1);
        }
    }
    r = munmap(address, size);
    assert(!r);
    close(fd);
    return 0;
}
