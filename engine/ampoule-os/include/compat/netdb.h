/* Name lookups are left to OTP's pure-Erlang resolver (inetrc {lookup, [file, dns]}),
 * so every libc lookup here reports "not found". */
#ifndef AMPOULE_OS_NETDB_H
#define AMPOULE_OS_NETDB_H

#include <stddef.h>
#include <errno.h>
#include <sys/socket.h>
#include <netinet/in.h>

struct hostent {
    char *h_name;
    char **h_aliases;
    int h_addrtype;
    int h_length;
    char **h_addr_list;
};
#define h_addr h_addr_list[0]

struct servent {
    char *s_name;
    char **s_aliases;
    int s_port;
    char *s_proto;
};

struct protoent {
    char *p_name;
    char **p_aliases;
    int p_proto;
};

struct addrinfo {
    int ai_flags;
    int ai_family;
    int ai_socktype;
    int ai_protocol;
    socklen_t ai_addrlen;
    struct sockaddr *ai_addr;
    char *ai_canonname;
    struct addrinfo *ai_next;
};

#define AI_PASSIVE 0x01
#define AI_CANONNAME 0x02
#define AI_NUMERICHOST 0x04
#define AI_V4MAPPED 0x08
#define AI_ALL 0x10
#define AI_ADDRCONFIG 0x20
#define AI_NUMERICSERV 0x400

#define NI_NUMERICHOST 0x01
#define NI_NUMERICSERV 0x02
#define NI_NOFQDN 0x04
#define NI_NAMEREQD 0x08
#define NI_DGRAM 0x10
#define NI_MAXHOST 255
#define NI_MAXSERV 32

#define EAI_BADFLAGS -1
#define EAI_NONAME -2
#define EAI_AGAIN -3
#define EAI_FAIL -4
#define EAI_FAMILY -6
#define EAI_SOCKTYPE -7
#define EAI_SERVICE -8
#define EAI_MEMORY -10
#define EAI_SYSTEM -11
#define EAI_OVERFLOW -12

#define HOST_NOT_FOUND 1
#define TRY_AGAIN 2
#define NO_RECOVERY 3
#define NO_DATA 4
#define NO_ADDRESS NO_DATA

static int h_errno = HOST_NOT_FOUND;

static inline struct hostent *gethostbyname(const char *n) { (void)n; return NULL; }
static inline struct hostent *gethostbyname2(const char *n, int af) { (void)n; (void)af; return NULL; }
static inline struct hostent *gethostbyaddr(const void *a, socklen_t l, int t) { (void)a; (void)l; (void)t; return NULL; }
static inline struct servent *getservbyname(const char *n, const char *p) { (void)n; (void)p; return NULL; }
static inline struct servent *getservbyport(int port, const char *p) { (void)port; (void)p; return NULL; }
static inline struct protoent *getprotobyname(const char *n) { (void)n; return NULL; }
static inline int getaddrinfo(const char *n, const char *s, const struct addrinfo *h, struct addrinfo **r) {
    (void)n; (void)s; (void)h; *r = NULL; return EAI_FAIL;
}
static inline void freeaddrinfo(struct addrinfo *r) { (void)r; }
static inline const char *gai_strerror(int e) { (void)e; return "name lookup not available"; }
static inline int getnameinfo(const struct sockaddr *a, socklen_t al, char *h, socklen_t hl, char *s, socklen_t sl, int f) {
    (void)a; (void)al; (void)h; (void)hl; (void)s; (void)sl; (void)f; return EAI_FAIL;
}
static inline const char *hstrerror(int e) { (void)e; return "host lookup not available"; }

#endif
