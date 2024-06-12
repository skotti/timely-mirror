# Enzian FPGA Memory Linux Driver

This kernel module enables mapping of the FPGA memory into user space with proper page attributes.
It supports only 1GB pages. Linux does not support it fully, therefore it outputs a warning during unmapping.

To build the module:
$ make

To insert the module:
$ sudo insmod enzian_memory.ko

It will create a char device, /dev/fpgamem, which supports mmapping (FPGA memory space access) and ioctls (L2$ control).

The mem.c is an example of how to use the device.
