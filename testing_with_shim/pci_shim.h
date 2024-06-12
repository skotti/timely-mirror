/* XDMA shim layer for Timely FPGA offload. */

#ifndef __XDMA_SHIM_H__
#define __XDMA_SHIM_H__
#include <stdint.h>



/* A hardware context for communicating fixed-size packets with an XDMA device
 * with an AXI-Stream interface. */
struct HardwareCommon {
    int   fd;             // descriptor for CPU -> FPGA
    int	  val1_o;
    int   val2_o;
    void* mem;
    void* omem;
    int offset;
};

#ifdef __cplusplus
extern "C" {
#endif

/* Open the Linux XDMA device files specified by XDMA_SHIM_H2C_DEVNAME and
 * XDMA_SHIM_C2H_DEVNAME for CPU to FPGA and FPGA to CPU communication
 * respectively, and allocates page-aligned send and receive buffers of
 * XDMA_SHIM_PACKETSIZE bytes for transfers.  Allocates and returns a struct
 * HardwareCommon, and returns NULL on failure. */
struct HardwareCommon* initialize();

/* Closes an XDMA hardware context and frees all resources. */
void closeHardware(struct HardwareCommon *hc);

void read_data(struct HardwareCommon *hc);

void write_data(struct HardwareCommon *hc);

#ifdef __cplusplus
}
#endif

#endif /* __XDMA_SHIM_H__ */
