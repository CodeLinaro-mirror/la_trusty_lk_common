/*
 * Copyright (c) 2024 LK Trusty Authors. All Rights Reserved.
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files
 * (the "Software"), to deal in the Software without restriction,
 * including without limitation the rights to use, copy, modify, merge,
 * publish, distribute, sublicense, and/or sell copies of the Software,
 * and to permit persons to whom the Software is furnished to do so,
 * subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be
 * included in all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
 * EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
 * CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
 * TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
 * SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */

#define LOCAL_TRACE 0

#include <arch/ops.h>
#include <err.h>
#include <lib/sm.h>
#include <lib/sm/sm_err.h>
#include <lib/smc/smc.h>
#include <lk/init.h>
#include <lk/trace.h>
#include <platform/interrupts.h>

#if ARCH_ARM
#define iframe arm_iframe
#define IFRAME_PC(frame) ((frame)->pc)
#elif ARCH_ARM64
#define iframe arm64_iframe_short
#define IFRAME_PC(frame) ((frame)->elr)
#else
#error "Unknown Trusty architecture for Hafnium"
#endif

enum handler_return platform_irq(struct iframe* frame)
{
    PANIC_UNIMPLEMENTED;
}

void platform_fiq(struct iframe* frame) {
    panic("Got FIQ on Hafnium\n");
}

void register_int_handler(unsigned int vector, int_handler handler, void* arg)
{
    PANIC_UNIMPLEMENTED;
}

status_t mask_interrupt(unsigned int vector)
{
    PANIC_UNIMPLEMENTED;
}

status_t unmask_interrupt(unsigned int vector)
{
    PANIC_UNIMPLEMENTED;
}

long smc_intc_get_next_irq(struct smc32_args *args)
{
    PANIC_UNIMPLEMENTED;
}

status_t sm_intc_fiq_enter(void)
{
    PANIC_UNIMPLEMENTED;
}

enum handler_return sm_intc_enable_interrupts(void)
{
    PANIC_UNIMPLEMENTED;
}

void sm_intc_raise_doorbell_irq(void)
{
    PANIC_UNIMPLEMENTED;
}

static void hafnium_interrupts_init(uint level)
{
    PANIC_UNIMPLEMENTED;
}

LK_INIT_HOOK_FLAGS(hafnium_interrupts_init, hafnium_interrupts_init,
                   LK_INIT_LEVEL_PLATFORM_EARLY, LK_INIT_FLAG_ALL_CPUS);
