/* Symbols wasi-libc lacks entirely, kept out of ampoule_os.h so configure's own
 * function probes still see clean declarations. */
#include <pthread.h>

int pthread_attr_setscope(pthread_attr_t *attr, int scope)
{
    (void)attr;
    (void)scope;
    return 0;
}

#include <string.h>
#include <sys/utsname.h>

/* Report Linux: OTP takes its Linux paths for the OS-specific pieces the runtime provides
 * (CA certificates from /etc/ssl/certs, /proc for os_mon), where an unknown OS gets enotsup. */
int aos_uname(struct utsname *u)
{
    int r = uname(u);
    if (r == 0) {
        strcpy(u->sysname, "Linux");
        strcpy(u->release, "6.1.0-ampoule");
        strcpy(u->machine, "wasm32");
    }
    return r;
}

#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <sys/types.h>
#include <unistd.h>

__attribute__((import_module("ampoule_os"), import_name("path_mode")))
int32_t __aos_path_mode(const char *path, uint32_t len, int32_t follow);
__attribute__((import_module("ampoule_os"), import_name("chmod")))
int32_t __aos_chmod(const char *path, uint32_t len, uint32_t mode);
__attribute__((import_module("ampoule_os"), import_name("fchmod")))
int32_t __aos_fchmod(int32_t fd, uint32_t mode);

/* `path` as an absolute guest path: the runtime resolves from the root, and the cwd lives in
 * wasi-libc. NULL (errno set) when it does not fit. */
static const char *aos_abs(const char *path, char buf[PATH_MAX])
{
    if (path[0] == '/')
        return path;
    size_t n, len = strlen(path);
    if (getcwd(buf, PATH_MAX) == NULL)
        return NULL;
    n = strlen(buf);
    if (n + 1 + len + 1 > PATH_MAX) {
        errno = ENAMETOOLONG;
        return NULL;
    }
    buf[n] = '/';
    memcpy(buf + n + 1, path, len + 1);
    return buf;
}

/* Permission bits for a path ERTS stats. 0 when the runtime cannot tell, which leaves the mode as
 * wasi-libc reported it. */
unsigned aos_path_mode(const char *path, int follow)
{
    char buf[PATH_MAX];
    const char *abs = aos_abs(path, buf);
    if (abs == NULL)
        return 0;
    int32_t r = __aos_path_mode(abs, strlen(abs), follow);
    return r < 0 ? 0 : (unsigned)r;
}

/* chmod and fchmod, which WASI lacks; the runtime sets them on the host file. */
int aos_chmod(const char *path, mode_t mode)
{
    char buf[PATH_MAX];
    const char *abs = aos_abs(path, buf);
    if (abs == NULL)
        return -1;
    int32_t r = __aos_chmod(abs, strlen(abs), mode);
    if (r < 0) {
        errno = -r;
        return -1;
    }
    return 0;
}

int aos_fchmod(int fd, mode_t mode)
{
    int32_t r = __aos_fchmod(fd, mode);
    if (r < 0) {
        errno = -r;
        return -1;
    }
    return 0;
}

/* A sockaddr_un with Linux's layout, as ampoule_os.h defines it (not visible from this file). */
struct aos_sun {
    unsigned short family;
    char path[108];
};

/* A unix socket address with a relative path, made absolute against wasi-libc's emulated cwd
 * into `buf`: the runtime resolves guest paths from the root. Anything else is returned as is. */
const void *aos_unix_abs(const void *addr, unsigned *len, void *buf)
{
    const struct aos_sun *a = addr;
    struct aos_sun *out = buf;
    if (addr == NULL || *len <= 2 || a->family != 3 /* AF_UNIX */ || a->path[0] == '/' || a->path[0] == '\0')
        return addr;
    size_t plen = strnlen(a->path, *len - 2 < sizeof(a->path) ? *len - 2 : sizeof(a->path));
    char cwd[PATH_MAX];
    if (getcwd(cwd, sizeof(cwd)) == NULL)
        return addr;
    size_t n = strlen(cwd);
    if (n + 1 + plen + 1 > sizeof(out->path))
        return addr;
    out->family = a->family;
    memcpy(out->path, cwd, n);
    out->path[n] = '/';
    memcpy(out->path + n + 1, a->path, plen);
    out->path[n + 1 + plen] = '\0';
    *len = (unsigned)(2 + n + 1 + plen + 1);
    return out;
}
