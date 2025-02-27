#
# Copyright (c) 2024, Google, Inc. All rights reserved
#
# Permission is hereby granted, free of charge, to any person obtaining
# a copy of this software and associated documentation files
# (the "Software"), to deal in the Software without restriction,
# including without limitation the rights to use, copy, modify, merge,
# publish, distribute, sublicense, and/or sell copies of the Software,
# and to permit persons to whom the Software is furnished to do so,
# subject to the following conditions:
#
# The above copyright notice and this permission notice shall be
# included in all copies or substantial portions of the Software.
#
# THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
# EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
# MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
# IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
# CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
# TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
# SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
#

LOCAL_DIR := $(GET_LOCAL_DIR)

MODULE := $(LOCAL_DIR)

MODULE_CRATE_NAME := rust_support

MODULE_SRCS := \
	$(LOCAL_DIR)/lib.rs \

# Don't make this module depend on itself.
MODULE_ADD_IMPLICIT_DEPS := false

MODULE_DEPS := \
	$(call FIND_CRATE,bitflags) \
	$(call FIND_CRATE,num-derive) \
	$(call FIND_CRATE,num-traits) \
	$(call FIND_CRATE,log) \
	trusty/kernel/lib/extmem \
	trusty/kernel/lib/ktipc \
	trusty/kernel/lib/vmm_obj_service \
	trusty/user/base/lib/liballoc-rust \
	trusty/user/base/lib/libcompiler_builtins-rust \
	trusty/user/base/lib/libcore-rust \
	trusty/user/base/lib/trusty-std \
	$(LOCAL_DIR)/wrappers \

MODULE_BINDGEN_ALLOW_FUNCTIONS := \
	_panic \
	ext_mem_.* \
	fflush \
	fputs \
	handle_close \
	handle_decref \
	handle_set_detach_ref \
	handle_set_attach \
	handle_set_create \
	handle_set_wait \
	handle_wait \
	handle_ref_is_attached \
	ipc_get_msg \
	ipc_port_connect_async \
	ipc_put_msg \
	ipc_read_msg \
	ipc_send_msg \
	ktipc_server_init \
	ktipc_server_start \
	lk_fiqs_disabled \
	lk_interrupt_restore \
	lk_interrupt_save \
	lk_ints_disabled \
	lk_stdin \
	lk_stdout \
	lk_stderr \
	mutex_acquire_timeout \
	mutex_destroy \
	mutex_init \
	mutex_release \
	thread_create \
	thread_resume \
	thread_sleep_ns \
	vaddr_to_paddr \
	vmm_alloc \
	vmm_alloc_physical_etc \
	vmm_alloc_contiguous \
	vmm_free_region \
	vmm_get_obj \
	vmm_obj_slice_init \
	vmm_obj_slice_release \
	vmm_obj_service_add \
	vmm_obj_service_create_ro \
	vmm_obj_service_destroy \

MODULE_BINDGEN_ALLOW_TYPES := \
	Error \
	ext_mem_.* \
	handle \
	handle_ref \
	iovec_kern \
	ipc_msg_.* \
	ktipc_port_acl \
	ktipc_server \
	lk_init_.* \
	lk_time_.* \
	spin_lock_save_flags_t \
	spin_lock_saved_state_t \
	trusty_ipc_event_type \
	uuid \
	vmm_obj_service \
	vmm_obj_slice \

MODULE_BINDGEN_ALLOW_VARS := \
	.*_PRIORITY \
	_kernel_aspace \
	ARCH_MMU_FLAG_.* \
	DEFAULT_STACK_SIZE \
	FILE \
	IPC_CONNECT_WAIT_FOR_PORT \
	IPC_HANDLE_POLL_.* \
	IPC_PORT_ALLOW_NS_CONNECT \
	IPC_PORT_ALLOW_TA_CONNECT \
	IPC_PORT_PATH_MAX \
	LK_LOGLEVEL_RUST \
	NUM_PRIORITIES \
	PAGE_SIZE \
	PAGE_SIZE_SHIFT \
	SPIN_LOCK_FLAG_FIQ \
	SPIN_LOCK_FLAG_INTERRUPTS \
	SPIN_LOCK_FLAG_IRQ \
	SPIN_LOCK_FLAG_IRQ_FIQ \
	zero_uuid \

MODULE_BINDGEN_FLAGS := \
	--newtype-enum Error \
	--newtype-enum lk_init_level \
	--bitfield-enum lk_init_flags \
	--no-prepend-enum-name \
	--with-derive-custom Error=FromPrimitive \
	--with-derive-custom handle_waiter=Default \
	--with-derive-custom ipc_msg_info=Default \

MODULE_BINDGEN_SRC_HEADER := $(LOCAL_DIR)/bindings.h

MODULE_RUSTFLAGS += \
	-A clippy::disallowed_names \
	-A clippy::type-complexity \
	-A clippy::unnecessary_fallible_conversions \
	-A clippy::unnecessary-wraps \
	-A clippy::unusual-byte-groupings \
	-A clippy::upper-case-acronyms \
	-D clippy::undocumented_unsafe_blocks \

MODULE_RUST_USE_CLIPPY := true

include make/module.mk
