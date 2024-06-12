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

#define OUTPUT_BYTES 704
#define OUTPUT_N 88
#define INPUT_BYTES 1152
#define INPUT_N 144

// 16 - 16 : 3 * 8 + 64 =  88 * 8 = 704
// 16 - 16 : 64 + 10 * 8 = 144 * 8 = 1152 
int main(int argc, char *argv[])
{

    struct HardwareCommon* hc = initialize(INPUT_BYTES, OUTPUT_BYTES);

    //----------------------------------

//---------------------------------------------------

    uint64_t* output = (uint64_t*) hc->mem_out;
    output[0] = 1; // L
    output[1] = 1; // t
    output[2] = 1; // f1
    output[3] = 1; // f2
    output[4] = 1; // f3
    output[5] = 1; // f4
    output[6] = 1; // f5
    output[7] = 1; // f6
    output[8] = 1; // L
    output[9] = 1; // t
    output[10] = 1; // f1
    output[11] = 1; // f2
    output[12] = 1; // f3
    output[13] = 1; // f4
    output[14] = 1; // f5
    output[15] = 1; // f6
    output[16] = 1; // L
    output[17] = 1; // t
    output[18] = 1; // f1
    output[19] = 1; // f2
    output[20] = 1; // f3
    output[21] = 1; // f4
    output[22] = 1; // f5
    output[23] = 1; // f6

    for (int i = 24; i < OUTPUT_N; i++) {
	    output[i] = 21;
    }
    for (int i = 0; i < OUTPUT_N; i++) {
        printf("%ld ", output[i]);
    }
    printf("\n");

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
