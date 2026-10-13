/* The runner owns the terminal; the guest never sees a tty. */
#ifndef AMPOULE_OS_TERMIOS_H
#define AMPOULE_OS_TERMIOS_H

#include <errno.h>

typedef unsigned char cc_t;
typedef unsigned int speed_t;
typedef unsigned int tcflag_t;

#define NCCS 32

struct termios {
    tcflag_t c_iflag;
    tcflag_t c_oflag;
    tcflag_t c_cflag;
    tcflag_t c_lflag;
    cc_t c_line;
    cc_t c_cc[NCCS];
    speed_t __c_ispeed;
    speed_t __c_ospeed;
};

#define VINTR 0
#define VQUIT 1
#define VERASE 2
#define VKILL 3
#define VEOF 4
#define VTIME 5
#define VMIN 6

#define ICRNL 0000400
#define IXON 0002000
#define OPOST 0000001
#define ONLCR 0000004
#define ISIG 0000001
#define ICANON 0000002
#define ECHO 0000010
#define ECHOE 0000020
#define ECHOK 0000040
#define ECHONL 0000100
#define IEXTEN 0100000

#define TCSANOW 0
#define TCSADRAIN 1
#define TCSAFLUSH 2

static inline int tcgetattr(int fd, struct termios *t) { (void)fd; (void)t; errno = ENOTTY; return -1; }
static inline int tcsetattr(int fd, int a, const struct termios *t) { (void)fd; (void)a; (void)t; errno = ENOTTY; return -1; }
static inline speed_t cfgetospeed(const struct termios *t) { (void)t; return 0; }
static inline speed_t cfgetispeed(const struct termios *t) { (void)t; return 0; }

#endif
