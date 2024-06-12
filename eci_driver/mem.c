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
    if (argc > 1) {
        offset = strtoll(argv[1], NULL, 16);
    }
    phys_addr = 0x10000000000ULL;
    printf("Mapping address: %016lx:%016lx\n", phys_addr, size);
    address = mmap((void *)phys_addr, size, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED, fd, 0); // mem
    printf("result: %s\n", strerror(errno));
    assert(address != MAP_FAILED);
    printf("Mapped address: %p\n", address);
    ptr = address;
    if (argc == 2) {
        ts1 = read_tsc();
        val = ptr[offset / 8];
        ts2 = read_tsc();
        printf("Read %016lx in %lu ns\n", val, (ts2 - ts1) * 10);
    } else if (argc == 3) {
        val = strtol(argv[2], NULL, 16);
        ts1 = read_tsc();
        for (unsigned k = 0; k < 1000; k++)
        ptr[offset / 8] = val;
        ts2 = read_tsc();
        printf("Written %016lx in %lu ns\n", val, (ts2 - ts1) * 10);
    }
    r = munmap(address, size);
    assert(!r);
    close(fd);
    return 0;
}
