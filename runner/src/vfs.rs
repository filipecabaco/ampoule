//! The release image appended by `ampoule pack`: b"RTFS0002", u32 dictionary length and the
//! dictionary (a zstd dictionary trained on the release's files, or nothing), u32 count, then per
//! entry u16 path length, path, u8 kind, u32 mode, u64 size, u64 offset, u32 stored size; then the
//! data region, one zstd frame per file compressed with the dictionary (symlink targets raw). The
//! index is parsed at startup and files are decompressed when the guest opens them. Read-only.
//! b"RTFS0001" is the same without the dictionary.

use std::collections::HashMap;

pub const MAGIC: &[u8; 8] = b"RTFS0002";
/// The format before the dictionary.
const MAGIC_V1: &[u8; 8] = b"RTFS0001";
pub const KIND_DIR: u8 = 0;
pub const KIND_FILE: u8 = 1;
pub const KIND_SYMLINK: u8 = 2;

pub struct Node {
    pub kind: u8,
    pub mode: u32,
    pub size: u64,
    pub ino: u64,
    offset: u64,
    csize: u32,
}

pub struct RootFs {
    data: &'static [u8],
    /// Prepared once: every file's frame was compressed with it.
    dict: Option<zstd::dict::DecoderDictionary<'static>>,
    nodes: HashMap<String, Node>,
    children: HashMap<String, Vec<String>>,
}

/// The next `n` bytes of the index, or an error if the image is cut short.
fn take<'a>(b: &mut &'a [u8], n: usize) -> anyhow::Result<&'a [u8]> {
    anyhow::ensure!(b.len() >= n, "the release image is truncated");
    let (head, rest) = b.split_at(n);
    *b = rest;
    Ok(head)
}

fn take_array<const N: usize>(b: &mut &[u8]) -> anyhow::Result<[u8; N]> {
    Ok(take(b, N)?.try_into()?)
}

impl RootFs {
    pub fn parse(blob: &'static [u8]) -> anyhow::Result<RootFs> {
        let mut b = blob;
        let dict = match take(&mut b, 8).ok() {
            Some(m) if m == &MAGIC[..] => {
                let len = u32::from_le_bytes(take_array(&mut b)?) as usize;
                let bytes = take(&mut b, len)?;
                (!bytes.is_empty()).then(|| zstd::dict::DecoderDictionary::copy(bytes))
            },
            Some(m) if m == &MAGIC_V1[..] => None,
            _ => anyhow::bail!("the release image has an unknown format"),
        };
        let count = u32::from_le_bytes(take_array(&mut b)?) as usize;
        // Each entry takes at least 27 bytes, which bounds a count read from a corrupt image.
        anyhow::ensure!(count <= b.len() / 27, "the release image is truncated");
        let mut nodes = HashMap::with_capacity(count);
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        for i in 0..count {
            let len = u16::from_le_bytes(take_array(&mut b)?) as usize;
            let path = std::str::from_utf8(take(&mut b, len)?)?.to_string();
            let [kind] = take_array(&mut b)?;
            let mode = u32::from_le_bytes(take_array(&mut b)?);
            let size = u64::from_le_bytes(take_array(&mut b)?);
            let offset = u64::from_le_bytes(take_array(&mut b)?);
            let csize = u32::from_le_bytes(take_array(&mut b)?);
            anyhow::ensure!(path.starts_with('/'), "the release image has a relative path {path:?}");
            if path != "/" {
                let (parent, name) = path.rsplit_once('/').unwrap_or(("", &path));
                let parent = if parent.is_empty() { "/" } else { parent };
                children.entry(parent.to_string()).or_default().push(name.to_string());
            }
            nodes.insert(path, Node { kind, mode, size, ino: i as u64 + 1, offset, csize });
        }
        // Every entry's data must lie inside the data region.
        for (path, n) in &nodes {
            let end = n.offset.checked_add(u64::from(n.csize));
            anyhow::ensure!(end.is_some_and(|e| e <= b.len() as u64), "the release image is truncated at {path}");
        }
        Ok(RootFs { data: b, dict, nodes, children })
    }

    pub fn get(&self, path: &str) -> Option<&Node> {
        self.nodes.get(path)
    }

    /// The node `path` names, with symlinks resolved in every directory on the way and, when
    /// `follow`, in the last component too. Targets are relative to the link's directory or
    /// absolute within the image; returns the resolved path too.
    pub fn lookup(&self, path: &str, follow: bool) -> Option<(String, &Node)> {
        let mut path = crate::kernel::normalize(path);
        'hops: for _ in 0..=crate::hostfs::MAX_HOPS {
            let parts: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
            let mut at = String::new();
            for (i, part) in parts.iter().enumerate() {
                let next = format!("{at}/{part}");
                let node = self.nodes.get(&next)?;
                let last = i + 1 == parts.len();
                if node.kind == KIND_SYMLINK && (!last || follow) {
                    let target = String::from_utf8_lossy(self.raw(node));
                    let base = if target.starts_with('/') { target.into_owned() } else { format!("{at}/{target}") };
                    path = crate::kernel::normalize(&format!("{base}/{}", parts[i + 1..].join("/")));
                    continue 'hops;
                }
                at = next;
            }
            let key = if at.is_empty() { "/".to_string() } else { at };
            let node = self.nodes.get(&key)?;
            return Some((key, node));
        }
        None
    }

    fn raw(&self, n: &Node) -> &'static [u8] {
        &self.data[n.offset as usize..n.offset as usize + n.csize as usize]
    }

    /// Decompress a file. Its recorded size bounds the buffer, and is itself bounded: no guest
    /// can address more than 4 GiB.
    pub fn read(&self, n: &Node) -> std::io::Result<Vec<u8>> {
        let size = usize::try_from(n.size).ok().filter(|s| *s <= u32::MAX as usize).ok_or(std::io::ErrorKind::InvalidData)?;
        match &self.dict {
            Some(d) => zstd::bulk::Decompressor::with_prepared_dictionary(d)?.decompress(self.raw(n), size),
            None => zstd::bulk::decompress(self.raw(n), size),
        }
    }

    pub fn readlink(&self, n: &Node) -> Option<Vec<u8>> {
        (n.kind == KIND_SYMLINK).then(|| self.raw(n).to_vec())
    }

    pub fn list(&self, dir: &str) -> Vec<(String, &Node)> {
        let Some(names) = self.children.get(dir) else { return Vec::new() };
        names
            .iter()
            .filter_map(|name| {
                let full = if dir == "/" { format!("/{name}") } else { format!("{dir}/{name}") };
                self.nodes.get(&full).map(|n| (name.clone(), n))
            })
            .collect()
    }

    pub fn files(&self) -> usize {
        self.nodes.values().filter(|n| n.kind == KIND_FILE).count()
    }
}
