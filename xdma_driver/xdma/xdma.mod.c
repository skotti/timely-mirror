#include <linux/build-salt.h>
#include <linux/module.h>
#include <linux/vermagic.h>
#include <linux/compiler.h>

BUILD_SALT;

MODULE_INFO(vermagic, VERMAGIC_STRING);
MODULE_INFO(name, KBUILD_MODNAME);

__visible struct module __this_module
__section(.gnu.linkonce.this_module) = {
	.name = KBUILD_MODNAME,
	.init = init_module,
#ifdef CONFIG_MODULE_UNLOAD
	.exit = cleanup_module,
#endif
	.arch = MODULE_ARCH_INIT,
};

#ifdef CONFIG_RETPOLINE
MODULE_INFO(retpoline, "Y");
#endif

static const struct modversion_info ____versions[]
__used __section(__versions) = {
	{ 0x5d619492, "module_layout" },
	{ 0x2d3385d3, "system_wq" },
	{ 0x5c54faab, "kmem_cache_destroy" },
	{ 0xc4595089, "dma_direct_unmap_sg" },
	{ 0x7d80239d, "cdev_del" },
	{ 0x792594c1, "kmalloc_caches" },
	{ 0xeb233a45, "__kmalloc" },
	{ 0xa8a37032, "cdev_init" },
	{ 0x1fdc7df2, "_mcount" },
	{ 0xb414e8ff, "pci_write_config_word" },
	{ 0xd6ee688f, "vmalloc" },
	{ 0xb674d990, "pci_read_config_byte" },
	{ 0x75359443, "dma_set_mask" },
	{ 0x75279966, "pcie_set_readrq" },
	{ 0xec6711b5, "pci_disable_device" },
	{ 0x8bb91709, "pci_disable_msix" },
	{ 0x351328d4, "set_page_dirty_lock" },
	{ 0xaf507de1, "__arch_copy_from_user" },
	{ 0x837b7b09, "__dynamic_pr_debug" },
	{ 0x72e7ffa, "device_destroy" },
	{ 0xe74de659, "kobject_set_name" },
	{ 0x87b8798d, "sg_next" },
	{ 0xdc6bb8c9, "pci_release_regions" },
	{ 0x6ad45fc9, "pcie_capability_clear_and_set_word" },
	{ 0x409bcb62, "mutex_unlock" },
	{ 0x6091b333, "unregister_chrdev_region" },
	{ 0x999e8297, "vfree" },
	{ 0xf02014b8, "dma_free_attrs" },
	{ 0x7a2af7b4, "cpu_number" },
	{ 0x3c3ff9fd, "sprintf" },
	{ 0xcd794918, "dma_set_coherent_mask" },
	{ 0x56081d20, "kthread_create_on_node" },
	{ 0x15ba50a6, "jiffies" },
	{ 0x3feea40, "cpumask_next" },
	{ 0x748efe7, "kthread_bind" },
	{ 0xd9a5ea54, "__init_waitqueue_head" },
	{ 0x17de3d5, "nr_cpu_ids" },
	{ 0x26b2b260, "pci_set_master" },
	{ 0x2b49a455, "pci_alloc_irq_vectors_affinity" },
	{ 0xdcb764ad, "memset" },
	{ 0xd12f7b36, "pci_restore_state" },
	{ 0x977f511b, "__mutex_init" },
	{ 0xc5850110, "printk" },
	{ 0xf9dc3216, "kthread_stop" },
	{ 0x5e3240a0, "__cpu_online_mask" },
	{ 0xa1c76e0a, "_cond_resched" },
	{ 0x978e2965, "pci_read_config_word" },
	{ 0x91512047, "dma_alloc_attrs" },
	{ 0x9ec664b2, "kmem_cache_free" },
	{ 0x2ab7989d, "mutex_lock" },
	{ 0xfa2897a9, "finish_swait" },
	{ 0x95159e24, "device_create" },
	{ 0x2072ee9b, "request_threaded_irq" },
	{ 0x6091797f, "synchronize_rcu" },
	{ 0x7892d1a1, "pci_enable_msi" },
	{ 0xfe487975, "init_wait_entry" },
	{ 0xafa5d586, "pci_find_capability" },
	{ 0x1c246e46, "cdev_add" },
	{ 0x3a2f6702, "sg_alloc_table" },
	{ 0xe6025196, "kmem_cache_alloc" },
	{ 0x618911fc, "numa_node" },
	{ 0x6b2941b2, "__arch_copy_to_user" },
	{ 0x9c1e5bf5, "queued_spin_lock_slowpath" },
	{ 0x6e7f6a1d, "pci_cleanup_aer_uncorrect_error_status" },
	{ 0xdecd0b29, "__stack_chk_fail" },
	{ 0x1000e51, "schedule" },
	{ 0x8ddd8aad, "schedule_timeout" },
	{ 0x1d24c881, "___ratelimit" },
	{ 0xc8118ba3, "prepare_to_swait_event" },
	{ 0xdaca81bf, "cpu_hwcaps" },
	{ 0xf3ecac14, "cpu_hwcap_keys" },
	{ 0xe0bd10d2, "wake_up_process" },
	{ 0x2be2b887, "pci_unregister_driver" },
	{ 0xc60f1c54, "kmem_cache_alloc_trace" },
	{ 0x3928efe9, "__per_cpu_offset" },
	{ 0xefb55d3b, "kmem_cache_create" },
	{ 0xa1d782b0, "pci_irq_vector" },
	{ 0x3eeb2322, "__wake_up" },
	{ 0xb3f7646e, "kthread_should_stop" },
	{ 0x8c26d495, "prepare_to_wait_event" },
	{ 0x37a0cba, "kfree" },
	{ 0xfe26585, "dma_direct_map_sg" },
	{ 0xaea93571, "remap_pfn_range" },
	{ 0x4829a47e, "memcpy" },
	{ 0x39b3b1ff, "pci_request_regions" },
	{ 0x6df1aaf1, "kernel_sigaction" },
	{ 0x4f5823b7, "pci_disable_msi" },
	{ 0x41832426, "__pci_register_driver" },
	{ 0xbf39f6e0, "class_destroy" },
	{ 0x92540fbf, "finish_wait" },
	{ 0x608741b5, "__init_swait_queue_head" },
	{ 0x7f5b4fe4, "sg_free_table" },
	{ 0xc5b6f236, "queue_work_on" },
	{ 0x656e4a6e, "snprintf" },
	{ 0x3ff1f906, "pci_iomap" },
	{ 0x9392add9, "pci_enable_device_mem" },
	{ 0x7f02188f, "__msecs_to_jiffies" },
	{ 0x8c9612b3, "pci_enable_device" },
	{ 0x5e5ea965, "param_ops_uint" },
	{ 0x14b89635, "arm64_const_caps_ready" },
	{ 0x72cc9508, "__class_create" },
	{ 0x25ea0a4, "flush_dcache_page" },
	{ 0x88db9f48, "__check_object_size" },
	{ 0xe3ec2f2b, "alloc_chrdev_region" },
	{ 0x1c3556a6, "__put_page" },
	{ 0x9a2a94e8, "get_user_pages_fast" },
	{ 0xc80ab559, "swake_up_one" },
	{ 0xc1514a3b, "free_irq" },
	{ 0xa3fa00fb, "pci_save_state" },
};

MODULE_INFO(depends, "");

MODULE_ALIAS("pci:v000010EEd00009048sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009044sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009042sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009041sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd0000903Fsv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009038sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009028sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009018sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009034sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009024sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009014sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009032sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009022sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009012sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009031sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009021sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00009011sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00008011sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00008012sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00008014sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00008018sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00008021sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00008022sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00008024sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00008028sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00008031sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00008032sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00008034sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00008038sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00007011sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00007012sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00007014sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00007018sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00007021sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00007022sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00007024sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00007028sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00007031sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00007032sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00007034sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00007038sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00006828sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00006830sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00006928sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00006930sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00006A28sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00006A30sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00006D30sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00004808sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00004828sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00004908sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00004A28sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00004B28sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v000010EEd00002808sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v00001D0Fd0000F000sv*sd*bc*sc*i*");
MODULE_ALIAS("pci:v00001D0Fd0000F001sv*sd*bc*sc*i*");

MODULE_INFO(srcversion, "AE703708EC3B4FE4A2AC78D");
