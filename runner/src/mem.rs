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
