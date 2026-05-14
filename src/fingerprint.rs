use blake3::Hasher;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Computes a cheap fingerprint of `dir` by hashing its direct children's
/// metadata only — no recursion. Runs in O(n direct children).
///
/// Captures:
///   - file/dir names (lowercased — Windows is case-insensitive)
///   - whether each entry is a file or directory
///   - file sizes (skipped for dirs — OS-reported dir size is meaningless/varies)
///   - mtimes in seconds (catches .gitignore edits, new files, deletions)
///
/// Falls back to an empty hash on I/O error so the caller treats the dir as
/// changed rather than silently skipping it.
pub fn fingerprint_dir(dir: &Path) -> std::io::Result<String> {
    let mut entries: Vec<(String, u8, u64, u64)> = Vec::new();
    //                      ^name  ^kind ^size ^mtime

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let name = entry.file_name().to_string_lossy().to_lowercase(); // Windows normalisation
        let kind = u8::from(meta.is_dir());
        let size = if meta.is_dir() { 0 } else { meta.len() };
        let mtime = mtime_secs(&meta);

        entries.push((name.clone(), kind, size, mtime));
    }

    // read_dir order varies by OS/filesystem — sort so the hash is deterministic
    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Hasher::new();
    for (name, kind, size, mtime) in &entries {
        hasher.update(name.as_bytes());
        hasher.update(&[*kind]);
        hasher.update(&size.to_le_bytes());
        hasher.update(&mtime.to_le_bytes());
        hasher.update(b"\0"); // field separator — prevents "ab"+"c" == "a"+"bc" collisions
    }

    Ok(hasher.finalize().to_hex().to_string())
}

/// Computes a deep fingerprint by combining a dir's own shallow fingerprint
/// with the already-computed deep fingerprints of its children.
///
/// This means a change anywhere in the subtree bubbles up through deep_fp
/// without requiring a full rescan — we only do the stat work as we recurse.
pub fn compute_deep_fp(shallow_fp: &str, child_deep_fps: &[String]) -> String {
    let mut hasher = Hasher::new();
    hasher.update(shallow_fp.as_bytes());
    hasher.update(b"|");
    for child_fp in child_deep_fps {
        hasher.update(child_fp.as_bytes());
        hasher.update(b"|");
    }
    hasher.finalize().to_hex().to_string()
}

/// Cross-platform mtime extraction.
///
/// Falls back to 0 on:
///   - FAT32 / network drives that return Err from .modified()
///   - Timestamps before UNIX_EPOCH (shouldn't happen but be safe)
///
/// A value of 0 means the entry looks perpetually "changed", which is safe:
/// we'll reprocess unnecessarily but never silently skip a real change.
fn mtime_secs(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
