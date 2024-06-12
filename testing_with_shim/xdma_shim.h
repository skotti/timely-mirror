/* XDMA shim layer for Timely FPGA offload. */

#ifndef __XDMA_SHIM_H__
#define __XDMA_SHIM_H__
#include <stdint.h>



/* A hardware context for communicating fixed-size packets with an XDMA device
 * with an AXI-Stream interface. */
struct HardwareCommon {
    int   fd_out;             // descriptor for CPU -> FPGA
    int   fd_in;              // descriptor for FPGA -> CPU
    void *mem_out;            // buffer to send data with CPU -> FPGA
    void *mem_in;             // buffer to receive data with FPGA -> CPU
    //const char *dev_name_out; // char array of device name CPU -> FPGA
    //const char *dev_name_in;  // FPGA -> CPU
};

#ifdef __cplusplus
extern "C" {
#endif

/* Open the Linux XDMA device files specified by XDMA_SHIM_H2C_DEVNAME and
 * XDMA_SHIM_C2H_DEVNAME for CPU to FPGA and FPGA to CPU communication
 * respectively, and allocates page-aligned send and receive buffers of
 * XDMA_SHIM_PACKETSIZE bytes for transfers.  Allocates and returns a struct
 * HardwareCommon, and returns NULL on failure. */
struct HardwareCommon* initialize(uint64_t input_size, uint64_t output_size);

/* Closes an XDMA hardware context and frees all resources. */
void closeHardware(struct HardwareCommon *hc);

/* Writes XDMA_SHIM_PACKETSIZE bytes from the memory pointed to by
 * hc->mem_out to the XDMA H2C channel, then reads the same amount from the
 * C2H channel. */
void run(struct HardwareCommon *hc, uint64_t input_size, uint64_t output_size);

#ifdef __cplusplus
}
#endif

#endif /* __XDMA_SHIM_H__ */
