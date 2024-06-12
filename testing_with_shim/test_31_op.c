#include <string.h>
#include <stdio.h>
#include <malloc.h>
#include <time.h>
#include <sys/time.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdint.h>


#include "xdma_shim.h"

#define SIZE_1 88
#define SIZE_2 144


#define OUTPUT_BYTES 256256
#define OUTPUT_N 32032
#define INPUT_BYTES 257024
#define INPUT_N 32128
#define FRONTIERS_N 32 

int main(int argc, char *argv[])
{

    struct HardwareCommon* hc = initialize(INPUT_BYTES, OUTPUT_BYTES);

    //----------------------------------

//---------------------------------------------------

    uint64_t* output = (uint64_t*) hc->mem_out;
    for (int i = 0; i < FRONTIERS_N; i++) {
	    output[i] = 1;
    }

    for (int i = FRONTIERS_N; i < OUTPUT_N; i++) {
	    output[i] = 21;
    }

    run(hc, INPUT_BYTES, OUTPUT_BYTES);

    int64_t* input = (int64_t*) hc->mem_in;

    for (int i = 0; i < INPUT_N; i++) {
        printf("%ld ", input[i]);
    }
    printf("\n");

//----------------------------------------------------
    closeHardware(hc);
    //free(hc);
    return 1;

}
