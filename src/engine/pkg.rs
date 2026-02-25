use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;

// ── Internal directory entry ───────────────────────────────────────────────────

struct FileEntry {
    offset: u32,
    length: u32,
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// A loaded Wallpaper Engine PKG archive.
///
/// All file data is kept in memory; [`get`](Self::get) returns zero-copy slices.
pub struct Package {
    data: Vec<u8>,
    files: HashMap<String, FileEntry>,
    base_offset: usize,
}

impl Package {
    /// Load a PKG archive from disk.
    pub fn from_file(path: &Path) -> Result<Self> {
        let data = std::fs::read(path)
            .with_context(|| format!("failed to read PKG file: {}", path.display()))?;
        Self::from_bytes(data)
    }

    /// Parse a PKG archive from an already-loaded byte buffer.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        let (files, base_offset) = Self::parse_directory(&data)?;
        Ok(Self { data, files, base_offset })
    }

    /// Return the raw bytes for `path`, or `None` if the path is not in the archive.
    ///
    /// Entries with `length = 0` return an empty slice (not `None`).
    /// Out-of-bounds entries return `None` rather than panicking.
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        let entry = self.files.get(&normalize_path(path))?;
        let start = self.base_offset + entry.offset as usize;
        let end = start + entry.length as usize;
        self.data.get(start..end)
    }

    /// Returns `true` if the archive contains `path`.
    pub fn contains(&self, path: &str) -> bool {
        self.files.contains_key(&normalize_path(path))
    }

    /// Iterator over every path in the archive (unordered).
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(|s| s.as_str())
    }

    /// Number of entries in the directory.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Returns `true` if the archive contains no entries.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    // ── Private ────────────────────────────────────────────────────────────────

    fn parse_directory(data: &[u8]) -> Result<(HashMap<String, FileEntry>, usize)> {
        let mut cur = Cursor::new(data);

        // Header: sized string, must start with "PKGV"
        let header = read_sized_string(&mut cur).context("failed to read PKG header")?;
        if !header.starts_with("PKGV") {
            bail!("not a PKG archive: header is {:?}", header);
        }

        let file_count = read_u32(&mut cur).context("failed to read file count")?;

        let mut files = HashMap::with_capacity(file_count as usize);
        for _ in 0..file_count {
            let name = read_sized_string(&mut cur).context("failed to read entry name")?;
            let offset = read_u32(&mut cur).context("failed to read entry offset")?;
            let length = read_u32(&mut cur).context("failed to read entry length")?;
            files.insert(normalize_path(&name), FileEntry { offset, length });
        }

        let base_offset = cur.position() as usize;
        Ok((files, base_offset))
    }
}

// ── Low-level readers ──────────────────────────────────────────────────────────

fn read_u32(cur: &mut Cursor<&[u8]>) -> Result<u32> {
    let mut buf = [0u8; 4];
    cur.read_exact(&mut buf).context("unexpected EOF reading u32")?;
    Ok(u32::from_le_bytes(buf))
}

fn read_sized_string(cur: &mut Cursor<&[u8]>) -> Result<String> {
    let len = read_u32(cur)? as usize;
    let mut buf = vec![0u8; len];
    cur.read_exact(&mut buf).context("unexpected EOF reading string bytes")?;
    String::from_utf8(buf).context("PKG string is not valid UTF-8")
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}
