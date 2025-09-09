/*
 * Copyright (c) 2014 Travis Geiselbrecht
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
#include <trace.h>
#include <err.h>
#include <platform/interrupts.h>
#include <arch/ops.h>

#define LOCAL_TRACE 0

enum handler_return arm_ipi_generic_handler(void *arg)
{
    LTRACEF("cpu %u, arg %p\n", arch_curr_cpu_num(), arg);

    return INT_NO_RESCHEDULE;
}

enum handler_return arm_ipi_reschedule_handler(void *arg)
{
    LTRACEF("cpu %u, arg %p\n", arch_curr_cpu_num(), arg);

    return mp_mbx_reschedule_irq();
}

void arch_mp_init_percpu(void)
{
    arch_mp_register_ipi_handler(MP_IPI_GENERIC, &arm_ipi_generic_handler, 0);
    arch_mp_register_ipi_handler(MP_IPI_RESCHEDULE, &arm_ipi_reschedule_handler, 0);
}

