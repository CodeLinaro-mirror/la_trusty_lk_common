LOCAL_DIR := $(GET_LOCAL_DIR)
MODULE := $(LOCAL_DIR)
MODULE_CRATE_NAME := hypervisor
MODULE_SRCS := \
	$(LOCAL_DIR)/src/lib.rs \

MODULE_EXPORT_INCLUDES += \
	$(LOCAL_DIR)/include

MODULE_LIBRARY_DEPS := \
	$(call FIND_CRATE,log) \
	$(call FIND_CRATE,spin) \

# hypervisor_backends is arm64-only for now
ifeq ($(ARCH),arm64)
MODULE_LIBRARY_DEPS += \
	packages/modules/Virtualization/libs/libhypervisor_backends \

endif

MODULE_RUST_USE_CLIPPY := true

include make/library.mk
