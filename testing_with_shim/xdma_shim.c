#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <unistd.h>

#include "xdma_shim.h"

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

//const uint64_t xdma_shim_packetsize= XDMA_SHIM_PACKETSIZE;
const char xdma_shim_h2c_devname[]= "/dev/xdma0_h2c_0";
const char xdma_shim_c2h_devname[]= "/dev/xdma0_c2h_0";

struct HardwareCommon *
initialize(uint64_t input_size, uint64_t output_size) {
    struct HardwareCommon *hc;
    int local_errno;

    hc= calloc(1, sizeof(struct HardwareCommon));
    if(!hc) {
        print_error("Failed to allocate hardware context\n", errno);
        goto hc_fail;
    }

    /* Align in and out buffers to 4k. */
    local_errno= posix_memalign(&hc->mem_out, 4096, output_size);
    if(local_errno != 0) {
        print_error("Failed to allocate page-aligned %zuB output buffer\n",
                    local_errno, (size_t)input_size);
        goto mem_out_fail;
    }

    local_errno= posix_memalign(&hc->mem_in, 4096, input_size);
    if(local_errno != 0) {
        print_error("Failed to allocate page-aligned %zuB input buffer\n",
                    local_errno, (size_t)output_size);
        goto mem_in_fail;
    }

    hc->fd_out= open(xdma_shim_h2c_devname, O_RDWR);
    if(hc->fd_out < 0) {
        print_error("Failed to open XDMA H2C device at %s\n", errno,
                    xdma_shim_h2c_devname);
        goto fd_out_fail;
    }

    hc->fd_in= open(xdma_shim_c2h_devname, O_RDWR);
    if(hc->fd_out < 0) {
        print_error("Failed to open XDMA C2H device at %s\n", errno,
                    xdma_shim_c2h_devname);
        goto fd_in_fail;
    }

    /*debug("Initialised context for %zuB writes on H2C channel at %s " \
          "and C2H channel at %s\n",
          (size_t),
          xdma_shim_h2c_devname,
          xdma_shim_c2h_devname);*/

    return hc;

    /* Free the partially-allocated components. */
fd_in_fail:
    close(hc->fd_out);
fd_out_fail:
    free(hc->mem_in);
mem_in_fail:
    free(hc->mem_out);
mem_out_fail:
    free(hc);
hc_fail:
    return NULL;
}

void
closeHardware(struct HardwareCommon *hc) {
    assert(hc);
    assert(hc->fd_in >= 0);
    assert(hc->fd_out >= 0);
    assert(hc->mem_in >= 0);
    assert(hc->mem_out >= 0);

    if(close(hc->fd_in) < 0) {
        print_error("Couldn't close C2H device %s\n", errno,
                    xdma_shim_c2h_devname);
    }
    if(close(hc->fd_out) < 0) {
        print_error("Couldn't close H2C device %s\n", errno,
                    xdma_shim_h2c_devname);
    }
    free(hc->mem_in);
    free(hc->mem_out);
    free(hc);

    debug("Deallocated context for %zuB writes on H2C channel at %s " \
          "and C2H channel at %s\n",
          (size_t)XDMA_SHIM_PACKETSIZE,
          xdma_shim_h2c_devname,
          xdma_shim_c2h_devname);
}

static int
send(struct HardwareCommon *hc, uint64_t output_size) {
    size_t count= 0, total= output_size;
    void *send_ptr= hc->mem_out;

    while(count < total) {
        size_t to_send= total - count;

        ssize_t written= write(hc->fd_out, send_ptr, to_send);
        if(written < 0) return written;

        count+= written;
        send_ptr+= written;
    }

    return 0;
}

static int
receive(struct HardwareCommon *hc, uint64_t input_size) {
    size_t count= 0, total= input_size;
    void *recv_ptr= hc->mem_in;

    while(count < total) {
        size_t to_recv= total - count;

        ssize_t nread= read(hc->fd_in, recv_ptr, to_recv);
        if(nread < 0) return nread;

        count+= nread;
        recv_ptr+= nread;
    }

    return 0;
}

void
run(struct HardwareCommon *hc, uint64_t input_size, uint64_t output_size) {
    assert(hc);
    assert(hc->fd_in >= 0);
    assert(hc->fd_out >= 0);
    assert(hc->mem_in >= 0);
    assert(hc->mem_out >= 0);

    /*debug("Sending %zuB on H2C channel at %s\n",
          (size_t)XDMA_SHIM_PACKETSIZE, xdma_shim_h2c_devname);
*/
    if(send(hc, output_size) < 0) {
        fprintf(stderr, "xdma_shim: send() failed: %s\n", strerror(errno));
        abort();
    }

  /*  debug("Receiving %zuB on C2H channel at %s\n",
          (size_t)XDMA_SHIM_PACKETSIZE, xdma_shim_c2h_devname);
*/
    if(receive(hc, input_size) < 0) {
        fprintf(stderr, "xdma_shim: receive() failed: %s\n", strerror(errno));
        abort();
    }

    debug("Success\n");
}
