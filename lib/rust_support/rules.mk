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

MODULE_DEPS := \
	trusty/user/base/lib/liballoc-rust \
	trusty/user/base/lib/trusty-std \

MODULE_BINDGEN_ALLOW_FUNCTIONS := \
	_panic \
	vmm_alloc_physical_etc \
	vmm_alloc_contiguous \
	vmm_free_region \
	vaddr_to_paddr \

MODULE_BINDGEN_ALLOW_VARS := \
	ARCH_MMU_FLAG_.* \
	ERR_.* \
	PAGE_SIZE \
	PAGE_SIZE_SHIFT \
	_kernel_aspace \

MODULE_BINDGEN_SRC_HEADER := $(LOCAL_DIR)/bindings.h

include make/module.mk
