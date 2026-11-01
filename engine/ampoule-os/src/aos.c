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

/* Memory segments for ERTS's carriers (erl_mmap.c's os_mmap/os_munmap).
 *
 * wasm linear memory only grows, and wasi-libc's malloc keeps whatever it frees: a spike stayed
 * resident on the host until the VM stopped. Carriers come from here instead: whole wasm pages,
 * tracked in bitmaps over the 4 GiB address space. `owned` marks the pages this allocator grew the
 * memory by, `used` the ones in a live segment, `warm` free pages still resident on the host.
 *
 * Freeing a range keeps it warm while the warm pages stay under AOS_WARM_MAX, for the next
 * segments to reuse without faulting fresh pages in (what ERTS's own segment cache does natively,
 * but shared by every allocator instance, and never in the way of an allocation). Past that, the
 * runtime discards the pages on the host: the RSS drops, and they read as zero when next touched.
 * Either way the pages are free for any later segment, so the memory stops growing once it has
 * covered the largest working set. malloc can't reuse these pages, nor this allocator malloc's. */

#define AOS_PAGE ((size_t)65536)
#define AOS_NPAGES ((size_t)65536) /* 4 GiB of 64 KiB pages */
#define AOS_WORDS (AOS_NPAGES / 64)
#define AOS_SPACE ((uint64_t)AOS_NPAGES * AOS_PAGE)
#define AOS_WARM_MAX ((size_t)(32 << 20) / AOS_PAGE)

__attribute__((import_module("ampoule_os"), import_name("discard")))
int32_t __aos_discard(uint32_t addr, uint32_t len);

static uint64_t aos_owned[AOS_WORDS], aos_used[AOS_WORDS], aos_warm[AOS_WORDS];
static size_t aos_warm_pages;
static pthread_mutex_t aos_pages_lock = PTHREAD_MUTEX_INITIALIZER;

static int aos_bit(const uint64_t *map, size_t p) { return (map[p / 64] >> (p % 64)) & 1; }

/* Pages `p` can go to a new segment from: any free page, or (`warm`) only resident ones. */
static uint64_t aos_avail(size_t w, int warm) { return warm ? aos_warm[w] : aos_owned[w] & ~aos_used[w]; }

static void aos_mark(uint64_t *map, size_t p, size_t n, int set)
{
    for (size_t i = p; i < p + n; i++) {
        if (set)
            map[i / 64] |= (uint64_t)1 << (i % 64);
        else
            map[i / 64] &= ~((uint64_t)1 << (i % 64));
    }
}

/* The first run of `n` available pages starting at a multiple of `align` pages, below `end`; or -1. */
static long aos_find(size_t n, size_t align, size_t end, int warm)
{
    size_t p = 0;
    while (p + n <= end) {
        /* Skip whole words without an available page. */
        if (p % 64 == 0 && aos_avail(p / 64, warm) == 0) {
            p = (p + 64 + align - 1) / align * align;
            continue;
        }
        size_t i = p;
        while (i < p + n && (aos_avail(i / 64, warm) >> (i % 64) & 1))
            i++;
        if (i == p + n)
            return (long)p;
        p = (i + 1 + align - 1) / align * align;
    }
    return -1;
}

static void *aos_take(size_t p, size_t n)
{
    for (size_t i = p; i < p + n; i++) {
        if (aos_bit(aos_warm, i)) {
            aos_warm[i / 64] &= ~((uint64_t)1 << (i % 64));
            aos_warm_pages--;
        }
    }
    aos_mark(aos_used, p, n, 1);
    return (void *)(p * AOS_PAGE);
}

void *aos_pages_alloc(size_t size, size_t align)
{
    if (size == 0 || align == 0 || (align & (align - 1)) != 0 || (uint64_t)size + AOS_PAGE > AOS_SPACE) {
        errno = EINVAL;
        return NULL;
    }
    size_t n = (size + AOS_PAGE - 1) / AOS_PAGE;
    size_t a = align < AOS_PAGE ? 1 : align / AOS_PAGE;
    void *res = NULL;
    pthread_mutex_lock(&aos_pages_lock);
    for (;;) {
        size_t cur = __builtin_wasm_memory_size(0);
        long p = n <= aos_warm_pages ? aos_find(n, a, cur, 1) : -1;
        if (p < 0)
            p = aos_find(n, a, cur, 0);
        if (p >= 0) {
            res = aos_take((size_t)p, n);
            break;
        }
        /* Grow: extend a free run that ends where the memory does, aligned. */
        size_t start = cur;
        while (start > 0 && aos_bit(aos_owned, start - 1) && !aos_bit(aos_used, start - 1))
            start--;
        start = (start + a - 1) / a * a;
        size_t want = start + n - cur;
        size_t old = __builtin_wasm_memory_grow(0, want);
        if (old == (size_t)-1) {
            errno = ENOMEM;
            break;
        }
        /* malloc may have grown the memory since `cur` was read: the new pages are ours anyway,
         * and the search runs again. */
        aos_mark(aos_owned, old, want, 1);
        if (old == cur) {
            res = aos_take(start, n);
            break;
        }
    }
    pthread_mutex_unlock(&aos_pages_lock);
    return res;
}

int aos_pages_free(void *addr, size_t size)
{
    size_t a = (size_t)addr;
    if (a % AOS_PAGE != 0 || size == 0 || (uint64_t)a + size > AOS_SPACE) {
        errno = EINVAL;
        return -1;
    }
    size_t p = a / AOS_PAGE, n = (size_t)(((uint64_t)size + AOS_PAGE - 1) / AOS_PAGE);
    pthread_mutex_lock(&aos_pages_lock);
    for (size_t i = p; i < p + n; i++) {
        if (!aos_bit(aos_owned, i) || !aos_bit(aos_used, i)) {
            pthread_mutex_unlock(&aos_pages_lock);
            errno = EINVAL;
            return -1;
        }
    }
    if (aos_warm_pages + n <= AOS_WARM_MAX) {
        aos_mark(aos_warm, p, n, 1);
        aos_warm_pages += n;
        aos_mark(aos_used, p, n, 0);
        pthread_mutex_unlock(&aos_pages_lock);
        return 0;
    }
    pthread_mutex_unlock(&aos_pages_lock);
    /* Discarded while still marked used, so no new segment can be handed these pages first. */
    int32_t r = __aos_discard((uint32_t)a, (uint32_t)(n * AOS_PAGE));
    pthread_mutex_lock(&aos_pages_lock);
    aos_mark(aos_used, p, n, 0);
    pthread_mutex_unlock(&aos_pages_lock);
    if (r < 0) {
        /* The pages are free for reuse even when the host could not drop them. */
        errno = -r;
        return -1;
    }
    return 0;
}

/* The host's interfaces (the runner's sockets are host sockets): records from the runner, in the
 * layout runner/src/ifaddrs.rs describes, made into wasi-libc's struct ifaddrs. Each node is one
 * allocation holding its name and addresses, so freeifaddrs frees node by node. */
#include <ifaddrs.h>
#include <netinet/in.h>
#include <stdlib.h>

__attribute__((import_module("ampoule_os"), import_name("getifaddrs")))
int32_t __aos_getifaddrs(uint8_t *buf, uint32_t len);
__attribute__((import_module("ampoule_os"), import_name("if_nametoindex")))
int32_t __aos_if_nametoindex(const char *name, uint32_t len);
__attribute__((import_module("ampoule_os"), import_name("if_indextoname")))
int32_t __aos_if_indextoname(uint32_t index, char *name, uint32_t len);

struct aos_ifa {
    struct ifaddrs ifa;
    union {
        struct sockaddr_in v4[3];
        struct sockaddr_in6 v6[2];
    } sa;
    char name[];
};

static uint32_t le32(const uint8_t *p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }

int getifaddrs(struct ifaddrs **out)
{
    uint8_t *buf = NULL;
    int32_t need = __aos_getifaddrs(NULL, 0);
    /* Asked again while it grows (an interface came up between the calls). */
    for (;;) {
        if (need < 0) {
            free(buf);
            errno = -need;
            return -1;
        }
        uint8_t *bigger = realloc(buf, need ? need : 1);
        if (!bigger) {
            free(buf);
            errno = ENOMEM;
            return -1;
        }
        buf = bigger;
        int32_t got = __aos_getifaddrs(buf, need);
        if (got <= need) {
            need = got;
            break;
        }
        need = got;
    }

    struct ifaddrs *head = NULL, **tail = &head;
    for (int32_t at = 0; at + 7 <= need;) {
        uint32_t n = buf[at] | buf[at + 1] << 8;
        const uint8_t *p = buf + at + 2;
        uint32_t flags = le32(p + n);
        uint8_t family = p[n + 4];
        p += n + 5;
        struct aos_ifa *node = calloc(1, sizeof(struct aos_ifa) + n + 1);
        if (!node) {
            freeifaddrs(head);
            free(buf);
            errno = ENOMEM;
            return -1;
        }
        memcpy(node->name, buf + at + 2, n);
        node->ifa.ifa_name = node->name;
        node->ifa.ifa_flags = flags;
        if (family == 1) {
            for (int i = 0; i < 3; i++)
                node->sa.v4[i].sin_family = AF_INET;
            memcpy(&node->sa.v4[0].sin_addr, p, 4);
            memcpy(&node->sa.v4[1].sin_addr, p + 4, 4);
            node->ifa.ifa_addr = (struct sockaddr *)&node->sa.v4[0];
            node->ifa.ifa_netmask = (struct sockaddr *)&node->sa.v4[1];
            if (p[8]) {
                memcpy(&node->sa.v4[2].sin_addr, p + 9, 4);
                node->ifa.ifa_broadaddr = (struct sockaddr *)&node->sa.v4[2];
            }
            p += 13;
        } else if (family == 2) {
            for (int i = 0; i < 2; i++)
                node->sa.v6[i].sin6_family = AF_INET6;
            memcpy(&node->sa.v6[0].sin6_addr, p, 16);
            memcpy(&node->sa.v6[1].sin6_addr, p + 16, 16);
            node->sa.v6[0].sin6_scope_id = le32(p + 32);
            node->ifa.ifa_addr = (struct sockaddr *)&node->sa.v6[0];
            node->ifa.ifa_netmask = (struct sockaddr *)&node->sa.v6[1];
            p += 36;
        }
        *tail = &node->ifa;
        tail = &node->ifa.ifa_next;
        at = p - buf;
    }
    free(buf);
    *out = head;
    return 0;
}

void freeifaddrs(struct ifaddrs *ifa)
{
    while (ifa) {
        struct ifaddrs *next = ifa->ifa_next;
        free(ifa); /* the struct ifaddrs is the first member of its aos_ifa */
        ifa = next;
    }
}

unsigned int if_nametoindex(const char *name)
{
    int32_t r = __aos_if_nametoindex(name, strlen(name));
    return r > 0 ? (unsigned int)r : 0;
}

char *if_indextoname(unsigned int index, char *name)
{
    /* IF_NAMESIZE (compat/net/if.h, not on this file's include path). */
    int32_t r = __aos_if_indextoname(index, name, 16);
    if (r < 0) {
        errno = -r;
        return NULL;
    }
    return name;
}
