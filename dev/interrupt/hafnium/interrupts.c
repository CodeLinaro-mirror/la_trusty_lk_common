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

#include <arch/mp.h>
#include <arch/ops.h>
#include <err.h>
#include <hf/abi.h>
#include <hf/types.h>
#include <interface/arm_ffa/arm_ffa.h>
#include <kernel/mp.h>
#include <kernel/spinlock.h>
#include <lib/sm.h>
#include <lib/sm/sm_err.h>
#include <lib/smc/smc.h>
#include <lk/init.h>
#include <lk/trace.h>
#include <platform/gic.h>
#include <platform/interrupts.h>
#include <stdatomic.h>

#if ARCH_ARM
#define iframe arm_iframe
#define IFRAME_PC(frame) ((frame)->pc)
#elif ARCH_ARM64
#define iframe arm64_iframe_short
#define IFRAME_PC(frame) ((frame)->elr)
#else
#error "Unknown Trusty architecture for Hafnium"
#endif

#define HF_INTERRUPT_TYPE_IRQ 0
#define HF_INTERRUPT_TYPE_FIQ 1

static spin_lock_t hafnium_interrupts_lock;
#define HF_MAX_PER_CPU_INT 32

struct int_handler_struct {
    _Atomic(int_handler) handler;
    void* arg;
};

static struct int_handler_struct int_handler_table_per_cpu[HF_MAX_PER_CPU_INT]
                                                          [SMP_MAX_CPUS];
static struct int_handler_struct
        int_handler_table_shared[MAX_INT - HF_MAX_PER_CPU_INT];

static struct int_handler_struct* get_int_handler(unsigned int vector,
                                                  uint cpu)
{
    if (vector >= MAX_INT)
        return NULL;
    if (cpu >= SMP_MAX_CPUS)
        return NULL;

    if (vector < HF_MAX_PER_CPU_INT)
        return &int_handler_table_per_cpu[vector][cpu];
    else
        return &int_handler_table_shared[vector - HF_MAX_PER_CPU_INT];
}

enum handler_return platform_irq(struct iframe* frame)
{
    enum handler_return ret = INT_NO_RESCHEDULE;
    enum handler_return hret;
    struct int_handler_struct* h;
    int_handler handler;
    uint cpu = arch_curr_cpu_num();

    for (;;) {
        struct smc_ret8 hvc_ret = hvc8(HF_INTERRUPT_GET, 0, 0, 0, 0, 0, 0, 0);
        uint32_t intnum = hvc_ret.r0;

        if (intnum == HF_INVALID_INTID) {
            /* No more interrupts */
            break;
        }

        if (intnum >= MAX_INT) {
            TRACEF("bad interrupt %d, cpu %d\n", intnum, cpu);
            return INT_NO_RESCHEDULE;
        }

        h = get_int_handler(intnum, cpu);
        /*
         * Load with acquire in order to enforce the same order
         * from register_int_handler between h->handler and h->arg
         */
        handler = h ? atomic_load_explicit(&h->handler, memory_order_acquire) : NULL;
        if (handler) {
            LTRACEF("interrupt %u, cpu %u, handler %p\n", intnum, cpu, handler);
            hret = handler(h->arg);
            if (hret == INT_RESCHEDULE) {
                ret = INT_RESCHEDULE;
            }
        } else {
            TRACEF("bad interrupt %d, cpu %d\n", intnum, cpu);
        }
    }

    return ret;
}

void platform_fiq(struct iframe* frame)
{
    /*
     * The only FIQ we can get on Hafnium is managed exit,
     * and we prefer it as an IRQ because the code is simpler.
     * Trusty does not need an FIQ because the irq-ns-switch-N threads
     * have the highest priority, so they should run immediately.
     * The SP manifest should have a managed-exit-virq line instead.
     */
    panic("Got FIQ on Hafnium\n");
}

void register_int_handler(unsigned int vector, int_handler handler, void* arg)
{
    spin_lock_saved_state_t state;
    struct int_handler_struct* h;
    uint cpu = arch_curr_cpu_num();

    LTRACEF("interrupt %u\n", vector);

    spin_lock_save(&hafnium_interrupts_lock, &state, SPIN_LOCK_FLAG_IRQ_FIQ);
    h = get_int_handler(vector, cpu);
    if (!h)
        panic("%s: invalid vector %u cpu %u\n", __func__, vector, cpu);
    if (h->handler)
        panic("%s: vector %u already registered on cpu %u\n", __func__, vector, cpu);

    h->arg = arg;
    /*
     * Perform an atomic write with release semantics in case other
     * CPUs are currently executing the handler for the current interrupt.
     * They should either see valid values for both arg and handler, or
     * handler==NULL. This is a half-barrier, so it is faster that a full
     * barrier because it only prevents stores above it to be moved below.
     */
    atomic_store_explicit(&h->handler, handler, memory_order_release);
    spin_unlock_restore(&hafnium_interrupts_lock, state, SPIN_LOCK_FLAG_IRQ_FIQ);
}

status_t mask_interrupt(unsigned int vector)
{
    struct smc_ret8 ret;

    if (vector >= MAX_INT)
        return ERR_INVALID_ARGS;

    ret = hvc8(HF_INTERRUPT_ENABLE, vector, 0, HF_INTERRUPT_TYPE_IRQ, 0, 0, 0, 0);
    LTRACEF("interrupt %d, ret %lu\n", vector, ret.r0);
    if (ret.r0) {
        return ERR_INVALID_ARGS;
    }

    return NO_ERROR;
}

status_t unmask_interrupt(unsigned int vector)
{
    spin_lock_saved_state_t state;
    struct int_handler_struct* h;
    int_handler handler;
    uint cpu = arch_curr_cpu_num();
    struct smc_ret8 ret;

    if (vector >= MAX_INT)
        return ERR_INVALID_ARGS;

    spin_lock_save(&hafnium_interrupts_lock, &state, SPIN_LOCK_FLAG_IRQ_FIQ);
    h = get_int_handler(vector, cpu);
    handler = h ? h->handler : NULL;
    spin_unlock_restore(&hafnium_interrupts_lock, state, SPIN_LOCK_FLAG_IRQ_FIQ);
    if (h && !handler) {
        LTRACEF("unmasking irq %u without handler on cpu %u\n", vector, cpu);
        return ERR_NOT_READY;
    }

    ret = hvc8(HF_INTERRUPT_ENABLE, vector, 1, HF_INTERRUPT_TYPE_IRQ, 0, 0, 0, 0);
    LTRACEF("interrupt %d, ret %lu\n", vector, ret.r0);
    if (ret.r0) {
        return ERR_INVALID_ARGS;
    }

    return NO_ERROR;
}

long smc_intc_get_next_irq(struct smc32_args *args)
{
    /*
     * TODO: we need to return a doorbell here for test-runner,
     * but we cannot use the SRI unconditionally because Linux
     * handles that in the FF-A driver.
     */
    return SM_ERR_END_OF_INPUT;
}

status_t sm_intc_fiq_enter(void)
{
    /*
     * This is called by the FF-A handler FFA_INTERRUPT,
     * which should never happen on Hafnium.
     * With sri-interrupts-policy=<3> in the Trusty SP manifest,
     * what should happen instead on a secure interrupt is that
     * Hafnium would trigger an SRI, causing the primary
     * scheduler to give Trusty cycles with FFA_RUN.
     * When that happens, we catch the interrupt normally through
     * the exception handler after interrupts are enabled.
     */
    panic("Got FFA_INTERRUPT on Hafnium\n");
}

enum handler_return sm_intc_enable_interrupts(void)
{
    /* Nothing to do here */
    return INT_NO_RESCHEDULE;
}

void sm_intc_raise_doorbell_irq(void)
{
    struct smc_ret8 ret;
    uint cpu = arch_curr_cpu_num();

    ret = hvc8(HF_INTERRUPT_SEND_IPI, cpu, 0, 0, 0, 0, 0, 0);
    if (ret.r0) {
        TRACEF("Self-IPI send failure %ld\n", (long)ret.r0);
    }
}

status_t arch_mp_send_ipi(mp_cpu_mask_t target, mp_ipi_t ipi)
{
    struct smc_ret8 ret;

    LTRACEF("IPI %u mask 0x%x\n", ipi, target);

    if (ipi != MP_IPI_RESCHEDULE) {
        TRACEF("unexpected Hafnium IPI %u\b", ipi);
        return ERR_NOT_SUPPORTED;
    }

    for (size_t cpu = 0; cpu < SMP_MAX_CPUS; cpu++) {
        if (!((target >> cpu) & 1)) {
            continue;
        }
        ret = hvc8(HF_INTERRUPT_SEND_IPI, cpu, 0, 0, 0, 0, 0, 0);
        if (ret.r0) {
            TRACEF("IPI send failure %ld\n", (long)ret.r0);
            return ERR_INVALID_ARGS;
        }
    }

    return NO_ERROR;
}

void arch_mp_register_ipi_handler(mp_ipi_t ipi, int_handler handler, void *arg)
{
    if (ipi == MP_IPI_RESCHEDULE) {
        register_int_handler(HF_IPI_INTID, handler, arg);
        unmask_interrupt(HF_IPI_INTID);
    }
}

static enum handler_return managed_exit_handler(void *arg)
{
    return sm_handle_irq();
}

static void hafnium_interrupts_init(uint level)
{
    /* TODO: get the intid using FFA_FEATURE_MEI call */
    register_int_handler(HF_MANAGED_EXIT_INTID, &managed_exit_handler, NULL);
    unmask_interrupt(HF_MANAGED_EXIT_INTID);
}

LK_INIT_HOOK_FLAGS(hafnium_interrupts_init, hafnium_interrupts_init,
                   LK_INIT_LEVEL_PLATFORM_EARLY, LK_INIT_FLAG_ALL_CPUS);
