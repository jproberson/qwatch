use std::fs::{File, FileTimes};
use std::path::Path;
use std::time::{Duration, SystemTime};

pub fn write_in_order(path: &Path, contents: &str, position: u64) {
    std::fs::write(path, contents).unwrap();
    let when = SystemTime::now() - Duration::from_secs(10_000 - position * 60);
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(when))
        .unwrap();
}
