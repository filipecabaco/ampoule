/* struct rusage with the Linux fields. WASI tracks only CPU time: getrusage (wrapped in
 * ampoule_os.h) fills ru_utime/ru_stime and leaves the counters at zero. */
#ifndef __wasilibc___struct_rusage_h
#define __wasilibc___struct_rusage_h

#include <__struct_timeval.h>

struct rusage {
    struct timeval ru_utime;
    struct timeval ru_stime;
    long ru_maxrss;
    long ru_ixrss;
    long ru_idrss;
    long ru_isrss;
    long ru_minflt;
    long ru_majflt;
    long ru_nswap;
    long ru_inblock;
    long ru_oublock;
    long ru_msgsnd;
    long ru_msgrcv;
    long ru_nsignals;
    long ru_nvcsw;
    long ru_nivcsw;
};

#endif
