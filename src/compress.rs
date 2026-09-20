//! gzip compression for a freshly rotated file, using the same format
//! `gzip`/`logrotate --compress` itself produces (so `gunzip`/`zcat` on the
//! result work unmodified).

use anyhow::{Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs::{self, File};
use std::io;
use std::path::Path;

/// Compresses `src` into `dst` (expected to end in `.gz`) with gzip's
/// default compression level, then removes `src`. Leaves `src` in place if
/// compression fails partway, so a crash never silently loses log data.
pub fn compress_file(src: &Path, dst: &Path) -> Result<u64> {
    let mut input =
        File::open(src).with_context(|| format!("opening {} to compress", src.display()))?;
    let output = File::create(dst).with_context(|| format!("creating {}", dst.display()))?;
    let mut encoder = GzEncoder::new(output, Compression::default());
    io::copy(&mut input, &mut encoder).with_context(|| format!("compressing {}", src.display()))?;
    encoder
        .finish()
        .with_context(|| format!("finishing gzip stream for {}", dst.display()))?;

    let compressed_size = fs::metadata(dst)?.len();
    fs::remove_file(src).with_context(|| format!("removing uncompressed {}", src.display()))?;
    Ok(compressed_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rotatelog-compress-test-{}-{}-{}",
            std::process::id(),
            name,
            fastrand_stub()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // Small dependency-free stand-in for a random suffix so parallel test
    // threads never collide on the same scratch directory name.
    fn fastrand_stub() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos() as u64
    }

    #[test]
    fn compresses_and_removes_original() {
        let dir = scratch_dir("basic");
        let src = dir.join("app.log.1");
        let dst = dir.join("app.log.1.gz");
        fs::write(&src, b"hello rotated log content\n".repeat(50)).unwrap();

        compress_file(&src, &dst).unwrap();

        assert!(!src.exists(), "uncompressed source should be removed");
        assert!(dst.exists(), "gzip output should exist");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compressed_output_is_genuinely_gzip_decodable() {
        let dir = scratch_dir("roundtrip");
        let src = dir.join("app.log.1");
        let dst = dir.join("app.log.1.gz");
        let original = b"the quick brown fox jumps over the lazy dog\n".repeat(200);
        fs::write(&src, &original).unwrap();

        compress_file(&src, &dst).unwrap();

        let file = File::open(&dst).unwrap();
        let mut decoder = GzDecoder::new(file);
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, original);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compressed_file_is_smaller_for_repetitive_content() {
        let dir = scratch_dir("smaller");
        let src = dir.join("app.log.1");
        let dst = dir.join("app.log.1.gz");
        let original = b"repeat me please\n".repeat(2000);
        fs::write(&src, &original).unwrap();
        let original_len = original.len() as u64;

        let compressed_len = compress_file(&src, &dst).unwrap();

        assert!(
            compressed_len < original_len,
            "expected gzip to shrink {original_len} highly repetitive bytes, got {compressed_len}"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_source_is_an_error_not_a_panic() {
        let dir = scratch_dir("missing");
        let src = dir.join("does-not-exist.log");
        let dst = dir.join("does-not-exist.log.gz");
        assert!(compress_file(&src, &dst).is_err());
        fs::remove_dir_all(&dir).ok();
    }
}
