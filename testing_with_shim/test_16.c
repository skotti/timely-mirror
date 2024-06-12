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

// 16 - 16 : 3 * 8 + 16 =  40 * 8 = 320
// 16 - 16 : 16 + 10 * 8 = 96 * 8 = 768
int main(int argc, char *argv[])
{

    struct HardwareCommon* hc = initialize(768, 320);

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

    for (int i = 24; i < 40; i++) {
	    output[i] = 21;
    }
    for (int i = 0; i < 40; i++) {
        printf("%ld ", output[i]);
    }
    printf("\n");

    run(hc, 768, 320);

    int64_t* input = (int64_t*) hc->mem_in;

    for (int i = 0; i < 96; i++) {
        printf("%ld ", input[i]);
    }
    printf("\n");

//----------------------------------------------------
    closeHardware(hc);
    //free(hc);
    return 1;

}
