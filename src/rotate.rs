//! Core rotation engine: decide whether a file has crossed a size/age
//! threshold, then shift `file.N` -> `file.N+1` (pruning past `max_count`),
//! move the live file to `file.1`, recreate an empty live file, and
//! optionally gzip the newly rotated copy.

use anyhow::{Context, Result};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::compress::compress_file;

/// Thresholds and retention policy for one rotation decision/run.
#[derive(Debug, Clone)]
pub struct RotateConfig {
    pub max_size: Option<u64>,
    pub max_age: Option<Duration>,
    pub max_count: u32,
    pub compress: bool,
}

/// What actually happened during a [`rotate`] call, used both for the
/// printed summary and for tests to assert on real on-disk effects.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RotationOutcome {
    /// `(old file name, new file name)` for every pre-existing rotated file
    /// that got renumbered up by one.
    pub shifted: Vec<(String, String)>,
    /// File names removed because they aged past `max_count`.
    pub pruned: Vec<String>,
    /// The size (bytes) of the live file at the moment it was rotated.
    pub rotated_size: u64,
    /// The rotated copy's file name after optional compression
    /// (`app.log.1` or `app.log.1.gz`).
    pub new_entry_name: String,
}

/// Returns `true` if `path` has crossed whichever of `max_size`/`max_age`
/// is configured (either condition is sufficient), evaluated as of `now`
/// so tests don't depend on real wall-clock timing.
pub fn needs_rotation(path: &Path, config: &RotateConfig, now: SystemTime) -> Result<bool> {
    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;

    if let Some(max_size) = config.max_size {
        if meta.len() >= max_size {
            return Ok(true);
        }
    }

    if let Some(max_age) = config.max_age {
        let modified = meta.modified().with_context(|| {
            format!(
                "reading mtime of {} (platform doesn't support it)",
                path.display()
            )
        })?;
        let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
        if age >= max_age {
            return Ok(true);
        }
    }

    Ok(false)
}

/// One parsed `<base>.<n>` or `<base>.<n>.gz` sibling file.
struct RotatedEntry {
    index: u32,
    is_gz: bool,
    path: PathBuf,
}

/// If `filename` is a rotated sibling of `base` (`base.3` or `base.3.gz`),
/// returns its index and whether it's gzip-compressed.
fn parse_rotated_name(base: &str, filename: &str) -> Option<(u32, bool)> {
    let rest = filename.strip_prefix(base)?.strip_prefix('.')?;
    if let Some(digits) = rest.strip_suffix(".gz") {
        digits.parse().ok().map(|n| (n, true))
    } else {
        rest.parse().ok().map(|n| (n, false))
    }
}

fn existing_rotated_entries(dir: &Path, base: &str) -> Result<Vec<RotatedEntry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some((index, is_gz)) = parse_rotated_name(base, &name) {
            entries.push(RotatedEntry {
                index,
                is_gz,
                path: entry.path(),
            });
        }
    }
    entries.sort_by_key(|e| std::cmp::Reverse(e.index));
    Ok(entries)
}

/// Performs one rotation of `path` unconditionally (callers check
/// [`needs_rotation`] first, or pass `--force`).
pub fn rotate(path: &Path, config: &RotateConfig) -> Result<RotationOutcome> {
    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let rotated_size = meta.len();

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let base = path
        .file_name()
        .context("rotation target has no file name")?
        .to_string_lossy()
        .into_owned();

    let mut outcome = RotationOutcome {
        rotated_size,
        ..Default::default()
    };

    // Highest index first, so renaming never clobbers a not-yet-moved file.
    for entry in existing_rotated_entries(dir, &base)? {
        let new_index = entry.index + 1;
        let old_name = entry
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if new_index > config.max_count {
            fs::remove_file(&entry.path)
                .with_context(|| format!("pruning {}", entry.path.display()))?;
            outcome.pruned.push(old_name);
        } else {
            let suffix = if entry.is_gz { ".gz" } else { "" };
            let new_name = format!("{base}.{new_index}{suffix}");
            let new_path = dir.join(&new_name);
            fs::rename(&entry.path, &new_path).with_context(|| {
                format!(
                    "renaming {} -> {}",
                    entry.path.display(),
                    new_path.display()
                )
            })?;
            outcome.shifted.push((old_name, new_name));
        }
    }

    // Move the live file to `<base>.1`.
    let rotated_path = dir.join(format!("{base}.1"));
    fs::rename(path, &rotated_path)
        .with_context(|| format!("rotating {} -> {}", path.display(), rotated_path.display()))?;

    // Recreate an empty live file so anything still writing by path picks
    // right back up (no copytruncate / reopen-signal support — see README).
    File::create(path).with_context(|| format!("recreating {}", path.display()))?;

    if config.compress {
        let gz_path = dir.join(format!("{base}.1.gz"));
        compress_file(&rotated_path, &gz_path)?;
        outcome.new_entry_name = format!("{base}.1.gz");
    } else {
        outcome.new_entry_name = format!("{base}.1");
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn scratch_dir(name: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH as EPOCH};
        let unique = SystemTime::now().duration_since(EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rotatelog-rotate-test-{}-{}-{}",
            std::process::id(),
            name,
            unique
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cfg(
        max_size: Option<u64>,
        max_age: Option<Duration>,
        max_count: u32,
        compress: bool,
    ) -> RotateConfig {
        RotateConfig {
            max_size,
            max_age,
            max_count,
            compress,
        }
    }

    #[test]
    fn needs_rotation_false_under_size_threshold() {
        let dir = scratch_dir("size-under");
        let file = dir.join("app.log");
        fs::write(&file, b"short").unwrap();
        let c = cfg(Some(1024), None, 5, false);
        assert!(!needs_rotation(&file, &c, SystemTime::now()).unwrap());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn needs_rotation_true_over_size_threshold() {
        let dir = scratch_dir("size-over");
        let file = dir.join("app.log");
        fs::write(&file, vec![b'x'; 2048]).unwrap();
        let c = cfg(Some(1024), None, 5, false);
        assert!(needs_rotation(&file, &c, SystemTime::now()).unwrap());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn needs_rotation_true_over_age_threshold_using_injected_now() {
        let dir = scratch_dir("age-over");
        let file = dir.join("app.log");
        fs::write(&file, b"content").unwrap();
        let c = cfg(None, Some(Duration::from_secs(60)), 5, false);
        // Pretend "now" is an hour after the file's real mtime.
        let far_future = SystemTime::now() + Duration::from_secs(3600);
        assert!(needs_rotation(&file, &c, far_future).unwrap());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn needs_rotation_false_under_age_threshold() {
        let dir = scratch_dir("age-under");
        let file = dir.join("app.log");
        fs::write(&file, b"content").unwrap();
        let c = cfg(None, Some(Duration::from_secs(3600)), 5, false);
        assert!(!needs_rotation(&file, &c, SystemTime::now()).unwrap());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn needs_rotation_false_when_no_thresholds_configured() {
        let dir = scratch_dir("no-threshold");
        let file = dir.join("app.log");
        fs::write(&file, vec![b'x'; 999_999]).unwrap();
        let c = cfg(None, None, 5, false);
        assert!(!needs_rotation(&file, &c, SystemTime::now()).unwrap());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn needs_rotation_errors_on_missing_file() {
        let dir = scratch_dir("missing");
        let file = dir.join("does-not-exist.log");
        let c = cfg(Some(1), None, 5, false);
        assert!(needs_rotation(&file, &c, SystemTime::now()).is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn first_rotation_creates_dot_1_and_fresh_empty_live_file() {
        let dir = scratch_dir("first");
        let file = dir.join("app.log");
        fs::write(&file, b"original content").unwrap();
        let c = cfg(None, None, 5, false);

        let outcome = rotate(&file, &c).unwrap();

        assert_eq!(outcome.new_entry_name, "app.log.1");
        assert_eq!(
            fs::read(dir.join("app.log.1")).unwrap(),
            b"original content"
        );
        assert_eq!(fs::read(&file).unwrap(), Vec::<u8>::new());
        assert!(outcome.shifted.is_empty());
        assert!(outcome.pruned.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn second_rotation_shifts_dot_1_to_dot_2() {
        let dir = scratch_dir("second");
        let file = dir.join("app.log");
        fs::write(&file, b"first generation").unwrap();
        let c = cfg(None, None, 5, false);
        rotate(&file, &c).unwrap();

        fs::write(&file, b"second generation").unwrap();
        let outcome = rotate(&file, &c).unwrap();

        assert_eq!(
            outcome.shifted,
            vec![("app.log.1".to_string(), "app.log.2".to_string())]
        );
        assert_eq!(
            fs::read(dir.join("app.log.2")).unwrap(),
            b"first generation"
        );
        assert_eq!(
            fs::read(dir.join("app.log.1")).unwrap(),
            b"second generation"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn oldest_pruned_once_max_count_exceeded() {
        let dir = scratch_dir("prune");
        let file = dir.join("app.log");
        let c = cfg(None, None, 2, false);

        for gen in 0..3 {
            fs::write(&file, format!("generation {gen}")).unwrap();
            rotate(&file, &c).unwrap();
        }
        // After 3 rotations with max_count=2: .1 = gen2, .2 = gen1, gen0 pruned.
        assert!(dir.join("app.log.1").exists());
        assert!(dir.join("app.log.2").exists());
        assert!(!dir.join("app.log.3").exists());
        assert_eq!(fs::read(dir.join("app.log.1")).unwrap(), b"generation 2");
        assert_eq!(fs::read(dir.join("app.log.2")).unwrap(), b"generation 1");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn last_rotation_reports_the_pruned_file_by_name() {
        let dir = scratch_dir("prune-report");
        let file = dir.join("app.log");
        let c = cfg(None, None, 1, false);

        fs::write(&file, b"gen0").unwrap();
        rotate(&file, &c).unwrap(); // -> .1 = gen0
        fs::write(&file, b"gen1").unwrap();
        let outcome = rotate(&file, &c).unwrap(); // gen0's .1 would become .2, pruned instead

        assert_eq!(outcome.pruned, vec!["app.log.1".to_string()]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compress_true_produces_gz_not_plain_copy() {
        let dir = scratch_dir("compress");
        let file = dir.join("app.log");
        fs::write(&file, b"compress me please\n".repeat(100)).unwrap();
        let c = cfg(None, None, 5, true);

        let outcome = rotate(&file, &c).unwrap();

        assert_eq!(outcome.new_entry_name, "app.log.1.gz");
        assert!(dir.join("app.log.1.gz").exists());
        assert!(!dir.join("app.log.1").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn renumbering_preserves_gz_suffix_on_shift() {
        let dir = scratch_dir("compress-shift");
        let file = dir.join("app.log");
        let c = cfg(None, None, 5, true);

        fs::write(&file, b"gen0").unwrap();
        rotate(&file, &c).unwrap(); // -> app.log.1.gz
        fs::write(&file, b"gen1").unwrap();
        let outcome = rotate(&file, &c).unwrap(); // .1.gz -> .2.gz, new .1.gz

        assert_eq!(
            outcome.shifted,
            vec![("app.log.1.gz".to_string(), "app.log.2.gz".to_string())]
        );
        assert!(dir.join("app.log.2.gz").exists());
        assert!(dir.join("app.log.1.gz").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rotating_missing_file_errors_cleanly_not_a_panic() {
        let dir = scratch_dir("missing-rotate");
        let file = dir.join("nope.log");
        let c = cfg(None, None, 5, false);
        let result = rotate(&file, &c);
        assert!(result.is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unrelated_files_in_same_directory_are_ignored() {
        let dir = scratch_dir("unrelated");
        let file = dir.join("app.log");
        fs::write(&file, b"content").unwrap();
        fs::write(dir.join("app.log.bak"), b"not a rotation target").unwrap();
        fs::write(dir.join("other.log.1"), b"different base name").unwrap();
        let c = cfg(None, None, 5, false);

        let outcome = rotate(&file, &c).unwrap();

        assert!(outcome.shifted.is_empty());
        assert!(dir.join("app.log.bak").exists());
        assert_eq!(
            fs::read(dir.join("other.log.1")).unwrap(),
            b"different base name"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_rotated_name_rejects_non_numeric_suffix() {
        assert_eq!(parse_rotated_name("app.log", "app.log.bak"), None);
        assert_eq!(parse_rotated_name("app.log", "app.log.3"), Some((3, false)));
        assert_eq!(
            parse_rotated_name("app.log", "app.log.3.gz"),
            Some((3, true))
        );
        assert_eq!(parse_rotated_name("app.log", "unrelated.txt"), None);
    }

    #[test]
    fn epoch_mtime_never_panics_needs_rotation() {
        // Sanity check that duration_since never panics even in odd clock
        // configurations (e.g. now < mtime), by using UNIX_EPOCH as "now".
        let dir = scratch_dir("epoch");
        let file = dir.join("app.log");
        fs::write(&file, b"content").unwrap();
        let c = cfg(None, Some(Duration::from_secs(1)), 5, false);
        assert!(!needs_rotation(&file, &c, UNIX_EPOCH).unwrap());
        fs::remove_dir_all(&dir).ok();
    }
}
