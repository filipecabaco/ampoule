/* No child processes exist under the runner (spawn ports return ENOTSUP). */
#ifndef AMPOULE_OS_SYS_WAIT_H
#define AMPOULE_OS_SYS_WAIT_H

#include <errno.h>
#include <sys/types.h>

#define WNOHANG 1
#define WUNTRACED 2

#define WEXITSTATUS(s) (((s) & 0xff00) >> 8)
#define WTERMSIG(s) ((s) & 0x7f)
#define WSTOPSIG(s) WEXITSTATUS(s)
#define WIFEXITED(s) (!WTERMSIG(s))
#define WIFSTOPPED(s) (((s) & 0xff) == 0x7f)
#define WIFSIGNALED(s) (((s) & 0xffff) - 1U < 0xffu)
#define WCOREDUMP(s) ((s) & 0x80)

static inline pid_t waitpid(pid_t p, int *s, int o) { (void)p; (void)s; (void)o; errno = ECHILD; return -1; }
static inline pid_t wait(int *s) { (void)s; errno = ECHILD; return -1; }

#endif
