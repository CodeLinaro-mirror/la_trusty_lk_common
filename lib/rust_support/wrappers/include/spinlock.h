/*
 * Copyright (c) 2025 Google Inc. All rights reserved
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

#include <kernel/spinlock.h>

/**
 * Can be safely called in any context.
 */
void lk_interrupt_save(spin_lock_saved_state_t *statep,
                       spin_lock_save_flags_t flags);

/**
 * Can be safely called in any context.
 *
 * `state` should be from the corresponding call to `lk_interrupt_save`,
 * and `flags` should be the same `flags` from that `lk_interrupt_save` call.
 */
void lk_interrupt_restore(spin_lock_saved_state_t old_state,
                          spin_lock_save_flags_t flags);

/**
 * Can be safely called in any context.
 */
bool lk_ints_disabled(void);

#if defined(__arm__) || defined(__aarch64__)
/**
 * Can be safely called in any context.
 */
bool lk_fiqs_disabled(void);
#endif
