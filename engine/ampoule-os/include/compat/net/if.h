/* The guest has no interface list of its own; the runner's sockets bind host addresses. */
#ifndef AMPOULE_OS_NET_IF_H
#define AMPOULE_OS_NET_IF_H

#include <stddef.h>
#include <sys/socket.h>

#define IF_NAMESIZE 16
#define IFNAMSIZ IF_NAMESIZE

struct if_nameindex {
    unsigned int if_index;
    char *if_name;
};

static inline unsigned int if_nametoindex(const char *n) { (void)n; return 0; }
static inline char *if_indextoname(unsigned int i, char *n) { (void)i; (void)n; return NULL; }
static inline struct if_nameindex *if_nameindex(void) { return NULL; }
static inline void if_freenameindex(struct if_nameindex *p) { (void)p; }

#endif
