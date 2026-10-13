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
