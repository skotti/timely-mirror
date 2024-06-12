/*
 * Enzian Memory Benchmark
 *
 * ETH 2022
 * Adam Turowski
 * adam.turowski@ethz.ch
 *
 * Latency and bandwidth memory benchmark using 1G hugepages
 * Core-to-core latency benchmark
 * To allocate huge pages in the main memory, do:
 * echo 3 > /sys/devices/system/node/node0/hugepages/hugepages-1048576kB/nr_hugepages
 */

#define _GNU_SOURCE
#include <unistd.h>
#include <stdio.h>
#include <time.h>
#include <assert.h>
#include <stdlib.h>
#include <stdint.h>
#include <sys/mman.h>
#include <linux/mman.h>
#include <assert.h>
#include <pthread.h>
#include <sys/sysinfo.h>
#include <fcntl.h>
#include <string.h>
#include <linux/ioctl.h>

#include <sys/ioctl.h>

#define SIZE_EXP 26
#define SIZE (1UL << SIZE_EXP)
#define MASK ((SIZE / 8) - 1)
#define COUNT (SIZE)
#define STRIDE ((128) * 16)
#define CACHELINE_SIZE 128

//#define TEST_CASE_1 0
//#define TEST_CASE_2 0
//#define TEST_CASE_3 0
//#define TEST_CASE_4 0
//#define TEST_CASE_5 0
//#define TEST_CASE_6 0
//#define TEST_CASE_7 1
//#define TEST_CASE_8 0

unsigned first_cpu, last_cpu, use_cpu_memory, do_cache_to_cache, do_latency, do_seq_latency, do_throughput;

void *area = NULL;
double rate = 1.0;
uint64_t itn;
uint64_t no_cpus = 1;
uint64_t l2_cache_size;
uint64_t area_test_size = 0;


static __inline__ uint64_t rdtsc(void)
{
#ifdef __aarch64__
    uint64_t t;
    __asm__ __volatile__(" isb\nmrs %0, cntvct_el0" : "=r" (t));
    return t;
#endif
#ifdef __amd64__
    uint32_t a;
    uint32_t d;
    __asm__ __volatile__(" lfence\nrdtsc\n" : "=a" (a), "=d" (d));
    return ((uint64_t) a) | (((uint64_t) d) << 32);
#endif
}


uint64_t now(void)
{
    return rdtsc();
}



const char * nice_size(unsigned int size)
{
    static char text[64];

    if (size < 1024) {
        snprintf(text, sizeof(text), "%9d", size);
    } else if (size < 1048576) {
        snprintf(text, sizeof(text), "%9dk", size / 1024);
    } else if (size < 1048576 * 1024) {
        snprintf(text, sizeof(text), "%9dM", size / 1024 / 1024);
    } else {
        snprintf(text, sizeof(text), "%9dG", size / 1024 / 1024 / 1024);
    }
    return text;
}

static __inline__ void dmb(void)
{
#ifdef __aarch64__
	__asm__ __volatile__ ("dmb 0xF":::"memory");
#endif
#ifdef __amd64__
	__asm__ __volatile__ ("dmb 0xF":::"memory");
#endif

}

static __inline__ void dsb(void)
{
#ifdef __aarch64__
        __asm__ __volatile__ ("dsb 0xF":::"memory");
#endif
#ifdef __amd64__
        __asm__ __volatile__ ("dsb 0xF":::"memory");
#endif

}



/*inline uint64_t read_cache_line(uint64_t* area, int cache_line) {
    return ((uint64_t*)area)[cache_line];
}

inline void write_cache_line(uint64_t* area, int cache_line, uint64_t val) {
    ((uint64_t*)area)[cache_line] = val;
}

inline void write_individuals(uint64_t* area, int cache_line) {
    write_cache_line(area, cache_line, 10);
    dmb();
}

inline void write_individuals_multiple(uint64_t* area, int cache_line) {
    for (int i = 0; i < 1000; i++) {
	write_cache_line(area, cache_line, i);
    	dmb();
    }
}

inline void read_individuals(uint64_t* result, uint64_t* area, int cache_line) {
    *result = read_cache_line(area, cache_line);
    dmb();
}

inline void read_individuals_multiple(uint64_t* result, uint64_t* area, int cache_line) {
    for (int i = 0; i < 1000; i++) {
        result[i] = read_cache_line(area, cache_line);
        dmb();
    }

}	

void read_and_write() {
    
}

void write_two_cache_lines() {

}

void throughput_writes_test(uint64_t* area) {
    uint64_t start = rdtsc();
    write_individuals(area, 0);
    uint64_t end = rdtsc();
    printf("%lu, ", (end - start));/// 100000000.0);

}

void throughput_reads_test(uint64_t* area) {
    uint64_t result;
    uint64_t start = rdtsc();
    read_individuals(&result, area, 0);
    uint64_t end = rdtsc();
    printf("%lu, ", (end - start));/// 100000000.0);

}

void latency_writes_test(uint64_t* area) {
    uint64_t start = rdtsc();

    //write_individuals(area, 0);
    ((uint64_t*)area)[0] = 10;
    dmb();

    uint64_t end = rdtsc();
    printf("%lu, ", (end - start));
}

void latency_reads_test(uint64_t* area) {
    uint64_t result;
    uint64_t start = rdtsc();

    read_individuals(&result, area, 0);

    uint64_t end = rdtsc();
    printf("result = %lu\n", result);
    printf("%lu\n ", (end - start));
}*/

/*printf("A = %ld\n", ((uint64_t*)area)[0]);
    printf("B = %ld\n", ((uint64_t*)area)[1]);
    printf("C = %ld\n", ((uint64_t*)area)[2]);
    printf("D = %ld\n", ((uint64_t*)area)[3]);
    printf("e = %ld\n", ((uint64_t*)area)[4]);
    printf("A = %ld\n", ((uint64_t*)area)[5]);
    printf("B = %ld\n", ((uint64_t*)area)[6]);
    printf("C = %ld\n", ((uint64_t*)area)[7]);
    printf("D = %ld\n", ((uint64_t*)area)[8]);
    printf("e = %ld\n", ((uint64_t*)area)[9]);*/

/*((uint64_t*)area)[0] = 5;
    dmb();
    ((uint64_t*)area)[16] = 6;
    dmb();
    ((uint64_t*)area)[0] = 7;
    dmb();
    ((uint64_t*)area)[16] = 8;
    dmb();*/


/*int c[2000];
    for (int i = 0; i < 1000; i++) {
        c[i] = ((uint64_t*)area)[0];
        //printf("%ld\n", ((uint64_t*)area)[0]);
        dmb();
        c[i+1] = ((uint64_t*)area)[16];
        //printf("%ld\n", ((uint64_t*)area)[16]);
        dmb();
    }*/

/*uint64_t start = rdtsc();

    //dmb();
    for (int i = 0; i < 1000; i++) {
        ((uint64_t*)area)[0] = 10;
        dmb();
        ((uint64_t*)area)[16] = i+1;
        dmb();
    }*/


int main(int argc, char *argv[])
{
    FILE *file = fopen("output.txt", "w");

    if (file == NULL) {
        printf("Failed to open file\n");
    }

    int fd;
    no_cpus = get_nprocs();

    fd = open("/dev/fpgamem", O_RDWR);
    assert(fd >= 0);

    area = mmap((void *)0x100000000000UL, 0x10000000000ULL, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED, fd, 0); // map 1TB
    assert(area != MAP_FAILED);



    for (int i = 0; i < 10; i++) {
    int offset_1 = 0;
    int offset_2 = 16;
    //-----------------------------------------------------------------Timely test case
    //
    uint64_t results[16 * 10];
    //
    // write frontiers 1
    ((uint64_t*)area)[offset_1] = 1;

    for (int i = 1; i < 16; i ++) {
        ((uint64_t*)area)[i+ offset_1] = 0;
    }

    dmb();

    // write frontiers 2
    ((uint64_t*)area)[offset_2] = 1;

    for (int i = 1; i < 16; i++) {
        ((uint64_t*)area)[offset_2 + i] = 0;
    }

    dmb();


    // write data - 1
    for (int i = 0; i < 8; i ++) {
        ((uint64_t*)area)[offset_1+i] = 21;
    }

    for (int i = 8; i < 16; i ++) {
        ((uint64_t*)area)[offset_1+i] = 21;
    }

    dmb();

    // read data 1
    for (int i = 0; i < 16; i++) {
        results[i] = ((uint64_t*)area)[i+offset_2];
    }

    dmb();

    // write data - 2
    //
    for (int i = 0; i < 8; i ++) {
        ((uint64_t*)area)[offset_1+i] = 21;
    }

    for (int i = 8; i < 16; i ++) {
        ((uint64_t*)area)[offset_1+i] = 21;
    }

    dmb();

    // read data 2
    for (int i = 0; i < 16; i++) {
        results[i+16] = ((uint64_t*)area)[i+offset_2];
    }

    dmb();

    // read progress 1 
    for (int i = 0; i < 16; i++) {
        results[i+32] = ((uint64_t*)area)[i+offset_1];
    }

    dmb();

    // read progress 2 
    for (int i = 0; i < 16; i++) {
        results[i+48] = ((uint64_t*)area)[i+offset_2];
    }

    dmb();
    
    // read progress 3 
    for (int i = 0; i < 16; i++) {
        results[i + 64] = ((uint64_t*)area)[i+offset_1];
    }

    dmb();
    
    // read progress 4 
    for (int i = 0; i < 16; i++) {
        results[i + 80] = ((uint64_t*)area)[i+offset_2];
    }


    dmb();
    // read progress 5
    for (int i = 0; i < 16; i++) {
        results[i+96] = ((uint64_t*)area)[i+offset_1];
    }

    dmb();

    // read progress 6
    for (int i = 0; i < 16; i++) {
        results[i+112] = ((uint64_t*)area)[i+offset_2];
    }

    dmb();

    // read progress 7
    for (int i = 0; i < 16; i++) {
        results[i + 128] = ((uint64_t*)area)[i+offset_1];
    }

    dmb();

    // read progress 8
    for (int i = 0; i < 16; i++) {
        results[i + 144] = ((uint64_t*)area)[i+offset_2];
    }

    dmb();

    // ---------------------------------second time 

    /*uint64_t results[16 * 5];
    //
    // write frontiers
    ((uint64_t*)area)[0] = 1;

    for (int i = 1; i < 16; i ++) {
        ((uint64_t*)area)[i] = 0;
    }

    dmb();

    // write data - 1
    for (int i = 0; i < 8; i ++) {
        ((uint64_t*)area)[16+i] = 17;
    }

    for (int i = 8; i < 16; i ++) {
        ((uint64_t*)area)[16+i] = 19;
    }

    dmb();

    // write data - 2
    for (int i = 0; i < 8; i ++) {
        ((uint64_t*)area)[i] = 17;
    }

    for (int i = 8; i < 16; i ++) {
        ((uint64_t*)area)[i] = 19;
    }

    dmb();

    // read data - transfer data, receive result
    for (int i = 16; i < 32; i++) {
        results_2[i-16] = ((uint64_t*)area)[i];
    }

    // read progress 1 
    dmb();
    for (int i = 0; i < 16; i++) {
        results_2[i+16] = ((uint64_t*)area)[i];
    }

    dmb();

    // read progress 2 
    for (int i = 16; i < 32; i++) {
        results_2[i+16] = ((uint64_t*)area)[i];
    }

    dmb();
    
    // read progress 3 
    for (int i = 0; i < 16; i++) {
        results_2[i + 48] = ((uint64_t*)area)[i];
    }

    dmb();
    
    // read progress 4 
    for (int i = 16; i < 32; i++) {
        results_2[i + 48] = ((uint64_t*)area)[i];
    }

    dmb();

*/
    printf("Data result:\n");
    for (int i = 0; i < 32; i++) {
        printf("%lu ", results[i]);
    }
    printf("\n");

    printf("Progress result:\n");
    for (int i = 32; i < 160; i++) {
        printf("%lu ", results[i]);
    }
    printf("\n");

    /*printf("Data result:\n");
    for (int i = 0; i < 16; i++) {
        printf("%lu ", results_2[i]);
    }
    printf("\n");

    printf("Progress result:\n");
    for (int i = 16; i < 80; i++) {
        printf("%lu ", results_2[i]);
    }
    printf("\n");
*/}
    munmap(area, SIZE * no_cpus);
    return 0;
}
