//! WASI errno codes (what wasi-libc's errno holds) and the mapping from host errno.

pub const SUCCESS: i32 = 0;
pub const ACCES: i32 = 2;
pub const ADDRINUSE: i32 = 3;
pub const ADDRNOTAVAIL: i32 = 4;
pub const AFNOSUPPORT: i32 = 5;
pub const AGAIN: i32 = 6;
pub const ALREADY: i32 = 7;
pub const BADF: i32 = 8;
pub const BUSY: i32 = 10;
pub const CONNABORTED: i32 = 13;
pub const CONNREFUSED: i32 = 14;
pub const CONNRESET: i32 = 15;
pub const DESTADDRREQ: i32 = 17;
pub const DQUOT: i32 = 19;
pub const EXIST: i32 = 20;
pub const FAULT: i32 = 21;
pub const FBIG: i32 = 22;
pub const HOSTUNREACH: i32 = 23;
pub const INPROGRESS: i32 = 26;
pub const INTR: i32 = 27;
pub const INVAL: i32 = 28;
pub const IO: i32 = 29;
pub const ISCONN: i32 = 30;
pub const ISDIR: i32 = 31;
pub const LOOP: i32 = 32;
pub const MFILE: i32 = 33;
pub const MLINK: i32 = 34;
pub const MSGSIZE: i32 = 35;
pub const NAMETOOLONG: i32 = 37;
pub const NETDOWN: i32 = 38;
pub const NETRESET: i32 = 39;
pub const NETUNREACH: i32 = 40;
pub const NFILE: i32 = 41;
pub const NOBUFS: i32 = 42;
pub const NOENT: i32 = 44;
pub const NOMEM: i32 = 48;
pub const NOPROTOOPT: i32 = 50;
pub const NOSPC: i32 = 51;
pub const NOSYS: i32 = 52;
pub const NOTCONN: i32 = 53;
pub const NOTDIR: i32 = 54;
pub const NOTEMPTY: i32 = 55;
pub const NOTSOCK: i32 = 57;
pub const NOTSUP: i32 = 58;
pub const NOTTY: i32 = 59;
pub const NXIO: i32 = 60;
pub const OVERFLOW: i32 = 61;
pub const PERM: i32 = 63;
pub const PIPE: i32 = 64;
pub const PROTONOSUPPORT: i32 = 66;
pub const PROTOTYPE: i32 = 67;
pub const RANGE: i32 = 68;
pub const ROFS: i32 = 69;
pub const SPIPE: i32 = 70;
pub const TIMEDOUT: i32 = 73;
pub const TXTBSY: i32 = 74;
pub const XDEV: i32 = 75;

/// Translate a host errno into the WASI code the guest expects.
pub fn from_host(e: i32) -> i32 {
    match e {
        0 => SUCCESS,
        libc::EACCES => ACCES,
        libc::EADDRINUSE => ADDRINUSE,
        libc::EADDRNOTAVAIL => ADDRNOTAVAIL,
        libc::EAFNOSUPPORT => AFNOSUPPORT,
        libc::EAGAIN => AGAIN,
        libc::EALREADY => ALREADY,
        libc::EBADF => BADF,
        libc::EBUSY => BUSY,
        libc::ECONNABORTED => CONNABORTED,
        libc::ECONNREFUSED => CONNREFUSED,
        libc::ECONNRESET => CONNRESET,
        libc::EDESTADDRREQ => DESTADDRREQ,
        libc::EDQUOT => DQUOT,
        libc::EEXIST => EXIST,
        libc::EFAULT => FAULT,
        libc::EFBIG => FBIG,
        libc::EHOSTUNREACH => HOSTUNREACH,
        libc::EINPROGRESS => INPROGRESS,
        libc::EINTR => INTR,
        libc::EINVAL => INVAL,
        libc::EIO => IO,
        libc::EISCONN => ISCONN,
        libc::EISDIR => ISDIR,
        libc::ELOOP => LOOP,
        libc::EMFILE => MFILE,
        libc::EMLINK => MLINK,
        libc::EMSGSIZE => MSGSIZE,
        libc::ENAMETOOLONG => NAMETOOLONG,
        libc::ENETDOWN => NETDOWN,
        libc::ENETRESET => NETRESET,
        libc::ENETUNREACH => NETUNREACH,
        libc::ENFILE => NFILE,
        libc::ENOBUFS => NOBUFS,
        libc::ENOENT => NOENT,
        libc::ENOMEM => NOMEM,
        libc::ENOPROTOOPT => NOPROTOOPT,
        libc::ENOSPC => NOSPC,
        libc::ENOSYS => NOSYS,
        libc::ENOTCONN => NOTCONN,
        libc::ENOTDIR => NOTDIR,
        libc::ENOTEMPTY => NOTEMPTY,
        libc::ENOTSOCK => NOTSOCK,
        libc::ENOTSUP => NOTSUP,
        libc::ENOTTY => NOTTY,
        libc::ENXIO => NXIO,
        libc::EOVERFLOW => OVERFLOW,
        libc::EPERM => PERM,
        libc::EPIPE => PIPE,
        libc::EPROTONOSUPPORT => PROTONOSUPPORT,
        libc::EPROTOTYPE => PROTOTYPE,
        libc::ERANGE => RANGE,
        libc::EROFS => ROFS,
        libc::ESPIPE => SPIPE,
        libc::ETIMEDOUT => TIMEDOUT,
        libc::ETXTBSY => TXTBSY,
        libc::EXDEV => XDEV,
        // Linux defines EOPNOTSUPP as ENOTSUP; macOS keeps them apart.
        #[cfg(target_os = "macos")]
        libc::EOPNOTSUPP => NOTSUP,
        _ => IO,
    }
}

/// The current host errno, translated. Never SUCCESS: it is only read after a call failed.
pub fn last() -> i32 {
    match std::io::Error::last_os_error().raw_os_error() {
        Some(0) | None => IO,
        Some(e) => from_host(e),
    }
}

/// A failed rustix call's host errno, translated.
pub fn rx(e: rustix::io::Errno) -> i32 {
    from_host(e.raw_os_error())
}

/// A failed std call's host errno, translated.
pub fn of_io(e: &std::io::Error) -> i32 {
    match e.raw_os_error() {
        Some(0) | None => IO,
        Some(e) => from_host(e),
    }
}

/// A libc return value, or the translated errno when it signals failure.
pub fn check(r: libc::c_int) -> Result<libc::c_int, i32> {
    if r < 0 {
        Err(last())
    } else {
        Ok(r)
    }
}

/// `check` for calls returning a byte count (`ssize_t`).
pub fn check_len(r: isize) -> Result<usize, i32> {
    if r < 0 {
        Err(last())
    } else {
        Ok(r as usize)
    }
}
