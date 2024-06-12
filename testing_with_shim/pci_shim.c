#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <unistd.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/time.h>
#include <time.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <sys/sendfile.h>
#include <errno.h>
#include <math.h>

#include "pci_shim.h"

#define N 40

typedef uint64_t __m256i __attribute__((vector_size(16)));
typedef uint64_t v8u __attribute ((vector_size(64)));
typedef uint64_t v4u __attribute ((vector_size(32)));
typedef uint64_t v2u __attribute ((vector_size(16)));



#define PCIE_BASE_ADDRESS 0xf0000000

#if DEBUG
    #define debug(s, ...) fprintf(stderr, "xdma_shim: " s \
                                  __VA_OPT__(,) __VA_ARGS__)
#else
    #define debug(s, ...)
#endif

#define print_error(s, e, ...) \
    fprintf(stderr, "xdma_shim: %s: " s, strerror(e) \
            __VA_OPT__(,) __VA_ARGS__)

#define STR(s) #s


struct HardwareCommon *
initialize() {
    struct HardwareCommon *hc;

    hc= calloc(1, sizeof(struct HardwareCommon));
    if(!hc) {
        print_error("Failed to allocate hardware context\n", errno);
    }
    int fdpcie;
    void *pcie_mem_base;
    void *base_address = (void *) PCIE_BASE_ADDRESS;

    // Open the device file for PCIe memory access

    fdpcie = open("/sys/bus/pci/devices/0004:90:00.0/resource0", O_RDWR | O_SYNC, 0);
//    fdpcie = open("/sys/bus/pci/devices/0000:06:00.1/resource0_wc", O_RDWR | O_SYNC, 0);
    if (fdpcie == -1) {
        perror("Error opening /dev/mem");
        return NULL;
    }

    // Map the PCIe memory region to user space
    pcie_mem_base = mmap(base_address, 65536 /*MEMORY_SIZE_BYTES */ , PROT_READ | PROT_WRITE,
                         MAP_SHARED, fdpcie, 0);
    if (pcie_mem_base == MAP_FAILED) {
        perror("Error mapping PCIe memory");
        close(fdpcie);
        return NULL;
    }

    hc->mem = (void*) pcie_mem_base;
    hc->fd = fdpcie;


    return hc;
}

void
closeHardware(struct HardwareCommon *hc) {

    if(close(hc->fd) < 0) {
        perror("Can't close the device");
    }

    free(hc->mem);
    free(hc);

    debug("Deallocated context for %zuB writes on H2C channel at %s " \
          "and C2H channel at %s\n",
          (size_t)XDMA_SHIM_PACKETSIZE,
          xdma_shim_h2c_devname,
          xdma_shim_c2h_devname);
}

void
write_data(struct HardwareCommon *hc) {

    volatile uint64_t *space = (uint64_t *)hc->mem;
    uint64_t* output = (uint64_t *)hc->omem;

    v2u r2[20];

    for (int i = 0; i < N - 1; i += 2) {

        r2[i/2][0] = output[i];
        r2[i/2][1] = output[i+1];
//      *(volatile v2u *)(space + i) = r3;
    }


    //int j = 0;
//    for (int i = 0; i < N - 1; i += 2) {
//        r2[i/2][0] = output[i];
//        r2[i/2][1] = output[i+1];
//      *(volatile v2u *)(space + i) = r2[i/2];
      //j++;
//    }
    *(volatile v2u *)(space + 0) = r2[0];
    *(volatile v2u *)(space + 2) = r2[1];
    *(volatile v2u *)(space + 4) = r2[2];
    *(volatile v2u *)(space + 6) = r2[3];
    *(volatile v2u *)(space + 8) = r2[4];

    *(volatile v2u *)(space + 10) = r2[5];
    *(volatile v2u *)(space + 12) = r2[6];
    *(volatile v2u *)(space + 14) = r2[7];
    *(volatile v2u *)(space + 16) = r2[8];

    *(volatile v2u *)(space + 18) = r2[9];
    *(volatile v2u *)(space + 20) = r2[10];
    *(volatile v2u *)(space + 22) = r2[11];
    *(volatile v2u *)(space + 24) = r2[12];
    *(volatile v2u *)(space + 26) = r2[13];
    *(volatile v2u *)(space + 28) = r2[14];
    *(volatile v2u *)(space + 30) = r2[15];
    *(volatile v2u *)(space + 32) = r2[16];
    *(volatile v2u *)(space + 34) = r2[17];
    *(volatile v2u *)(space + 36) = r2[18];
    *(volatile v2u *)(space + 38) = r2[19];

    debug("Success\n");
}

void
read_data(struct HardwareCommon *hc) {

    v2u r1;

    volatile uint64_t *space = (uint64_t *)hc->mem;
    uint64_t offset = hc->offset;

    r1 = *(volatile v2u *)(space + offset);
    __sync_synchronize();

    hc->val1_o = r1[0];
    hc->val2_o = r1[1];


    debug("Success\n");
}

