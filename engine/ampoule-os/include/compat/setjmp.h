/* setjmp/longjmp come from wasm exception handling (-mllvm -wasm-enable-sjlj). There are no signal
 * masks to save, so the sig* variants are the plain ones; they stay macros so the compiler's
 * setjmp lowering sees a direct setjmp call. */
#include_next <setjmp.h>

#ifndef AMPOULE_OS_SETJMP_H
#define AMPOULE_OS_SETJMP_H

#ifndef sigsetjmp
typedef jmp_buf sigjmp_buf;
#define sigsetjmp(env, savemask) setjmp(env)
#define siglongjmp(env, val) longjmp(env, val)
#endif

#endif
