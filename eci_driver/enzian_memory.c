#include <linux/init.h>
#include <linux/module.h>
#include <linux/kernel.h>
#include <linux/cdev.h>
#include <linux/device.h>
#include <linux/fs.h>
#include <asm/io.h>
#include <linux/mm.h>
#include <linux/pfn_t.h>
#include "enzian_memory.h"

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Adam S. Turowski");
MODULE_DESCRIPTION("Enzian FPGA memory driver.");
MODULE_VERSION("1");

struct mychar_device_data {
    struct cdev cdev;
    void *fpga_memory;
};

static int dev_major = 0;
static struct class *mychardev_class = NULL;
static struct mychar_device_data mychardev_data;

int enzian_memory_open(struct inode *inode, struct file *file)
{
    inode->i_flags = S_DAX;
    return 0;
}

long int enzian_memory_ioctl(struct file *file, unsigned int cmd, unsigned long arg)
{
    switch (cmd) {
    case 0: // L2 Cache Index Writeback Invalidate, SYS CVMCACHEWBIL2I, Xt
        asm volatile("sys #0,c11,c0,#5,%0 \n" :: "r" (arg));
        break;
    case 1: // L2 Cache Index Writeback, SYS CVMCACHEWBL2I, Xt
        asm volatile("sys #0,c11,c0,#6,%0 \n" :: "r" (arg));
        break;
    case 2: // L2 Cache Index Load Tag, SYS CVMCACHELTGL2I, Xt
        asm volatile("sys #0,c11,c0,#7,%0 \n" :: "r" (arg));
        break;
    case 3: // L2 Cache Index Store Tag, SYS CVMCACHESTGL2I, Xt
        asm volatile("sys #0,c11,c1,#0,%0 \n" :: "r" (arg));
        break;
    case 4: // L2 Cache Hit Invalidat, SYS CVMCACHEINVL2, Xt
        asm volatile("sys #0,c11,c1,#1,%0 \n" :: "r" (arg));
        break;
    case 5: // L2 Cache Hit Writeback Invalidate, SYS CVMCACHEWBIL2, Xt <<<
        asm volatile("sys #0,c11,c1,#2,%0 \n" :: "r" (arg));
        break;
    case 6: // L2 Cache Hit Writeback, SYS CVMCACHEWBL2, Xt
        asm volatile("sys #0,c11,c1,#3,%0 \n" :: "r" (arg));
        break;
    case 7: // L2 Cache Fetch and Lock, SYS CVMCACHELCKL2, Xt
        asm volatile("sys #0,c11,c1,#4,%0 \n" :: "r" (arg));
        break;
    case 8: // uTLB read, SYS CVMCACHERDUTLB, Xt
        asm volatile("sys #0,c11,c1,#5,%0 \n" :: "r" (arg));
        break;
    case 9: // MTLB read, SYS CVMCACHERDMTLB, Xt
        asm volatile("sys #0,c11,c1,#6,%0 \n" :: "r" (arg));
        break;
    case 10: // PREFu, SYS CVMCACHEPREFUTLB, Xt
        asm volatile("sys #0,c11,c2,#0,%0 \n" :: "r" (arg));
        break;
    default:
        return -EINVAL;
    }
    return 0;
}

void enzian_memory_vma_open(struct vm_area_struct *vma)
{
}

void enzian_memory_vma_close(struct vm_area_struct *vma)
{
}

static vm_fault_t enzian_memory_fault(struct vm_fault *vmf)
{
    return VM_FAULT_SIGBUS;
}

static vm_fault_t enzian_memory_huge_fault(struct vm_fault *vmf,
        enum page_entry_size pe_size)
{
    unsigned long addr = vmf->address & PUD_MASK;
    struct vm_area_struct *vma = vmf->vma;
    pgprot_t pgprot = vma->vm_page_prot;
    struct mm_struct *mm = vma->vm_mm;
    pud_t *pud = vmf->pud;
    spinlock_t *ptl;
    pfn_t pfn;
    pud_t entry;

    pfn = phys_to_pfn_t(0x10000000000ULL + (vmf->pgoff << PAGE_SHIFT), 0);
    if (pe_size != PE_SIZE_PUD) {
        pr_err("Wrong size!\n");
        return VM_FAULT_SIGBUS;
    }

    ptl = pud_lock(mm, pud);
    entry = pfn_pud(pfn_t_to_pfn(pfn), pgprot);
    entry = pud_mkhuge(entry);
    entry = pte_pud(pte_mkwrite(pud_pte(entry)));
    set_pte_at(mm, addr, (pte_t *)pud, pud_pte(entry));
    spin_unlock(ptl);

    return VM_FAULT_NOPAGE;
}

static struct vm_operations_struct enzian_memory_remap_vm_ops = {
    .open = enzian_memory_vma_open,
    .close = enzian_memory_vma_close,
    .fault = enzian_memory_fault,
    .huge_fault = enzian_memory_huge_fault
};

int enzian_memory_mmap(struct file *file, struct vm_area_struct *vma)
{
    vma->vm_flags |= VM_PFNMAP;
    vma->vm_ops = &enzian_memory_remap_vm_ops;
    enzian_memory_vma_open(vma);
    return 0;
}

int enzian_memory_release(struct inode *inode, struct file *file)
{
    return 0;
}

struct file_operations fops = {
    .open = enzian_memory_open,
    .release = enzian_memory_release,
    .mmap = enzian_memory_mmap,
    .unlocked_ioctl = enzian_memory_ioctl,
};

static int __init enzian_memory_init(void)
{
    int err;
    dev_t dev;

    err = alloc_chrdev_region(&dev, 0, 1, "fpgamem");
    dev_major = MAJOR(dev);
    mychardev_class = class_create(THIS_MODULE, "fpgamem");
    cdev_init(&mychardev_data.cdev, &fops);
    mychardev_data.cdev.owner = THIS_MODULE;
    cdev_add(&mychardev_data.cdev, MKDEV(dev_major, 0), 1);
    device_create(mychardev_class, NULL, MKDEV(dev_major, 0), NULL, "fpgamem");

    return 0;
}

static void __exit enzian_memory_exit(void)
{
    device_destroy(mychardev_class, MKDEV(dev_major, 0));
    class_destroy(mychardev_class);
    unregister_chrdev_region(MKDEV(dev_major, 0), MINORMASK);
}

module_init(enzian_memory_init);
module_exit(enzian_memory_exit);
