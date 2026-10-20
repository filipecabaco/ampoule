/* ampoule-os: the POSIX surface ERTS needs that wasi-libc (wasm32-wasip1-threads) lacks.
 *
 * Force-included into every ERTS compile unit (-include ampoule_os.h). Sockets, pipes and
 * resource limits are host imports from the "ampoule_os" module, implemented by the Rust
 * runner over the same fd table it uses for wasi_snapshot_preview1, so read/write/close/poll
 * through wasi-libc work on these fds unchanged.
 *
 * Constants follow Linux numbering; the runner decodes them. Return values follow libc:
 * -1 with errno set (the import returns -errno).
 */
#ifndef AMPOULE_OS_H
#define AMPOULE_OS_H

#if defined(__wasi__) && !defined(__ASSEMBLER__)

/* The socket shims exist for ERTS's inet_drv. C++ sources (NIF libraries) skip them unless they
 * ask (-DAMPOULE_OS_NET): <sys/socket.h> brings a global struct iovec that libraries such as
 * Snappy declare themselves. */
#if defined(__cplusplus) && !defined(AMPOULE_OS_NET)
#define AMPOULE_OS_NO_NET
#endif

#include <stddef.h>
#include <stdint.h>
#include <errno.h>
#include <sys/types.h>
#ifndef AMPOULE_OS_NO_NET
/* wasi-libc's sockaddr_un has no sun_path, WASI having no unix sockets. The runtime provides them
 * (runner/src/unix.rs) with Linux's layout, defined here first so wasi-libc's header stands aside. */
#include <__typedef_sa_family_t.h>
#define __wasilibc___struct_sockaddr_un_h
struct sockaddr_un {
    sa_family_t sun_family;
    char sun_path[108];
};
#include <sys/socket.h>
#include <netinet/in.h>
#endif
#include <unistd.h>
#include <sys/ioctl.h>
#include <sys/resource.h>
#include <time.h>
#include <signal.h>

/* errno names WASI does not define, numbered above WASI's range. */
#ifndef EHOSTDOWN
#define EHOSTDOWN 1000
#endif
#ifndef ESHUTDOWN
#define ESHUTDOWN 1001
#endif
#ifndef ETOOMANYREFS
#define ETOOMANYREFS 1002
#endif
#ifndef EPFNOSUPPORT
#define EPFNOSUPPORT 1003
#endif
#ifndef ESOCKTNOSUPPORT
#define ESOCKTNOSUPPORT 1004
#endif
#ifndef EREMOTEIO
#define EREMOTEIO 1005
#endif
#ifndef EUSERS
#define EUSERS 1006
#endif
#ifndef ENOTBLK
#define ENOTBLK 1007
#endif
#ifndef EREMOTE
#define EREMOTE 1008
#endif
#ifndef ECOMM
#define ECOMM 1009
#endif
#ifndef ENONET
#define ENONET 1010
#endif
#ifndef ENOPKG
#define ENOPKG 1011
#endif
#ifndef ESTRPIPE
#define ESTRPIPE 1012
#endif
#ifndef EBADFD
#define EBADFD 1013
#endif
#ifndef ENOTUNIQ
#define ENOTUNIQ 1014
#endif
#ifndef ERESTART
#define ERESTART 1015
#endif
#ifndef EUCLEAN
#define EUCLEAN 1016
#endif
#ifndef EKEYEXPIRED
#define EKEYEXPIRED 1017
#endif
#ifndef EKEYREVOKED
#define EKEYREVOKED 1018
#endif
#ifndef EKEYREJECTED
#define EKEYREJECTED 1019
#endif
#ifndef ENOKEY
#define ENOKEY 1020
#endif
#ifndef EMEDIUMTYPE
#define EMEDIUMTYPE 1021
#endif
#ifndef ENOMEDIUM
#define ENOMEDIUM 1022
#endif
#ifndef ERFKILL
#define ERFKILL 1023
#endif
#ifndef EHWPOISON
#define EHWPOISON 1024
#endif
#ifndef ELIBBAD
#define ELIBBAD 1025
#endif
#ifndef ELIBACC
#define ELIBACC 1026
#endif
#ifndef ELIBMAX
#define ELIBMAX 1027
#endif
#ifndef ELIBSCN
#define ELIBSCN 1028
#endif
#ifndef ELIBEXEC
#define ELIBEXEC 1029
#endif
#ifndef ENOSR
#define ENOSR 1030
#endif
#ifndef ENOSTR
#define ENOSTR 1031
#endif
#ifndef ETIME
#define ETIME 1032
#endif
#ifndef ENODATA
#define ENODATA 1033
#endif
#ifndef EPROCLIM
#define EPROCLIM 1034
#endif
#ifndef EBADRPC
#define EBADRPC 1035
#endif
#ifndef ERPCMISMATCH
#define ERPCMISMATCH 1036
#endif
#ifndef EPROGUNAVAIL
#define EPROGUNAVAIL 1037
#endif
#ifndef EPROGMISMATCH
#define EPROGMISMATCH 1038
#endif
#ifndef EPROCUNAVAIL
#define EPROCUNAVAIL 1039
#endif
#ifndef EFTYPE
#define EFTYPE 1040
#endif
#ifndef EAUTH
#define EAUTH 1041
#endif
#ifndef ENEEDAUTH
#define ENEEDAUTH 1042
#endif

#define AOS_IMPORT(name) __attribute__((import_module("ampoule_os"), import_name(#name)))

#ifndef RLIMIT_NOFILE
#define RLIMIT_NOFILE 7
#endif
#ifndef RLIM_INFINITY
#define RLIM_INFINITY (~0ULL)
#endif
typedef unsigned long long rlim_t;
struct rlimit {
    rlim_t rlim_cur;
    rlim_t rlim_max;
};

#ifndef SOL_SOCKET
#define SOL_SOCKET 1
#endif
#ifndef SO_DEBUG
#define SO_DEBUG 1
#endif
#ifndef SO_REUSEADDR
#define SO_REUSEADDR 2
#endif
#ifndef SO_TYPE
#define SO_TYPE 3
#endif
#ifndef SO_ERROR
#define SO_ERROR 4
#endif
#ifndef SO_DONTROUTE
#define SO_DONTROUTE 5
#endif
#ifndef SO_BROADCAST
#define SO_BROADCAST 6
#endif
#ifndef SO_SNDBUF
#define SO_SNDBUF 7
#endif
#ifndef SO_RCVBUF
#define SO_RCVBUF 8
#endif
#ifndef SO_KEEPALIVE
#define SO_KEEPALIVE 9
#endif
#ifndef SO_OOBINLINE
#define SO_OOBINLINE 10
#endif
#ifndef SO_PRIORITY
#define SO_PRIORITY 12
#endif
#ifndef SO_LINGER
#define SO_LINGER 13
#endif
#ifndef SO_REUSEPORT
#define SO_REUSEPORT 15
#endif
#ifndef SO_RCVLOWAT
#define SO_RCVLOWAT 18
#endif
#ifndef SO_SNDLOWAT
#define SO_SNDLOWAT 19
#endif
#ifndef SO_RCVTIMEO
#define SO_RCVTIMEO 20
#endif
#ifndef SO_SNDTIMEO
#define SO_SNDTIMEO 21
#endif
#ifndef SO_ACCEPTCONN
#define SO_ACCEPTCONN 30
#endif

#ifndef IPPROTO_IP
#define IPPROTO_IP 0
#endif
#ifndef IPPROTO_TCP
#define IPPROTO_TCP 6
#endif
#ifndef IPPROTO_UDP
#define IPPROTO_UDP 17
#endif
#ifndef IPPROTO_SCTP
#define IPPROTO_SCTP 132
#endif
#ifndef IPPROTO_IPV6
#define IPPROTO_IPV6 41
#endif
#ifndef TCP_NODELAY
#define TCP_NODELAY 1
#endif
#ifndef IP_TOS
#define IP_TOS 1
#endif
#ifndef IP_TTL
#define IP_TTL 2
#endif
#ifndef IPV6_V6ONLY
#define IPV6_V6ONLY 26
#endif

#ifndef MSG_OOB
#define MSG_OOB 0x10000
#endif
#ifndef MSG_DONTROUTE
#define MSG_DONTROUTE 0x4
#endif
#ifndef MSG_DONTWAIT
#define MSG_DONTWAIT 0x40
#endif
#ifndef MSG_NOSIGNAL
#define MSG_NOSIGNAL 0x4000
#endif

#ifndef INADDR_ANY
#define INADDR_ANY ((in_addr_t)0x00000000)
#endif
#ifndef INADDR_LOOPBACK
#define INADDR_LOOPBACK ((in_addr_t)0x7f000001)
#endif
#ifndef INADDR_NONE
#define INADDR_NONE ((in_addr_t)0xffffffff)
#endif

#ifndef FIONREAD
#define FIONREAD 0x541B
#endif
#ifndef FIONBIO
#define FIONBIO 0x5421
#endif

AOS_IMPORT(socket) int32_t __aos_socket(int32_t domain, int32_t type, int32_t protocol);
AOS_IMPORT(bind) int32_t __aos_bind(int32_t fd, const void *addr, uint32_t len);
AOS_IMPORT(listen) int32_t __aos_listen(int32_t fd, int32_t backlog);
AOS_IMPORT(accept) int32_t __aos_accept(int32_t fd, void *addr, uint32_t *len);
AOS_IMPORT(connect) int32_t __aos_connect(int32_t fd, const void *addr, uint32_t len);
AOS_IMPORT(getsockopt) int32_t __aos_getsockopt(int32_t fd, int32_t level, int32_t name, void *val, uint32_t *len);
AOS_IMPORT(setsockopt) int32_t __aos_setsockopt(int32_t fd, int32_t level, int32_t name, const void *val, uint32_t len);
AOS_IMPORT(getsockname) int32_t __aos_getsockname(int32_t fd, void *addr, uint32_t *len);
AOS_IMPORT(getpeername) int32_t __aos_getpeername(int32_t fd, void *addr, uint32_t *len);
AOS_IMPORT(recvfrom) int32_t __aos_recvfrom(int32_t fd, void *buf, uint32_t len, int32_t flags, void *addr, uint32_t *addrlen);
AOS_IMPORT(sendto) int32_t __aos_sendto(int32_t fd, const void *buf, uint32_t len, int32_t flags, const void *addr, uint32_t addrlen);
AOS_IMPORT(shutdown) int32_t __aos_shutdown(int32_t fd, int32_t how);
AOS_IMPORT(pipe) int32_t __aos_pipe(int32_t *fds);
AOS_IMPORT(ioctl_int) int32_t __aos_ioctl_int(int32_t fd, int32_t request, int32_t *arg);
AOS_IMPORT(gethostname) int32_t __aos_gethostname(char *name, uint32_t len);
/* The runtime's host pid, so os:getpid/0 names a process host tools (ps, kill) can find; wasi-libc's
 * emulated getpid returns a constant. */
AOS_IMPORT(getpid) int32_t __aos_getpid(void);
AOS_IMPORT(thread_exit) __attribute__((noreturn)) void __aos_thread_exit(void);
/* The write end of ERTS's signal-dispatcher pipe: the runtime writes host signals (SIGTERM,
 * SIGINT, ...) into it as the guest's signal numbers, as a signal handler would. */
AOS_IMPORT(signal_fd) void __aos_signal_fd(int32_t fd);
/* wasi-libc's dup() is a stub returning ENOTSUP; the runtime shares the entry under a new fd. */
AOS_IMPORT(dup) int32_t __aos_dup(int32_t fd);
/* The runtime plays erl_child_setup for the spawn driver. `forker` returns the read end of the
 * pipe it writes SigChld messages to; `forker_write` takes a Start request and its fds (the
 * runtime keeps its own references, so the caller closes them as after a sendmsg). */
AOS_IMPORT(forker) int32_t __aos_forker(void);
AOS_IMPORT(forker_write) int32_t __aos_forker_write(int32_t fd, const void *buf, uint32_t len, const int32_t *fds, uint32_t nfds);
/* The permission bits of an absolute guest path, which a WASI filestat does not carry. */
AOS_IMPORT(path_mode) int32_t __aos_path_mode(const char *path, uint32_t len, int32_t follow);

static inline __attribute__((noreturn)) void aos_pthread_exit(void *res) { (void)res; __aos_thread_exit(); }
static inline int aos_pthread_sigmask(int how, const void *set, void *oset) { (void)how; (void)set; (void)oset; return 0; }
static inline int aos_sigprocmask(int how, const void *set, void *oset) { (void)how; (void)set; (void)oset; return 0; }
static inline int aos_sigwait(const void *set, int *sig) { (void)set; (void)sig; return ENOTSUP; }
static inline int aos_pthread_kill(unsigned long tid, int sig) { (void)tid; (void)sig; return 0; }

static inline int __aos_ret(int32_t r) {
    if (r < 0) {
        errno = -r;
        return -1;
    }
    return r;
}

#ifndef AMPOULE_OS_NO_NET
static inline int aos_socket(int d, int t, int p) { return __aos_ret(__aos_socket(d, t, p)); }
const void *aos_unix_abs(const void *addr, unsigned *len, void *buf);
static inline int aos_bind(int fd, const struct sockaddr *a, socklen_t l) { struct sockaddr_un u; a = (const struct sockaddr *)aos_unix_abs(a, (unsigned *)&l, &u); return __aos_ret(__aos_bind(fd, a, l)); }
static inline int aos_listen(int fd, int b) { return __aos_ret(__aos_listen(fd, b)); }
static inline int aos_accept(int fd, struct sockaddr *a, socklen_t *l) { return __aos_ret(__aos_accept(fd, a, (uint32_t *)l)); }
static inline int aos_connect(int fd, const struct sockaddr *a, socklen_t l) { struct sockaddr_un u; a = (const struct sockaddr *)aos_unix_abs(a, (unsigned *)&l, &u); return __aos_ret(__aos_connect(fd, a, l)); }
static inline int aos_getsockopt(int fd, int lv, int n, void *v, socklen_t *l) { return __aos_ret(__aos_getsockopt(fd, lv, n, v, (uint32_t *)l)); }
static inline int aos_setsockopt(int fd, int lv, int n, const void *v, socklen_t l) { return __aos_ret(__aos_setsockopt(fd, lv, n, v, l)); }
static inline int aos_getsockname(int fd, struct sockaddr *a, socklen_t *l) { return __aos_ret(__aos_getsockname(fd, a, (uint32_t *)l)); }
static inline int aos_getpeername(int fd, struct sockaddr *a, socklen_t *l) { return __aos_ret(__aos_getpeername(fd, a, (uint32_t *)l)); }
static inline ssize_t aos_recvfrom(int fd, void *b, size_t n, int f, struct sockaddr *a, socklen_t *l) { return __aos_ret(__aos_recvfrom(fd, b, n, f, a, (uint32_t *)l)); }
static inline ssize_t aos_sendto(int fd, const void *b, size_t n, int f, const struct sockaddr *a, socklen_t l) { struct sockaddr_un u; a = (const struct sockaddr *)aos_unix_abs(a, (unsigned *)&l, &u); return __aos_ret(__aos_sendto(fd, b, n, f, a, l)); }
static inline ssize_t aos_recv(int fd, void *b, size_t n, int f) { return aos_recvfrom(fd, b, n, f, NULL, NULL); }
static inline ssize_t aos_send(int fd, const void *b, size_t n, int f) { return aos_sendto(fd, b, n, f, NULL, 0); }
#endif
static inline int aos_shutdown(int fd, int how) { return __aos_ret(__aos_shutdown(fd, how)); }
static inline int aos_pipe(int fds[2]) { return __aos_ret(__aos_pipe(fds)); }
static inline int aos_gethostname(char *n, size_t l) { return __aos_ret(__aos_gethostname(n, l)); }
static inline pid_t aos_getpid(void) { return (pid_t)__aos_getpid(); }
static inline int aos_dup(int fd) { return __aos_ret(__aos_dup(fd)); }
static inline int aos_forker(void) { return __aos_ret(__aos_forker()); }
static inline ssize_t aos_forker_write(int fd, const void *b, size_t n, const int *fds, int nfds) {
    return __aos_ret(__aos_forker_write(fd, b, n, (const int32_t *)fds, nfds));
}
unsigned aos_path_mode(const char *path, int follow);
/* WASI has no chmod; these set the mode on the host file (src/aos.c). */
int aos_chmod(const char *path, mode_t mode);
int aos_fchmod(int fd, mode_t mode);
static inline int aos_getrlimit(int res, struct rlimit *r) {
    if (res != RLIMIT_NOFILE) { errno = EINVAL; return -1; }
    r->rlim_cur = r->rlim_max = 4096;
    return 0;
}
static inline void aos_tzset(void) {}
static inline int aos_getrusage(int who, struct rusage *r) { __builtin_memset(r, 0, sizeof(*r)); return getrusage(who, r); }
static inline int aos_setrlimit(int res, const struct rlimit *r) { (void)res; (void)r; return 0; }

/* Signals: the emulated signal() delivers only raise(); masks and handlers are no-ops. */
#ifndef SIG_BLOCK
#define SIG_BLOCK 0
#define SIG_UNBLOCK 1
#define SIG_SETMASK 2
#endif
#ifndef SA_RESTART
#define SA_NOCLDSTOP 1
#define SA_NOCLDWAIT 2
#define SA_SIGINFO 4
#define SA_ONSTACK 0x08000000
#define SA_RESTART 0x10000000
#define SA_NODEFER 0x40000000
#define SA_RESETHAND 0x80000000
#endif
struct aos_sigaction {
    void (*sa_handler)(int);
    sigset_t sa_mask;
    int sa_flags;
};
static inline int aos_sigaction(int sig, const struct aos_sigaction *act, struct aos_sigaction *old) {
    (void)sig; (void)act;
    if (old) { old->sa_handler = 0; old->sa_mask = 0; old->sa_flags = 0; }
    return 0;
}
static inline int aos_sigemptyset(sigset_t *s) { *s = 0; return 0; }
static inline int aos_sigfillset(sigset_t *s) { *s = (sigset_t)~0; return 0; }
static inline int aos_sigaddset(sigset_t *s, int n) { (void)s; (void)n; return 0; }
static inline int aos_sigdelset(sigset_t *s, int n) { (void)s; (void)n; return 0; }
static inline int aos_sigismember(const sigset_t *s, int n) { (void)s; (void)n; return 0; }

/* Processes and identity: one process, no users, no children. */
static inline int aos_nice(int inc) { (void)inc; return 0; }
static inline unsigned aos_alarm(unsigned s) { (void)s; return 0; }
static inline int aos_kill(pid_t p, int sig) { (void)p; (void)sig; errno = ENOSYS; return -1; }
struct utsname;
int aos_uname(struct utsname *u);
static inline char *aos_ttyname(int fd) { (void)fd; errno = ENOTTY; return NULL; }
static inline uid_t aos_getuid(void) { return 0; }
static inline uid_t aos_geteuid(void) { return 0; }
static inline gid_t aos_getgid(void) { return 0; }
static inline gid_t aos_getegid(void) { return 0; }
static inline int aos_chown(const char *p, uid_t u, gid_t g) { (void)p; (void)u; (void)g; return 0; }
static inline int aos_fchown(int fd, uid_t u, gid_t g) { (void)fd; (void)u; (void)g; return 0; }
static inline pid_t aos_fork(void) { errno = ENOSYS; return -1; }
static inline pid_t aos_setsid(void) { errno = ENOSYS; return -1; }
static inline int aos_execv(const char *p, char *const a[]) { (void)p; (void)a; errno = ENOSYS; return -1; }
static inline int aos_execve(const char *p, char *const a[], char *const e[]) { (void)p; (void)a; (void)e; errno = ENOSYS; return -1; }
static inline int aos_socketpair(int d, int t, int p, int fds[2]) { (void)d; (void)t; (void)p; (void)fds; errno = ENOTSUP; return -1; }

#ifndef AMPOULE_OS_NO_NET
/* fd passing over unix sockets (only the disabled forker uses it). */
#ifndef SCM_RIGHTS
#define SCM_RIGHTS 1
#endif
#ifndef MSG_CTRUNC
#define MSG_CTRUNC 0x8
#endif
struct cmsghdr {
    socklen_t cmsg_len;
    int cmsg_level;
    int cmsg_type;
};
#define CMSG_ALIGN(len) (((len) + sizeof(size_t) - 1) & (size_t) ~(sizeof(size_t) - 1))
#define CMSG_DATA(c) ((unsigned char *)(((struct cmsghdr *)(c)) + 1))
#define CMSG_SPACE(len) (CMSG_ALIGN(len) + CMSG_ALIGN(sizeof(struct cmsghdr)))
#define CMSG_LEN(len) (CMSG_ALIGN(sizeof(struct cmsghdr)) + (len))
#define CMSG_FIRSTHDR(m) ((size_t)(m)->msg_controllen >= sizeof(struct cmsghdr) ? (struct cmsghdr *)(m)->msg_control : (struct cmsghdr *)0)
#define CMSG_NXTHDR(m, c) ((struct cmsghdr *)0)
static inline ssize_t aos_recvmsg(int fd, struct msghdr *m, int f) { (void)fd; (void)m; (void)f; errno = ENOTSUP; return -1; }
static inline ssize_t aos_sendmsg(int fd, const struct msghdr *m, int f) { (void)fd; (void)m; (void)f; errno = ENOTSUP; return -1; }
#endif

#ifndef AMPOULE_OS_NO_NET
#define socket aos_socket
#define bind aos_bind
#define listen aos_listen
#define accept aos_accept
#define connect aos_connect
#define getsockopt aos_getsockopt
#define setsockopt aos_setsockopt
#define getsockname aos_getsockname
#define getpeername aos_getpeername
#define recvfrom aos_recvfrom
#define sendto aos_sendto
#define recv aos_recv
#define send aos_send
#define shutdown aos_shutdown
#endif
#define pipe aos_pipe
#define gethostname aos_gethostname
#define getpid aos_getpid
#define dup(fd) aos_dup(fd)
#define pthread_exit aos_pthread_exit
#define pthread_sigmask aos_pthread_sigmask
#define sigprocmask aos_sigprocmask
#define sigwait aos_sigwait
#define pthread_kill(t, s) aos_pthread_kill((unsigned long)(t), (s))
#define tzset aos_tzset
#define getrusage aos_getrusage
#define getrlimit aos_getrlimit
#define setrlimit aos_setrlimit
#define sigaction aos_sigaction
#define sigemptyset aos_sigemptyset
#define sigfillset aos_sigfillset
#define sigaddset aos_sigaddset
#define sigdelset aos_sigdelset
#define sigismember aos_sigismember
#define nice aos_nice
#define alarm aos_alarm
#define kill aos_kill
#define uname aos_uname
#define ttyname aos_ttyname
#define getuid aos_getuid
#define geteuid aos_geteuid
#define getgid aos_getgid
#define getegid aos_getegid
#define chmod(p, m) aos_chmod(p, m)
#define fchmod(fd, m) aos_fchmod(fd, m)
#define chown aos_chown
#define fchown aos_fchown
#define fork aos_fork
#define setsid aos_setsid
#define execv aos_execv
#define execve aos_execve
#define execle(...) (errno = ENOSYS, -1)
#define execvp(...) (errno = ENOSYS, -1)
#define execl(...) (errno = ENOSYS, -1)
#ifndef AMPOULE_OS_NO_NET
#define socketpair aos_socketpair
#define recvmsg aos_recvmsg
#define sendmsg aos_sendmsg
#endif

#endif /* __wasi__ */
#endif /* AMPOULE_OS_H */
