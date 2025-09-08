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
#pragma once

#include <sys/types.h>
#include <kernel/mp.h>
#include <platform/interrupts.h>

/* send inter processor interrupt, if supported */
status_t arch_mp_send_ipi(mp_cpu_mask_t target, mp_ipi_t ipi);

/**
 * arch_mp_register_ipi_handler() - Register a handler for one of the IPIs
 * @ipi: One of the IPIs from &enum mp_ipi.
 * @handler: Function to call when receiving @ipi.
 * @arg: Argument for the handler.
 */
void arch_mp_register_ipi_handler(mp_ipi_t ipi, int_handler handler, void *arg);

void arch_mp_init_percpu(void);
