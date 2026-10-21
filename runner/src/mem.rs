//! Access to the guest's shared linear memory.
//!
//! Every wasm thread shares one `SharedMemory`, and other threads write to it concurrently, so
//! nothing here hands out Rust references into it: values are copied in and out, and raw
//! pointers are given only to host syscalls that read or fill guest buffers directly.

use wasmtime::SharedMemory;

use crate::errno;

#[derive(Clone)]
pub struct Mem {
    memory: SharedMemory,
}

pub type R<T> = Result<T, i32>;

impl Mem {
    pub fn new(memory: SharedMemory) -> Mem {
        Mem { memory }
    }

    fn base(&self) -> *mut u8 {
        self.memory.data().as_ptr() as *mut u8
    }

    /// The guest's memory size in bytes. From the slice `data()` returns, not `data_size()`, which
    /// takes the memory's lock: every bounds check calls this, from every guest thread, and that
    /// lock was the hottest point of the runtime under load. Shared memory never shrinks, so a
    /// length read here stays valid.
    pub fn size(&self) -> usize {
        self.memory.data().len()
    }

    /// The most the guest's memory can grow to, in bytes: `--memory-max`, or wasm32's 4 GiB.
    pub fn limit(&self) -> u64 {
        self.memory.ty().maximum().unwrap_or(65536) * 65536
    }

    /// A host pointer to `len` guest bytes at `addr`, bounds-checked.
    pub fn ptr(&self, addr: u32, len: u32) -> R<*mut u8> {
        let end = addr as usize + len as usize;
        if end > self.size() {
            return Err(errno::FAULT);
        }
        // SAFETY: `addr + len` is within the memory's current size, which never shrinks.
        Ok(unsafe { self.base().add(addr as usize) })
    }

    pub fn read(&self, addr: u32, len: u32) -> R<Vec<u8>> {
        let p = self.ptr(addr, len)?;
        let mut v = vec![0u8; len as usize];
        // SAFETY: `p` is `len` bounds-checked guest bytes, `v` a separate host buffer as long; a
        // raw copy, never a Rust slice over memory other threads write.
        unsafe { std::ptr::copy_nonoverlapping(p, v.as_mut_ptr(), len as usize) };
        Ok(v)
    }

    pub fn write(&self, addr: u32, data: &[u8]) -> R<()> {
        let p = self.ptr(addr, data.len() as u32)?;
        // SAFETY: as in `read`, the other way.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len()) };
        Ok(())
    }

    /// Check that `count` elements of `size` bytes at `base` are all inside guest memory, so
    /// offsets computed within the array cannot overflow or leave it.
    pub fn array(&self, base: u32, count: u32, size: u32) -> R<()> {
        let bytes = count.checked_mul(size).ok_or(errno::FAULT)?;
        self.ptr(base, bytes).map(|_| ())
    }

    fn bytes<const N: usize>(&self, addr: u32) -> R<[u8; N]> {
        let p = self.ptr(addr, N as u32)?;
        let mut out = [0u8; N];
        // SAFETY: as in `read`.
        unsafe { std::ptr::copy_nonoverlapping(p, out.as_mut_ptr(), N) };
        Ok(out)
    }

    pub fn str(&self, addr: u32, len: u32) -> R<String> {
        String::from_utf8(self.read(addr, len)?).map_err(|_| errno::INVAL)
    }

    pub fn u8(&self, addr: u32) -> R<u8> {
        Ok(self.bytes::<1>(addr)?[0])
    }

    pub fn u16(&self, addr: u32) -> R<u16> {
        self.bytes(addr).map(u16::from_le_bytes)
    }

    pub fn u32(&self, addr: u32) -> R<u32> {
        self.bytes(addr).map(u32::from_le_bytes)
    }

    pub fn u64(&self, addr: u32) -> R<u64> {
        self.bytes(addr).map(u64::from_le_bytes)
    }

    pub fn set_u8(&self, addr: u32, v: u8) -> R<()> {
        self.write(addr, &[v])
    }

    pub fn set_u16(&self, addr: u32, v: u16) -> R<()> {
        self.write(addr, &v.to_le_bytes())
    }

    pub fn set_u32(&self, addr: u32, v: u32) -> R<()> {
        self.write(addr, &v.to_le_bytes())
    }

    pub fn set_u64(&self, addr: u32, v: u64) -> R<()> {
        self.write(addr, &v.to_le_bytes())
    }

    /// Give the host back the pages of `len` guest bytes at `addr`, which the guest no longer
    /// uses: they stop counting in the process's RSS and read as zero when next touched. The
    /// range must be page-aligned (wasm pages, which are whole host pages) and inside the memory.
    ///
    /// Linear memory can't shrink, so this is how memory the VM freed leaves the process: the
    /// guest's segment allocator (`aos_pages_free` in engine/ampoule-os) calls it for every range
    /// ERTS unmaps.
    pub fn discard(&self, addr: u32, len: u32) -> R<()> {
        const WASM_PAGE: u32 = 65536;
        if !addr.is_multiple_of(WASM_PAGE) || !len.is_multiple_of(WASM_PAGE) {
            return Err(errno::INVAL);
        }
        if len == 0 {
            return Ok(());
        }
        let p = self.ptr(addr, len)?.cast::<std::ffi::c_void>();
        // Wasm pages are a multiple of every host page size ampoule runs on (4K, 16K, 64K).
        debug_assert!((p as usize).is_multiple_of(rustix::param::page_size()));
        discard_pages(p, len as usize)
    }

    /// Guest iovecs (`{ buf: u32, len: u32 }`) as host iovecs pointing into guest memory.
    pub fn iovecs(&self, iovs: u32, count: u32) -> R<Vec<libc::iovec>> {
        self.array(iovs, count, 8)?;
        // IOV_MAX: the host's readv/writev refuse more anyway.
        if count > 1024 {
            return Err(errno::INVAL);
        }
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let buf = self.u32(iovs + i * 8)?;
            let len = self.u32(iovs + i * 8 + 4)?;
            out.push(libc::iovec { iov_base: self.ptr(buf, len)? as *mut libc::c_void, iov_len: len as usize });
        }
        Ok(out)
    }
}

/// `ampoule_os.discard(addr, len)`: the guest's segment allocator handing freed pages back
/// (`Mem::discard`). 0, or -errno.
pub fn add(linker: &mut wasmtime::Linker<crate::Ctx>) -> anyhow::Result<()> {
    linker.func_wrap("ampoule_os", "discard", |c: wasmtime::Caller<'_, crate::Ctx>, addr: i32, len: i32| -> i32 {
        crate::net::ret(crate::k(&c).mem.discard(addr as u32, len as u32).map(|()| 0))
    })?;
    Ok(())
}

/// Linux drops the pages of a private anonymous mapping on `MADV_DONTNEED`: the RSS falls at
/// once and the next touch maps a zero page. The VMA is left whole.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn discard_pages(p: *mut std::ffi::c_void, len: usize) -> R<()> {
    // SAFETY: `p..p + len` is page-aligned guest memory inside the shared memory's mapping
    // (`Mem::discard` checked both). The guest has freed it, so no guest thread uses it, and the
    // host holds no Rust references into guest memory (see the module doc). The advice only
    // replaces the contents with zero pages; the mapping and its protection stay as they were.
    unsafe { rustix::mm::madvise(p, len, rustix::mm::Advice::LinuxDontNeed) }.map_err(errno::rx)
}

/// macOS keeps anonymous pages resident after `MADV_DONTNEED`, and `MADV_FREE` only drops them
/// under memory pressure. Mapping fresh anonymous memory over the range, with the protection it
/// already has, frees the old pages at once; the next touch maps a zero page, as on Linux.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn discard_pages(p: *mut std::ffi::c_void, len: usize) -> R<()> {
    use rustix::mm::{MapFlags, ProtFlags};
    // SAFETY: as for Linux's `discard_pages`: freed, page-aligned guest memory inside the shared
    // memory's mapping, which wasmtime reserved private, anonymous and read-write up to its
    // current size. MAP_FIXED replaces exactly that range with a mapping of the same kind, so the
    // memory stays valid for every thread; only its contents are dropped.
    let got = unsafe { rustix::mm::mmap_anonymous(p, len, ProtFlags::READ | ProtFlags::WRITE, MapFlags::PRIVATE | MapFlags::FIXED) }.map_err(errno::rx)?;
    if got != p {
        return Err(errno::FAULT);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: u32 = 65536;

    fn mem(pages: u32) -> Mem {
        let engine = wasmtime::Engine::new(&crate::engine::config()).unwrap();
        Mem::new(SharedMemory::new(&engine, wasmtime::MemoryType::shared(pages, pages)).unwrap())
    }

    /// Host pages of `len` bytes at guest `addr` that are resident, from mincore(2).
    fn resident(m: &Mem, addr: u32, len: u32) -> usize {
        let host_page = rustix::param::page_size();
        let mut vec = vec![0u8; len as usize / host_page];
        // SAFETY: the range is page-aligned guest memory inside the mapping (the tests use
        // whole wasm pages), and `vec` has one byte per host page in it.
        let r = unsafe { libc::mincore(m.ptr(addr, len).unwrap().cast(), len as usize, vec.as_mut_ptr().cast()) };
        assert_eq!(r, 0, "mincore: {}", std::io::Error::last_os_error());
        vec.iter().filter(|&&b| b & 1 != 0).count()
    }

    #[test]
    fn discard_zeroes_the_range_and_drops_its_pages() {
        let m = mem(8);
        let all = 8 * PAGE;
        m.write(0, &vec![0xa5; all as usize]).unwrap();
        let host_pages = all as usize / rustix::param::page_size();
        assert_eq!(resident(&m, 0, all), host_pages);

        m.discard(2 * PAGE, 4 * PAGE).unwrap();

        assert_eq!(resident(&m, 2 * PAGE, 4 * PAGE), 0, "discarded pages still resident");
        assert_eq!(resident(&m, 0, 2 * PAGE) + resident(&m, 6 * PAGE, 2 * PAGE), host_pages / 2);
        assert!(m.read(2 * PAGE, 4 * PAGE).unwrap().iter().all(|&b| b == 0));
        assert!(m.read(0, 2 * PAGE).unwrap().iter().all(|&b| b == 0xa5));
        assert!(m.read(6 * PAGE, 2 * PAGE).unwrap().iter().all(|&b| b == 0xa5));
        // The memory is still read-write.
        m.write(3 * PAGE, b"again").unwrap();
        assert_eq!(m.read(3 * PAGE, 5).unwrap(), b"again");
    }

    #[test]
    fn discard_checks_the_range() {
        let m = mem(4);
        assert_eq!(m.discard(PAGE / 2, PAGE), Err(errno::INVAL));
        assert_eq!(m.discard(0, PAGE + 1), Err(errno::INVAL));
        assert_eq!(m.discard(3 * PAGE, 2 * PAGE), Err(errno::FAULT));
        assert_eq!(m.discard(u32::MAX - PAGE + 1, PAGE), Err(errno::FAULT));
        assert_eq!(m.discard(PAGE, 0), Ok(()));
    }
}
