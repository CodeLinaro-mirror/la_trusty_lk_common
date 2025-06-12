LOCAL_DIR := $(GET_LOCAL_DIR)
MODULE := $(LOCAL_DIR)
MODULE_CRATE_NAME := vsock
MODULE_SRCS := \
	$(LOCAL_DIR)/src/lib.rs \

MODULE_EXPORT_INCLUDES += \
	$(LOCAL_DIR)/include

MODULE_LIBRARY_DEPS := \
	trusty/kernel/lib/rand/rust \
	trusty/kernel/lib/trusty/rust \
	trusty/user/base/lib/liballoc-rust \
	trusty/user/base/lib/trusty-std \
	$(call FIND_CRATE,cfg-if) \
	$(call FIND_CRATE,lazy_static) \
	$(call FIND_CRATE,libc) \
	$(call FIND_CRATE,log) \
	$(call FIND_CRATE,num-integer) \
	$(call FIND_CRATE,spin) \
	$(call FIND_CRATE,static_assertions) \
	$(call FIND_CRATE,virtio-drivers-and-devices) \
	$(call FIND_CRATE,zerocopy) \
	lib/libhypervisor \

# `trusty-std` is for its `#[global_allocator]`.


VSOCK_WITH_VIRTIO_MSG_DEVICE ?= false
VSOCK_WITH_VIRTIO_MSG_DRIVER ?= false

# hypervisor_backends supports arm64 and x86-64 only for now
ifeq ($(SUBARCH),x86-64)
MODULE_LIBRARY_DEPS += \
	packages/modules/Virtualization/libs/libhypervisor_backends \

endif
ifeq ($(ARCH),arm64)
MODULE_LIBRARY_DEPS += \
	packages/modules/Virtualization/libs/libhypervisor_backends \
	trusty/kernel/lib/arm_ffa/rust \

endif

ifeq (true,$(call TOBOOL,$(VSOCK_WITH_VIRTIO_MSG_DEVICE)))
MODULE_LIBRARY_DEPS += \
	trusty/kernel/lib/extmem/rust \
	trusty/kernel/lib/sm/rust \

endif

# Size in bytes of the shared memory region over FFA by the virtio-msg vsock
# driver. This is rounded up to be a multiple of the page size.
# Virtio-msg needs at least 14 pages = 56KiB for the following:
# * 6 pages for the vqueues: 2 pages per vqueue (per the spec) times 3 queues
# * 8 pages for the RX buffers (one page per buffer, see src/msg/driver.rs).
VSOCK_VIRTIO_MSG_SHARED_MEMORY_SIZE ?= 65536 # 64 KiB

# The guest context ID of the virtio-msg vsock device. Setting this to a value
# reserved by the virtio specification will trigger a compiler error.
VSOCK_VIRTIO_MSG_DEVICE_GUEST_CID ?= 10

# The maximum number of VMs supported by the virtio-msg transport. Only one vsock device per VM is
# currently supported.
VSOCK_VIRTIO_MSG_NUM_VMS ?= 4

MODULE_RUST_ENV += \
	VSOCK_VIRTIO_MSG_SHARED_MEMORY_SIZE=$(VSOCK_VIRTIO_MSG_SHARED_MEMORY_SIZE) \
	VSOCK_VIRTIO_MSG_DEVICE_GUEST_CID=$(VSOCK_VIRTIO_MSG_DEVICE_GUEST_CID) \
	VSOCK_VIRTIO_MSG_NUM_VMS=$(VSOCK_VIRTIO_MSG_NUM_VMS) \

MODULE_RUSTFLAGS += \
	-A clippy::disallowed_names \
	-A clippy::type-complexity \
	-A clippy::unnecessary_fallible_conversions \
	-A clippy::unnecessary-wraps \
	-A clippy::unusual-byte-groupings \
	-A clippy::upper-case-acronyms \
	-D clippy::undocumented_unsafe_blocks \

ifeq (true,$(call TOBOOL,$(TRUSTY_VM_USE_WIDEVINE_AIDL_COMM)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="widevine_aidl_comm"' \

endif
ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_GATEKEEPER)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="gatekeeper"' \

endif
ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_KEYMINT)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="keymint"' \
	--cfg 'feature="keymint_commservice"' \

endif
ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_AUTHMGR)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="authmgr"' \

endif
ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_VINTF_TA)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="vintf_ta"' \

endif

ifeq (true,$(call TOBOOL,$(VSOCK_WITH_VIRTIO_MSG_DEVICE)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="virtio_msg_device"' \

endif

ifeq (true,$(call TOBOOL,$(VSOCK_WITH_VIRTIO_MSG_DRIVER)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="virtio_msg_driver"' \

endif

ifeq (true,$(call TOBOOL,$(TRUSTY_VM_ENABLE_AUTHMGR_VIA_VSOCK)))
MODULE_RUSTFLAGS += --cfg 'feature="tipc_vsock_authmgr"'

endif

ifeq (true,$(call TOBOOL,$(TRUSTY_VM_ENABLE_TIPC_PORT_VIA_VSOCK)))
MODULE_RUSTFLAGS += --cfg 'feature="tipc_vsock_forwarder"'

endif

MODULE_RUST_USE_CLIPPY := true

include make/library.mk
