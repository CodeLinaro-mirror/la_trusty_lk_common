LOCAL_DIR := $(GET_LOCAL_DIR)

MODULE := $(LOCAL_DIR)

MODULE_DEPS := \
	platform \
	target \
	app \
	dev \
	kernel

MODULE_SRCS := \
	$(LOCAL_DIR)/init.c \
	$(LOCAL_DIR)/main.c \

# Stack size for bootstrap2 thread that runs init hooks when booting primary
# CPU. This only applies to init hooks from LK_INIT_LEVEL_THREADING and up.
ifdef PRIMARY_BOOTSTRAP2_STACK_SIZE
MODULE_DEFINES += PRIMARY_BOOTSTRAP2_STACK_SIZE=$(PRIMARY_BOOTSTRAP2_STACK_SIZE)
endif

# Stack sizes for secondarybootstrap2 threads that runs init hooks when booting
# secondary CPUs. This only applies to init hooks from LK_INIT_LEVEL_THREADING
# and up.
ifdef SECONDARY_BOOTSTRAP2_STACK_SIZE
MODULE_DEFINES += SECONDARY_BOOTSTRAP2_STACK_SIZE=$(SECONDARY_BOOTSTRAP2_STACK_SIZE)
endif

EXTRA_LINKER_SCRIPTS += $(LOCAL_DIR)/init.ld

include make/module.mk
