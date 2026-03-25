LOCAL_DIR := $(GET_LOCAL_DIR)
MODULE := $(LOCAL_DIR)
MODULE_CRATE_NAME := vsock
MODULE_SRCS := \
	$(LOCAL_DIR)/src/lib.rs \

MODULE_EXPORT_INCLUDES += \
	$(LOCAL_DIR)/include

MODULE_LIBRARY_DEPS := \
	trusty/kernel/lib/shared/peer_id/rust \
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
# virtio-msg specification version (must be either dev2 or alp0)
VSOCK_WITH_VIRTIO_MSG_MIN_SPEC_VERSION ?= alp0

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
	--cfg 'feature="keymint_provisioning_with_thal"' \
	--cfg 'feature="remotelyprovisionedcomponent_default"' \
	--cfg 'feature="secureclock_service"' \

endif
ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_AUTHMGR)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="authmgr"' \

endif
ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_VINTF_TA)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="vintf_ta"' \

endif
ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_PLACEHOLDER_SHARED_SECRET)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="placeholder_shared_secret"' \

endif

ifeq (true,$(call TOBOOL,$(VSOCK_WITH_VIRTIO_MSG_DEVICE)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="virtio_msg_device"' \

endif

ifeq (true,$(call TOBOOL,$(VSOCK_WITH_VIRTIO_MSG_DRIVER)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="virtio_msg_driver"' \

endif

VIRTIO_MSG_SPEC_VERSIONS = dev2 alp0

ifneq ($(filter-out $(VIRTIO_MSG_SPEC_VERSIONS),$(VSOCK_WITH_VIRTIO_MSG_MIN_SPEC_VERSION)),)
$(error unrecognized VSOCK_WITH_VIRTIO_MSG_MIN_SPEC_VERSION, $(VSOCK_WITH_VIRTIO_MSG_MIN_SPEC_VERSION))
endif

MODULE_RUSTFLAGS += \
	--cfg 'feature="virtio_msg_min_spec_version_$(VSOCK_WITH_VIRTIO_MSG_MIN_SPEC_VERSION)"' \


ifeq (true,$(call TOBOOL,$(VSOCK_WITH_DEVICE_TREE)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="device_tree"' \

MODULE_LIBRARY_DEPS += \
	$(LKROOT)/lib/region_alloc \
	packages/modules/Virtualization/libs/libfdt \
	trusty/kernel/lib/dtb_service/rust \

endif

ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_GATEKEEPER_WITH_THAL)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="gatekeeper_with_thal"' \

endif

ifeq (true,$(call TOBOOL,$(TRUSTY_VM_ENABLE_AUTHMGR_VIA_VSOCK)))
MODULE_RUSTFLAGS += --cfg 'feature="tipc_vsock_authmgr"'

endif

ifeq (true,$(call TOBOOL,$(TRUSTY_VM_ENABLE_TIPC_PORT_VIA_VSOCK)))
MODULE_RUSTFLAGS += --cfg 'feature="tipc_vsock_forwarder"'

endif

ifeq (true,$(call TOBOOL,$(TRUSTY_VM_ENABLE_KEYMINT_PROVISIONING)))
MODULE_RUSTFLAGS += --cfg 'feature="keymint_provisioning"'

endif

ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_FINGERGUARD)))
MODULE_RUSTFLAGS += --cfg 'feature="fingerguard"'

endif

ifeq (true,$(call TOBOOL,$(INCLUDE_CMD_PROCESSOR)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="cmd_processor_service"' \

endif

ifeq (true,$(call TOBOOL,$(INCLUDE_MEM_SHARE)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="mem_share_service"' \

endif

ifeq (true,$(call TOBOOL,$(TRUSTY_VM_INCLUDE_RKP_TA)))
MODULE_RUSTFLAGS += \
	--cfg 'feature="vm_attestation_service"' \

endif

MODULE_RUST_USE_CLIPPY := true

# TODO: These are the options used to generate new_bindings.rs from the latest version of the
# virtio-msg headers. Once the headers are in mainline linux and we find a suitable location for
# them they may be generated as part of the build.
VSOCK_WITH_VIRTIO_MSG_HEADERS ?= false

ifeq (true,$(call TOBOOL,$(VSOCK_WITH_VIRTIO_MSG_HEADERS)))
MODULE_BINDGEN_SRC_HEADER := $(LOCAL_DIR)/bindings.h

MODULE_BINDGEN_ALLOW_FILES := \
	.*virtio_config.h \
	.*virtio_msg.h \
	.*virtio_msg_ffa.h \

VIRTIO_MSG_BINDGEN_TYPES := \
	bus_area_share \
	bus_area_share_resp \
	bus_area_unshare \
	bus_area_unshare_resp \
	bus_area_release \
	bus_event_device \
	bus_event_configure \
	bus_event_configure_resp \
	bus_ffa_version \
	bus_ffa_version_resp \
	bus_fifo_configure \
	bus_fifo_configure_resp \
	bus_get_devices \
	bus_ping \
	bus_ping_resp \
	bus_reset_resp \
	bus_status \
	event_avail \
	event_used \
	get_config \
	get_device_info_resp \
	get_device_status_resp \
	get_device_status \
	get_features \
	get_shm \
	get_shm_resp \
	get_vqueue \
	get_vqueue_resp \
	reset_vqueue \
	set_device_status \
	set_device_status_resp \
	set_vqueue \

ZEROCOPY_TRAITS := \
	zerocopy::Immutable \
	zerocopy::FromBytes \
	zerocopy::IntoBytes \
	zerocopy::KnownLayout \

MODULE_BINDGEN_FLAGS := \
	--with-derive-custom .*=Default \
    $(foreach type,$(VIRTIO_MSG_BINDGEN_TYPES),$(foreach trait,$(ZEROCOPY_TRAITS),--with-derive-custom $(type)=$(trait)))

endif

include make/library.mk
