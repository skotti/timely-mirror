#include <stdio.h>
#include <stdlib.h>
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
//#include <immintrin.h>
//#include <emmintrin.h>

#define N 40
#define M 96

#define PCIE_BASE_ADDRESS 0xf0000000    // Replace with the PCIe base address of your device
#define MEMORY_SIZE_BYTES 2097152       // Replace with the size of the memory region you want to access

typedef uint64_t __m256i __attribute__((vector_size(16)));
typedef uint64_t v8u __attribute ((vector_size(64)));
typedef uint64_t v4u __attribute ((vector_size(32)));
typedef uint64_t v2u __attribute ((vector_size(16)));


inline uint64_t now(void);

uint64_t now(void)
{
    uint64_t t;
#ifdef __aarch64__
    __asm__ __volatile__(" isb\nmrs %0, cntvct_el0" : "=r" (t));
    t *= 10; // in nanoseconds
#endif
#ifdef __amd64__
    uint32_t a;
    uint32_t d;
    __asm__ __volatile__(" lfence\nrdtsc\n" : "=a" (a), "=d" (d));

    t = ((uint64_t) a) | (((uint64_t) d) << 32);
//    t = (t * 17274) >> 16; // AMD 5700G
//    t = (t * 14562) >> 16; // AMD 7700X
    t = (t * 18767) >> 16; // Intel i7-3770K
#endif
    return t;
}

int main(int argc, char *argv[])
{
    int fdpcie;
    void *pcie_mem_base;
    void *base_address = (void *) PCIE_BASE_ADDRESS;

    // Open the device file for PCIe memory access

    fdpcie = open("/sys/bus/pci/devices/0004:90:00.0/resource0", O_RDWR | O_SYNC, 0);
//    fdpcie = open("/sys/bus/pci/devices/0000:06:00.1/resource0_wc", O_RDWR | O_SYNC, 0);
    if (fdpcie == -1) {
        perror("Error opening /dev/mem");
        return 1;
    }

    // Map the PCIe memory region to user space
    pcie_mem_base = mmap(base_address, 65536 /*MEMORY_SIZE_BYTES */ , PROT_READ | PROT_WRITE,
                         MAP_SHARED, fdpcie, 0);
    if (pcie_mem_base == MAP_FAILED) {
        perror("Error mapping PCIe memory");
        close(fdpcie);
        return 1;
    }

    uint64_t output[N];
    uint64_t input[M];

    for (int i = 0; i < 10; i++) {

    output[0] = 3; // L
    output[1] = 5; // t
    output[2] = 7; // f1
    output[3] = 9; // f2
    output[4] = 11; // f3
    output[5] = 13; // f4
    output[6] = 15; // f5
    output[7] = 17; // f6
    output[8] = 19; // L
    output[9] = 21; // t
    output[10] = 23; // f1
    output[11] = 25; // f2
    output[12] = 27; // f3
    output[13] = 29; // f4
    output[14] = 31; // f5
    output[15] = 33; // f6
    output[16] = 35; // L
    output[17] = 37; // t
    output[18] = 39; // f1
    output[19] = 41; // f2
    output[20] = 43; // f3
    output[21] = 45; // f4
    output[22] = 47; // f5
    output[23] = 49; // f6

    output[24] = 21; // L
    output[25] = 25; // t
    output[26] = 27; // f1
    output[27] = 29; // f2
    output[28] = 31; // f3
    output[29] = 33; // f4
    output[30] = 35; // f5
    output[31] = 37; // f6

    output[32] = 39; // L
    output[33] = 41; // t
    output[34] = 43; // f1
    output[35] = 45; // f2
    output[36] = 47; // f3
    output[37] = 49; // f4
    output[38] = 51; // f5
    output[39] = 53; // f6


    int i, r = 0;

    v2u r2[20];
    v2u r3;

    volatile uint64_t *space = pcie_mem_base;

    for (int i = 0; i < N - 1; i += 2) {

	r2[i/2][0] = output[i];
	r2[i/2][1] = output[i+1];
//	*(volatile v2u *)(space + i) = r3;
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

    /*r2[0] = output[0];
    r2[1] = output[1];
    r2[2] = output[2];
    r2[3] = output[3];
    r2[4] = output[4];
    r2[5] = output[5];
    r2[6] = output[6];
    r2[7] = output[7];
    r2[8] = output[8];
    r2[9] = output[9];
    r2[10] = output[10];
    r2[11] = output[11];
    r2[12] = output[12];
    r2[13] = output[13];

    r2[14] = output[14];
    r2[15] = output[15];
    r2[16] = output[16];
    r2[17] = output[17];
    r2[18] = output[18];
    r2[19] = output[];
    r2[20] = output[i];
    r2[21] = output[i+1];
    r2[22] = output[i];
    r2[23] = output[i+1];
    r2[24] = output[i];
    r2[25] = output[i+1];
    r2[26] = output[i];
    r2[27] = output[i+1];
    r2[28] = output[i];
    r2[29] = output[i+1];
    r2[30] = output[i];
    r2[31] = output[i+1];

    r2[32] = output[i];
    r2[33] = output[i+1];
    r2[34] = output[i];
    r2[35] = output[i+1];
    r2[36] = output[i];
    r2[37] = output[i+1];
    r2[38] = output[i];
    r2[39] = output[i+1];
*/

    __sync_synchronize();

    for (int i = 0; i < M - 1; i += 2) {

	r3 = *(volatile v2u *)(space + i);
	//input[i] = r3[0];
	//input[i+1] = r3[1];
	printf("%lu %lu  \n", r3[0], r3[1]);
	__sync_synchronize();

    }
    __sync_synchronize();


    }
    /*for (int i = 0; i < M; i++) {
	printf("%lu ", input[i]);
    }*/
    printf("\n");

    munmap(pcie_mem_base, 65536);

    return 0;
}
