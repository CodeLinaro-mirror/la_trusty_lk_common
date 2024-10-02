LOCAL_DIR := $(GET_LOCAL_DIR)
MODULE := $(LOCAL_DIR)
MODULE_CRATE_NAME := vsock
MODULE_SRCS := \
	$(LOCAL_DIR)/src/lib.rs \

MODULE_LIBRARY_DEPS := \
	trusty/user/base/lib/liballoc-rust \
	trusty/user/base/lib/trusty-std \
	external/rust/crates/lazy_static \
	external/rust/crates/log \
	external/rust/crates/static_assertions \
	external/rust/crates/virtio-drivers \

# `trusty-std` is for its `#[global_allocator]`.

MODULE_RUSTFLAGS += \
	-A clippy::disallowed_names \
	-A clippy::type-complexity \
	-A clippy::unnecessary_fallible_conversions \
	-A clippy::unnecessary-wraps \
	-A clippy::unusual-byte-groupings \
	-A clippy::upper-case-acronyms \
	-D clippy::undocumented_unsafe_blocks \

MODULE_RUST_USE_CLIPPY := true

include make/library.mk
