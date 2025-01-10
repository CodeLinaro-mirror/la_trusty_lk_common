/*
 * Copyright (c) 2012-2015 Travis Geiselbrecht
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
#include <arch/mp.h>
#include <assert.h>
#include <bits.h>
#include <err.h>
#include <sys/types.h>
#include <debug.h>
#include <dev/interrupt/arm_gic.h>
#include <inttypes.h>
#include <reg.h>
#include <kernel/thread.h>
#include <kernel/debug.h>
#include <kernel/mp.h>
#include <kernel/vm.h>
#include <lk/init.h>
#include <lk/macros.h>
#include <platform/interrupts.h>
#include <arch/ops.h>
#include <platform/gic.h>
#include <string.h>
#include <trace.h>
#include <inttypes.h>
#if WITH_LIB_SM
#include <lib/sm.h>
#include <lib/sm/sm_err.h>
#endif

#include "arm_gic_common.h"

#if GIC_VERSION > 2
#include "gic_v3.h"
#endif

#define LOCAL_TRACE 0

#if ARCH_ARM
#define iframe arm_iframe
#define IFRAME_PC(frame) ((frame)->pc)
#endif
#if ARCH_ARM64
#define iframe arm64_iframe_short
#define IFRAME_PC(frame) ((frame)->elr)
#endif

void platform_fiq(struct iframe *frame);
static status_t arm_gic_set_secure_locked(u_int irq, bool secure);
static void gic_set_enable(uint vector, bool enable);
static void arm_gic_init_hw(void);

static spin_lock_t gicd_lock;
#if WITH_LIB_SM
#define GICD_LOCK_FLAGS SPIN_LOCK_FLAG_IRQ_FIQ
#else
#define GICD_LOCK_FLAGS SPIN_LOCK_FLAG_INTERRUPTS
#endif
#define GIC_MAX_PER_CPU_INT 32
#define GIC_MAX_SGI_INT 16

#if ARM_GIC_USE_DOORBELL_NS_IRQ
#ifndef GIC_MAX_DEFERRED_ACTIVE_IRQS
#define GIC_MAX_DEFERRED_ACTIVE_IRQS 32
#endif
static bool doorbell_enabled;
#endif

struct arm_gic arm_gics[NUM_ARM_GICS];

static bool arm_gic_check_init(int irq)
{
    /* check if we have a vaddr for gicd, both gicv2 and gicv3/4 use this */
    if (!arm_gics[0].gicd_vaddr) {
        TRACEF("change to interrupt %d ignored before init\n", irq);
        return false;
    }
    return true;
}

#if WITH_LIB_SM
static bool arm_gic_non_secure_interrupts_frozen;

static bool arm_gic_interrupt_change_allowed(int irq)
{
    if (!arm_gic_non_secure_interrupts_frozen)
        return arm_gic_check_init(irq);

    TRACEF("change to interrupt %d ignored after booting ns\n", irq);
    return false;
}
#else
static bool arm_gic_interrupt_change_allowed(int irq)
{
    return arm_gic_check_init(irq);
}
#endif

struct int_handler_struct {
    int_handler handler;
    void *arg;
};

#ifndef WITH_GIC_COMPACT_TABLE
/* Handler and argument storage, per interrupt. */
static struct int_handler_struct int_handler_table_per_cpu[GIC_MAX_PER_CPU_INT][SMP_MAX_CPUS];
static struct int_handler_struct int_handler_table_shared[MAX_INT-GIC_MAX_PER_CPU_INT];

static struct int_handler_struct *get_int_handler(unsigned int vector, uint cpu)
{
    if (vector < GIC_MAX_PER_CPU_INT)
        return &int_handler_table_per_cpu[vector][cpu];
    else if(vector < MAX_INT)
        return &int_handler_table_shared[vector - GIC_MAX_PER_CPU_INT];
    else
        return NULL;
}

static struct int_handler_struct *alloc_int_handler(unsigned int vector, uint cpu) {
    return get_int_handler(vector, cpu);
}

#else /* WITH_GIC_COMPACT_TABLE */

#ifdef WITH_SMP
#error WITH_GIC_COMPACT_TABLE does not support SMP
#endif

/* Maximum count of vector entries that can be registered / handled. */
#ifndef GIC_COMPACT_MAX_HANDLERS
#define GIC_COMPACT_MAX_HANDLERS 16
#endif

/* Array giving a mapping from a vector number to a handler entry index.
 * This structure is kept small so it can be searched reasonably
 * efficiently.  The position in int_handler_vecnum[] gives the index into
 * int_handler_table[].
 */
__attribute__((aligned(CACHE_LINE)))
static uint16_t int_handler_vecnum[GIC_COMPACT_MAX_HANDLERS];
static uint16_t int_handler_count = 0;

/* Handler entries themselves. */
static struct int_handler_struct int_handler_table[GIC_COMPACT_MAX_HANDLERS];

static struct int_handler_struct *bsearch_handler(const uint16_t num, const uint16_t *base, uint_fast16_t count) {
    const uint16_t *bottom = base;

    while (count > 0) {
        const uint16_t *mid = &bottom[count / 2];

        if (num < *mid) {
            count /= 2;
        } else if (num > *mid) {
            bottom = mid + 1;
            count -= count / 2 + 1;
        } else {
            return &int_handler_table[mid - base];
        }
    }

    return NULL;
}

static struct int_handler_struct *get_int_handler(unsigned int vector, uint cpu)
{
    return bsearch_handler(vector, int_handler_vecnum, int_handler_count);
}

static struct int_handler_struct *alloc_int_handler(unsigned int vector, uint cpu)
{
    struct int_handler_struct *handler = get_int_handler(vector, cpu);

    /* Return existing allocation if there is one */
    if (handler) {
        return handler;
    }

    /* Check an allocation is possible */
    assert(int_handler_count < GIC_COMPACT_MAX_HANDLERS);
    assert(spin_lock_held(&gicd_lock));

    /* Find insertion point */
    int i = 0;
    while (i < int_handler_count && vector > int_handler_vecnum[i]) {
        i++;
    }

    /* Move any remainder down */
    const int remainder = int_handler_count - i;
    memmove(&int_handler_vecnum[i + 1], &int_handler_vecnum[i],
            sizeof(int_handler_vecnum[0]) * remainder);
    memmove(&int_handler_table[i + 1], &int_handler_table[i],
            sizeof(int_handler_table[0]) * remainder);

    int_handler_count++;

    /* Initialise the new entry */
    int_handler_vecnum[i] = vector;
    int_handler_table[i].handler = NULL;
    int_handler_table[i].arg = NULL;

    /* Return the allocated handler */
    return &int_handler_table[i];
}
#endif /* WITH_GIC_COMPACT_TABLE */

static bool has_int_handler(unsigned int vector, uint cpu) {
    const struct int_handler_struct *h = get_int_handler(vector, cpu);

    return likely(h && h->handler);
}

#if ARM_GIC_USE_DOORBELL_NS_IRQ
static status_t arm_gic_set_priority_locked(u_int irq, uint8_t priority);
static u_int deferred_active_irqs[SMP_MAX_CPUS][GIC_MAX_DEFERRED_ACTIVE_IRQS];

static status_t reserve_deferred_active_irq_slot(void)
{
    static unsigned int num_handlers = 0;

    if (num_handlers == GIC_MAX_DEFERRED_ACTIVE_IRQS)
        return ERR_NO_MEMORY;

    num_handlers++;
    return NO_ERROR;
}

static status_t defer_active_irq(unsigned int vector, uint cpu)
{
    uint idx;

    for (idx = 0; idx < GIC_MAX_DEFERRED_ACTIVE_IRQS; idx++) {
        u_int irq = deferred_active_irqs[cpu][idx];

        if (!irq)
            break;

        if (irq == vector) {
            TRACEF("irq %d already deferred on cpu %u!\n", irq, cpu);
            return ERR_ALREADY_EXISTS;
        }
    }

    if (idx == GIC_MAX_DEFERRED_ACTIVE_IRQS)
        panic("deferred active irq list is full on cpu %u\n", cpu);

    deferred_active_irqs[cpu][idx] = vector;
    GICCREG_WRITE(0, icc_eoir1_el1, vector);
    LTRACEF_LEVEL(2, "deferred irq %u on cpu %u\n", vector, cpu);
    return NO_ERROR;
}

static void raise_ns_doorbell_irq(uint cpu)
{
    uint64_t reg = arm_gicv3_sgir_val(ARM_GIC_DOORBELL_IRQ, cpu);

    if (doorbell_enabled) {
        LTRACEF("GICD_SGIR: %" PRIx64 "\n", reg);
        GICCREG_WRITE(0, icc_asgi1r_el1, reg);
    }
}

static status_t fiq_enter_defer_irqs(uint cpu)
{
    bool inject = false;

    do {
        u_int irq = GICCREG_READ(0, icc_iar1_el1) & 0x3ff;

        if (irq >= 1020)
            break;

        if (defer_active_irq(irq, cpu) != NO_ERROR)
            break;

        inject = true;
    } while (true);

    if (inject)
        raise_ns_doorbell_irq(cpu);

    return ERR_NO_MSG;
}

static enum handler_return handle_deferred_irqs(void)
{
    enum handler_return ret = INT_NO_RESCHEDULE;
    uint cpu = arch_curr_cpu_num();

    for (uint idx = 0; idx < GIC_MAX_DEFERRED_ACTIVE_IRQS; idx++) {
        struct int_handler_struct *h;
        u_int irq = deferred_active_irqs[cpu][idx];

        if (!irq)
            break;

        h = get_int_handler(irq, cpu);
        if (h->handler && h->handler(h->arg) == INT_RESCHEDULE)
            ret = INT_RESCHEDULE;

        deferred_active_irqs[cpu][idx] = 0;
        GICCREG_WRITE(0, icc_dir_el1, irq);
        LTRACEF_LEVEL(2, "handled deferred irq %u on cpu %u\n", irq, cpu);
    }

    return ret;
}
#endif

void register_int_handler(unsigned int vector, int_handler handler, void *arg)
{
    struct int_handler_struct *h;
    uint cpu = arch_curr_cpu_num();

    spin_lock_saved_state_t state;

    if (vector >= MAX_INT)
        panic("register_int_handler: vector out of range %d\n", vector);

    spin_lock_save(&gicd_lock, &state, GICD_LOCK_FLAGS);

    if (arm_gic_interrupt_change_allowed(vector)) {
#if ARM_GIC_USE_DOORBELL_NS_IRQ
        if (reserve_deferred_active_irq_slot() != NO_ERROR) {
            panic("register_int_handler: exceeded %d deferred active irq slots\n",
                  GIC_MAX_DEFERRED_ACTIVE_IRQS);
        }
#endif
#if GIC_VERSION > 2
        arm_gicv3_configure_irq_locked(cpu, vector);
#endif
        h = alloc_int_handler(vector, cpu);
        h->handler = handler;
        h->arg = arg;
#if ARM_GIC_USE_DOORBELL_NS_IRQ
        arm_gic_set_priority_locked(vector, 0x7f);
#endif

        /*
         * For GICv3, SGIs are maskable, and on GICv2, whether they are
         * maskable is implementation defined. As a result, the caller cannot
         * rely on them being maskable, so we enable all registered SGIs as if
         * they were non-maskable.
         */
        if (vector < GIC_MAX_SGI_INT) {
            gic_set_enable(vector, true);
        }
    }

    spin_unlock_restore(&gicd_lock, state, GICD_LOCK_FLAGS);
}

#define GIC_REG_COUNT(bit_per_reg) DIV_ROUND_UP(MAX_INT, (bit_per_reg))
#define DEFINE_GIC_SHADOW_REG(name, bit_per_reg, init_val, init_from) \
    uint32_t (name)[GIC_REG_COUNT(bit_per_reg)] = { \
        [(init_from / bit_per_reg) ... \
         (GIC_REG_COUNT(bit_per_reg) - 1)] = (init_val) \
    }

#if WITH_LIB_SM
static DEFINE_GIC_SHADOW_REG(gicd_igroupr, 32, ~0U, 0);
#endif
static DEFINE_GIC_SHADOW_REG(gicd_itargetsr, 4, 0x01010101, 32);

static void gic_set_enable(uint vector, bool enable)
{
    int reg = vector / 32;
    uint32_t mask = 1ULL << (vector % 32);

#if GIC_VERSION > 2
    if (reg == 0) {
        uint32_t cpu = arch_curr_cpu_num();

        /* On GICv3/v4 these are on GICR */
        if (enable)
            GICRREG_WRITE(0, cpu, GICR_ISENABLER0, mask);
        else
            GICRREG_WRITE(0, cpu, GICR_ICENABLER0, mask);
        return;
    }
#endif
    if (enable)
        GICDREG_WRITE(0, GICD_ISENABLER(reg), mask);
    else {
        GICDREG_WRITE(0, GICD_ICENABLER(reg), mask);

#if GIC_VERSION > 2
        /* for GIC V3, make sure write is complete */
        arm_gicv3_wait_for_write_complete();
#endif
    }
}

static void arm_gic_init_percpu(uint level)
{
#if GIC_VERSION > 2
    /* GICv3/v4 */
    arm_gicv3_init_percpu();
#else
    /* GICv2 */
#if WITH_LIB_SM
    GICCREG_WRITE(0, GICC_CTLR, 0xb); // enable GIC0 and select fiq mode for secure
    GICDREG_WRITE(0, GICD_IGROUPR(0), ~0U); /* GICD_IGROUPR0 is banked */
#else
    GICCREG_WRITE(0, GICC_CTLR, 1); // enable GIC0
#endif
    GICCREG_WRITE(0, GICC_PMR, 0xFF); // unmask interrupts at all priority levels
#endif /* GIC_VERSION > 2 */
}

LK_INIT_HOOK_FLAGS(arm_gic_init_percpu,
                   arm_gic_init_percpu,
                   LK_INIT_LEVEL_PLATFORM_EARLY, LK_INIT_FLAG_SECONDARY_CPUS);

static void arm_gic_suspend_cpu(uint level)
{
#if GIC_VERSION > 2
    arm_gicv3_suspend_cpu(arch_curr_cpu_num());
#endif
}

LK_INIT_HOOK_FLAGS(arm_gic_suspend_cpu, arm_gic_suspend_cpu,
                   LK_INIT_LEVEL_PLATFORM, LK_INIT_FLAG_CPU_OFF);

static void arm_gic_resume_cpu(uint level)
{
    spin_lock_saved_state_t state;
    __UNUSED bool resume_gicd = false;

    spin_lock_save(&gicd_lock, &state, GICD_LOCK_FLAGS);

#if GIC_VERSION > 2
    if (!(GICDREG_READ(0, GICD_CTLR) & 5)) {
#else
    if (!(GICDREG_READ(0, GICD_CTLR) & 1)) {
#endif
        dprintf(SPEW, "%s: distibutor is off, calling arm_gic_init instead\n", __func__);
        arm_gic_init_hw();
        resume_gicd = true;
    } else {
        arm_gic_init_percpu(0);
    }

#if GIC_VERSION > 2
    {
        uint cpu = arch_curr_cpu_num();
        uint max_irq = resume_gicd ? MAX_INT : GIC_MAX_PER_CPU_INT;

        for (uint v = 0; v < max_irq; v++) {
            if (has_int_handler(v, cpu)) {
                arm_gicv3_configure_irq_locked(cpu, v);
            }
        }
        arm_gicv3_resume_cpu_locked(cpu, resume_gicd);
    }
#endif
    spin_unlock_restore(&gicd_lock, state, GICD_LOCK_FLAGS);
}

LK_INIT_HOOK_FLAGS(arm_gic_resume_cpu, arm_gic_resume_cpu,
                   LK_INIT_LEVEL_PLATFORM, LK_INIT_FLAG_CPU_RESUME);

static int arm_gic_max_cpu(void)
{
    return (GICDREG_READ(0, GICD_TYPER) >> 5) & 0x7;
}

static void arm_gic_init_hw(void)
{
#if GIC_VERSION > 2
    /* GICv3/v4 */
    arm_gicv3_init();
#else
    int i;

    for (i = 0; i < MAX_INT; i+= 32) {
        GICDREG_WRITE(0, GICD_ICENABLER(i / 32), ~0U);
        GICDREG_WRITE(0, GICD_ICPENDR(i / 32), ~0U);
    }

    if (arm_gic_max_cpu() > 0) {
        /* Set external interrupts to target cpu 0 */
        for (i = 32; i < MAX_INT; i += 4) {
            GICDREG_WRITE(0, GICD_ITARGETSR(i / 4), gicd_itargetsr[i / 4]);
        }
    }

    GICDREG_WRITE(0, GICD_CTLR, 1); // enable GIC0
#if WITH_LIB_SM
    GICDREG_WRITE(0, GICD_CTLR, 3); // enable GIC0 ns interrupts
    /*
     * Iterate through all IRQs and set them to non-secure
     * mode. This will allow the non-secure side to handle
     * all the interrupts we don't explicitly claim.
     */
    for (i = 32; i < MAX_INT; i += 32) {
        u_int reg = i / 32;
        GICDREG_WRITE(0, GICD_IGROUPR(reg), gicd_igroupr[reg]);
    }
#endif
#endif /* GIC_VERSION > 2 */
    arm_gic_init_percpu(0);
}

void arm_gic_init(void) {
#ifdef GICBASE
    arm_gics[0].gicd_vaddr = GICBASE(0) + GICD_OFFSET;
    arm_gics[0].gicd_size = GICD_MIN_SIZE;
#if GIC_VERSION > 2
    arm_gics[0].gicr_vaddr = GICBASE(0) + GICR_OFFSET;
    arm_gics[0].gicr_size = GICR_CPU_OFFSET(SMP_MAX_CPUS - 1) + GICR_MIN_SIZE;
#else /* GIC_VERSION > 2 */
    arm_gics[0].gicc_vaddr = GICBASE(0) + GICC_OFFSET;
    arm_gics[0].gicc_size = GICC_MIN_SIZE;
#endif /* GIC_VERSION > 2 */
#else
    /* Platforms should define GICBASE if they want to call this */
    panic("%s: GICBASE not defined\n", __func__);
#endif /* GICBASE */

    arm_gic_init_hw();
}

static void arm_map_regs(const char* name,
                         vaddr_t* vaddr,
                         paddr_t paddr,
                         size_t size) {
    status_t ret;
    void* vaddrp = (void*)vaddr;

    if (!size) {
        return;
    }

    ret = vmm_alloc_physical(vmm_get_kernel_aspace(), "gic", size, &vaddrp, 0,
                             paddr, 0, ARCH_MMU_FLAG_UNCACHED_DEVICE |
                                 ARCH_MMU_FLAG_PERM_NO_EXECUTE);
    if (ret) {
        panic("%s: failed %d\n", __func__, ret);
    }

    *vaddr = (vaddr_t)vaddrp;
}

void arm_gic_init_map(struct arm_gic_init_info* init_info)
{
    if (init_info->gicd_size < GICD_MIN_SIZE) {
        panic("%s: gicd mapping too small %zu\n", __func__,
              init_info->gicd_size);
    }
    arm_map_regs("gicd", &arm_gics[0].gicd_vaddr, init_info->gicd_paddr,
                 init_info->gicd_size);
    arm_gics[0].gicd_size = init_info->gicd_size;

#if GIC_VERSION > 2
    if (init_info->gicr_size < GICR_CPU_OFFSET(SMP_MAX_CPUS - 1) + GICR_MIN_SIZE) {
        panic("%s: gicr mapping too small %zu\n", __func__,
              init_info->gicr_size);
    }
    arm_map_regs("gicr", &arm_gics[0].gicr_vaddr, init_info->gicr_paddr,
                 init_info->gicr_size);
    arm_gics[0].gicr_size = init_info->gicr_size;
#else /* GIC_VERSION > 2 */
    if (init_info->gicc_size < GICC_MIN_SIZE) {
        panic("%s: gicc mapping too small %zu\n", __func__,
              init_info->gicc_size);
    }
    arm_map_regs("gicc", &arm_gics[0].gicc_vaddr, init_info->gicc_paddr,
                 init_info->gicc_size);
    arm_gics[0].gicc_size = init_info->gicc_size;
#endif /* GIC_VERSION > 2 */

    arm_gic_init_hw();
}

static status_t arm_gic_set_secure_locked(u_int irq, bool secure)
{
#if WITH_LIB_SM
    int reg = irq / 32;
    uint32_t mask = 1ULL << (irq % 32);

    if (irq >= MAX_INT)
        return ERR_INVALID_ARGS;

    if (secure)
        GICDREG_WRITE(0, GICD_IGROUPR(reg), (gicd_igroupr[reg] &= ~mask));
    else
        GICDREG_WRITE(0, GICD_IGROUPR(reg), (gicd_igroupr[reg] |= mask));
    LTRACEF("irq %d, secure %d, GICD_IGROUP%d = %x\n",
            irq, secure, reg, GICDREG_READ(0, GICD_IGROUPR(reg)));
#endif
    return NO_ERROR;
}

static status_t arm_gic_set_target_locked(u_int irq, u_int cpu_mask, u_int enable_mask)
{
    u_int reg = irq / 4;
    u_int shift = 8 * (irq % 4);
    u_int old_val;
    u_int new_val;

    cpu_mask = (cpu_mask & 0xff) << shift;
    enable_mask = (enable_mask << shift) & cpu_mask;

    old_val = GICDREG_READ(0, GICD_ITARGETSR(reg));
    new_val = (gicd_itargetsr[reg] & ~cpu_mask) | enable_mask;
    GICDREG_WRITE(0, GICD_ITARGETSR(reg), (gicd_itargetsr[reg] = new_val));
    LTRACEF("irq %i, GICD_ITARGETSR%d %x => %x (got %x)\n",
            irq, reg, old_val, new_val, GICDREG_READ(0, GICD_ITARGETSR(reg)));

    return NO_ERROR;
}

static status_t arm_gic_get_priority(u_int irq)
{
    u_int reg = irq / 4;
    u_int shift = 8 * (irq % 4);
    return (GICDREG_READ(0, GICD_IPRIORITYR(reg)) >> shift) & 0xff;
}

static status_t arm_gic_set_priority_locked(u_int irq, uint8_t priority)
{
    u_int reg = irq / 4;
    u_int shift = 8 * (irq % 4);
    u_int mask = 0xffU << shift;
    uint32_t regval;

#if GIC_VERSION > 2
    if (irq < 32) {
        uint cpu = arch_curr_cpu_num();

        /* On GICv3 IPRIORITY registers are on redistributor */
        regval = GICRREG_READ(0, cpu, GICR_IPRIORITYR(reg));
        LTRACEF("irq %i, cpu %d: old GICR_IPRIORITYR%d = %x\n", irq, cpu, reg,
                regval);
        regval = (regval & ~mask) | ((uint32_t)priority << shift);
        GICRREG_WRITE(0, cpu, GICR_IPRIORITYR(reg), regval);
        LTRACEF("irq %i, cpu %d, new GICD_IPRIORITYR%d = %x, req %x\n",
                irq, cpu, reg, GICDREG_READ(0, GICD_IPRIORITYR(reg)), regval);
        return 0;
    }
#endif

    regval = GICDREG_READ(0, GICD_IPRIORITYR(reg));
    LTRACEF("irq %i, old GICD_IPRIORITYR%d = %x\n", irq, reg, regval);
    regval = (regval & ~mask) | ((uint32_t)priority << shift);
    GICDREG_WRITE(0, GICD_IPRIORITYR(reg), regval);
    LTRACEF("irq %i, new GICD_IPRIORITYR%d = %x, req %x\n",
            irq, reg, GICDREG_READ(0, GICD_IPRIORITYR(reg)), regval);

    return 0;
}

status_t arm_gic_sgi(u_int irq, u_int flags, u_int cpu_mask)
{
    if (irq >= 16) {
        return ERR_INVALID_ARGS;
    }

#if GIC_VERSION > 2
    for (size_t cpu = 0; cpu < SMP_MAX_CPUS; cpu++) {
        if (!((cpu_mask >> cpu) & 1)) {
            continue;
        }

        uint64_t val = arm_gicv3_sgir_val(irq, cpu);

        GICCREG_WRITE(0, GICC_PRIMARY_SGIR, val);
    }

#else /* else GIC_VERSION > 2 */

    u_int val =
        ((flags & ARM_GIC_SGI_FLAG_TARGET_FILTER_MASK) << 24) |
        ((cpu_mask & 0xff) << 16) |
        ((flags & ARM_GIC_SGI_FLAG_NS) ? (1U << 15) : 0) |
        (irq & 0xf);

    LTRACEF("GICD_SGIR: %x\n", val);

    GICDREG_WRITE(0, GICD_SGIR, val);

#endif /* else GIC_VERSION > 2 */

    return NO_ERROR;
}

status_t mask_interrupt(unsigned int vector)
{
    if (vector >= MAX_INT)
        return ERR_INVALID_ARGS;

    if (arm_gic_interrupt_change_allowed(vector))
        gic_set_enable(vector, false);

    return NO_ERROR;
}

status_t unmask_interrupt(unsigned int vector)
{
    if (vector >= MAX_INT)
        return ERR_INVALID_ARGS;

    if (arm_gic_interrupt_change_allowed(vector))
        gic_set_enable(vector, true);

    return NO_ERROR;
}

static
enum handler_return __platform_irq(struct iframe *frame)
{
    // get the current vector
    uint32_t iar = GICCREG_READ(0, GICC_PRIMARY_IAR);
    unsigned int vector = iar & 0x3ff;

    if (vector >= 0x3fe) {
#if WITH_LIB_SM && ARM_GIC_USE_DOORBELL_NS_IRQ
        // spurious or non-secure interrupt
        return sm_handle_irq();
#else
        // spurious
        return INT_NO_RESCHEDULE;
#endif
    }

    THREAD_STATS_INC(interrupts);
    KEVLOG_IRQ_ENTER(vector);

    uint cpu = arch_curr_cpu_num();

    LTRACEF_LEVEL(2, "iar 0x%x cpu %u currthread %p vector %d pc 0x%" PRIxPTR "\n", iar, cpu,
                  get_current_thread(), vector, (uintptr_t)IFRAME_PC(frame));

    // deliver the interrupt
    enum handler_return ret;

    ret = INT_NO_RESCHEDULE;
    struct int_handler_struct *handler = get_int_handler(vector, cpu);
    if (handler && handler->handler)
        ret = handler->handler(handler->arg);

    GICCREG_WRITE(0, GICC_PRIMARY_EOIR, iar);
#if ARM_GIC_USE_DOORBELL_NS_IRQ
    GICCREG_WRITE(0, icc_dir_el1, iar);
#endif

    LTRACEF_LEVEL(2, "cpu %u exit %d\n", cpu, ret);

    KEVLOG_IRQ_EXIT(vector);

    return ret;
}

enum handler_return platform_irq(struct iframe *frame)
{
#if WITH_LIB_SM && !ARM_GIC_USE_DOORBELL_NS_IRQ
    uint32_t ahppir = GICCREG_READ(0, GICC_PRIMARY_HPPIR);
    uint32_t pending_irq = ahppir & 0x3ff;
    struct int_handler_struct *h;
    uint cpu = arch_curr_cpu_num();

#if ARM_MERGE_FIQ_IRQ
    {
        uint32_t hppir = GICCREG_READ(0, GICC_HPPIR);
        uint32_t pending_fiq = hppir & 0x3ff;
        if (pending_fiq < MAX_INT) {
            platform_fiq(frame);
            return INT_NO_RESCHEDULE;
        }
    }
#endif

    LTRACEF("ahppir %d\n", ahppir);
    if (pending_irq < MAX_INT && has_int_handler(pending_irq, cpu)) {
        enum handler_return ret = 0;
        uint32_t irq;
        uint8_t old_priority;
        spin_lock_saved_state_t state;

        spin_lock_save(&gicd_lock, &state, GICD_LOCK_FLAGS);

        /* Temporarily raise the priority of the interrupt we want to
         * handle so another interrupt does not take its place before
         * we can acknowledge it.
         */
        old_priority = arm_gic_get_priority(pending_irq);
        arm_gic_set_priority_locked(pending_irq, 0);
        DSB;
        irq = GICCREG_READ(0, GICC_PRIMARY_IAR) & 0x3ff;
        arm_gic_set_priority_locked(pending_irq, old_priority);

        spin_unlock_restore(&gicd_lock, state, GICD_LOCK_FLAGS);

        LTRACEF("irq %d\n", irq);
        h = get_int_handler(pending_irq, cpu);
        if (likely(h && h->handler))
            ret = h->handler(h->arg);
        else
            TRACEF("unexpected irq %d != %d may get lost\n", irq, pending_irq);
        GICCREG_WRITE(0, GICC_PRIMARY_EOIR, irq);
        return ret;
    }
    return sm_handle_irq();
#else
    return __platform_irq(frame);
#endif
}

void platform_fiq(struct iframe *frame)
{
#if WITH_LIB_SM
    sm_handle_fiq();
#else
    PANIC_UNIMPLEMENTED;
#endif
}

#if WITH_LIB_SM
static status_t arm_gic_get_next_irq_locked(u_int min_irq, uint type)
{
#if ARM_GIC_USE_DOORBELL_NS_IRQ
    if (type == TRUSTY_IRQ_TYPE_DOORBELL && min_irq <= ARM_GIC_DOORBELL_IRQ) {
        doorbell_enabled = true;
        return ARM_GIC_DOORBELL_IRQ;
    }
#else
    u_int irq;
    u_int max_irq = type == TRUSTY_IRQ_TYPE_PER_CPU ? GIC_MAX_PER_CPU_INT :
                    type == TRUSTY_IRQ_TYPE_NORMAL ? MAX_INT : 0;
    uint cpu = arch_curr_cpu_num();

    if (type == TRUSTY_IRQ_TYPE_NORMAL && min_irq < GIC_MAX_PER_CPU_INT)
        min_irq = GIC_MAX_PER_CPU_INT;

    for (irq = min_irq; irq < max_irq; irq++)
        if (has_int_handler(irq, cpu))
            return irq;
#endif

    return SM_ERR_END_OF_INPUT;
}

long smc_intc_get_next_irq(struct smc32_args *args)
{
    status_t ret;
    spin_lock_saved_state_t state;

    spin_lock_save(&gicd_lock, &state, GICD_LOCK_FLAGS);

#if !ARM_GIC_USE_DOORBELL_NS_IRQ
    arm_gic_non_secure_interrupts_frozen = true;
#endif
    ret = arm_gic_get_next_irq_locked(args->params[0], args->params[1]);
    LTRACEF("min_irq %d, per_cpu %d, ret %d\n",
            args->params[0], args->params[1], ret);

    spin_unlock_restore(&gicd_lock, state, GICD_LOCK_FLAGS);

    return ret;
}

enum handler_return sm_intc_enable_interrupts(void)
{
#if ARM_GIC_USE_DOORBELL_NS_IRQ
    return handle_deferred_irqs();
#else
    return INT_NO_RESCHEDULE;
#endif
}

static status_t fiq_enter_unexpected_irq(u_int cpu)
{
#if GIC_VERSION > 2
    u_int irq = GICCREG_READ(0, icc_iar0_el1) & 0x3ff;
#else
    u_int irq = GICCREG_READ(0, GICC_IAR) & 0x3ff;
#endif

    LTRACEF("cpu %d, irq %i\n", cpu, irq);

    if (irq >= 1020) {
        LTRACEF("spurious fiq: cpu %d, new %d\n", cpu, irq);
        return ERR_NO_MSG;
    }

#if GIC_VERSION > 2
    GICCREG_WRITE(0, icc_eoir0_el1, irq);
#else
    GICCREG_WRITE(0, GICC_EOIR, irq);
#endif

    dprintf(INFO, "got disabled fiq: cpu %d, new %d\n", cpu, irq);
    return ERR_NOT_READY;
}

status_t sm_intc_fiq_enter(void)
{
    u_int cpu = arch_curr_cpu_num();
#if ARM_GIC_USE_DOORBELL_NS_IRQ
    return fiq_enter_defer_irqs(cpu);
#else
    return fiq_enter_unexpected_irq(cpu);
#endif
}

void sm_intc_raise_doorbell_irq(void)
{
    u_int cpu = arch_curr_cpu_num();
#if ARM_GIC_USE_DOORBELL_NS_IRQ
    raise_ns_doorbell_irq(cpu);
#else
    arch_mp_send_ipi(1U << cpu, MP_IPI_GENERIC);
#endif
}
#endif
