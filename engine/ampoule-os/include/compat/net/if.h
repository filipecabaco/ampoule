/* The runner's sockets are host sockets, so the interfaces are the host's: `getifaddrs`,
 * `if_nametoindex` and `if_indextoname` ask the runner (src/aos.c, runner/src/ifaddrs.rs). */
#ifndef AMPOULE_OS_NET_IF_H
#define AMPOULE_OS_NET_IF_H

#include <stddef.h>
#include <sys/socket.h>

#define IF_NAMESIZE 16
#define IFNAMSIZ IF_NAMESIZE

/* Linux's numbering, which the runner translates the host's into. */
#define IFF_UP 0x1
#define IFF_BROADCAST 0x2
#define IFF_LOOPBACK 0x8
#define IFF_POINTOPOINT 0x10
#define IFF_RUNNING 0x40
#define IFF_MULTICAST 0x1000

struct if_nameindex {
    unsigned int if_index;
    char *if_name;
};

unsigned int if_nametoindex(const char *name);
char *if_indextoname(unsigned int index, char *name);
static inline struct if_nameindex *if_nameindex(void) { return NULL; }
static inline void if_freenameindex(struct if_nameindex *p) { (void)p; }

#endif
