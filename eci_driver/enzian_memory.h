#ifndef ENZIAN_MEMORY

#define ENZIAN_MEMORY

#include <linux/fs.h>

long int enzian_memory_ioctl(struct file *file, unsigned int cmd, unsigned long arg);

#endif // ENZIAN_MEMORY
