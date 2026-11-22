//! Durable snapshot replacement, used only during lifecycle or from the async scheduler.

use crate::regions::{self, MAX_FILE_BYTES, Policy};
use std::io::{Read, Write};
use std::path::Path;

pub fn read(path: &Path) -> Result<Policy, &'static str> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Policy::build(vec![]),
        Err(_) => return Err("storage"),
    };
    let mut text = String::new();
    file.take((MAX_FILE_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .map_err(|_| "storage")?;
    let (regions, bad) = regions::decode(&text);
    if bad > 0 {
        return Err("invalid");
    }
    Policy::build(regions)
}
pub fn write(path: &Path, policy: &Policy) -> Result<(), &'static str> {
    let text = regions::encode(policy.regions());
    if text.len() > MAX_FILE_BYTES {
        return Err("limit");
    }
    let temp = path.with_extension("tmp");
    let mut file = std::fs::File::create(&temp).map_err(|_| "storage")?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|_| "storage")?;
    drop(file);
    std::fs::rename(&temp, path).map_err(|_| "storage")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regions::{Bounds, Flag, Region, Verdict};
    fn scratch() -> std::path::PathBuf {
        // The build's own folder when it names one; any machine's temporary folder otherwise.
        let dir = std::env::var_os("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "worldprotector-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    #[test]
    fn restart_keeps_snapshot_and_failed_replace_keeps_old_bytes() {
        let dir = scratch();
        let file = dir.join("regions.tsv");
        assert!(read(&file).unwrap().regions().is_empty());
        let r = Region::new("r", "world", Bounds::point([0, 0, 0]), Some("0".repeat(32)));
        let p = Policy::build(vec![r]).unwrap();
        write(&file, &p).unwrap();
        assert_eq!(
            read(&file)
                .unwrap()
                .decide("world", [0, 0, 0], Flag::Break, None),
            Verdict::Deny
        );
        let old = std::fs::read(&file).unwrap();
        assert!(write(&dir.join("absent").join("regions.tsv"), &p).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), old);
    }
    #[test]
    fn corrupt_or_oversized_file_is_not_rewritten() {
        let dir = scratch();
        let file = dir.join("regions.tsv");
        for data in ["bad".to_owned(), "x".repeat(MAX_FILE_BYTES + 1)] {
            std::fs::write(&file, &data).unwrap();
            assert!(read(&file).is_err());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), data);
        }
    }
}
