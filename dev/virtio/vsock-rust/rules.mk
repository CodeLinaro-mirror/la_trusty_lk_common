LOCAL_DIR := $(GET_LOCAL_DIR)
MODULE := $(LOCAL_DIR)
MODULE_CRATE_NAME := vsock
MODULE_SRCS := \
	$(LOCAL_DIR)/src/lib.rs \

MODULE_EXPORT_INCLUDES += \
	$(LOCAL_DIR)/include

MODULE_LIBRARY_DEPS := \
	trusty/user/base/lib/liballoc-rust \
	trusty/user/base/lib/trusty-std \
	$(call FIND_CRATE,cfg-if) \
	$(call FIND_CRATE,lazy_static) \
	$(call FIND_CRATE,log) \
	$(call FIND_CRATE,num-integer) \
	$(call FIND_CRATE,spin) \
	$(call FIND_CRATE,static_assertions) \
	$(call FIND_CRATE,virtio-drivers) \

# `trusty-std` is for its `#[global_allocator]`.

# hypervisor_backends is arm64-only for now
ifeq ($(ARCH),arm64)
MODULE_LIBRARY_DEPS += \
	packages/modules/Virtualization/libs/libhypervisor_backends \

endif

MODULE_RUSTFLAGS += \
	-A clippy::disallowed_names \
	-A clippy::type-complexity \
	-A clippy::unnecessary_fallible_conversions \
	-A clippy::unnecessary-wraps \
	-A clippy::unusual-byte-groupings \
	-A clippy::upper-case-acronyms \
	-D clippy::undocumented_unsafe_blocks \

ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_HW_CRYPTO_HAL)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="hwcrypto_hal"' \

endif
ifeq (true,$(call TOBOOL,$(TRUSTY_VM_USE_WIDEVINE_AIDL_COMM)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="widevine_aidl_comm"' \

endif
ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_GATEKEEPER)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="gatekeeper"' \

endif
ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_KEMINT)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="keymint"' \

endif
ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_SECURE_STORAGE_HAL)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="securestorage_hal"' \

endif
ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_AUTHMGR)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="authmgr"' \

endif


MODULE_RUST_USE_CLIPPY := true

include make/library.mk
