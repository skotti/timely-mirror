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
	{ 0x6091b333, "unregister_chrdev_region" },
	{ 0xbf39f6e0, "class_destroy" },
	{ 0x72e7ffa, "device_destroy" },
	{ 0xdecd0b29, "__stack_chk_fail" },
	{ 0x95159e24, "device_create" },
	{ 0x1c246e46, "cdev_add" },
	{ 0xa8a37032, "cdev_init" },
	{ 0x72cc9508, "__class_create" },
	{ 0xe3ec2f2b, "alloc_chrdev_region" },
	{ 0xc5850110, "printk" },
	{ 0x65e01af9, "__sync_icache_dcache" },
	{ 0x9c1e5bf5, "queued_spin_lock_slowpath" },
	{ 0xf3ecac14, "cpu_hwcap_keys" },
	{ 0x14b89635, "arm64_const_caps_ready" },
	{ 0x1fdc7df2, "_mcount" },
};

MODULE_INFO(depends, "");


MODULE_INFO(srcversion, "6320CD2B010CE9EDBB45BB8");
