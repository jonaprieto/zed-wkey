//! An event log for debugging: what wkey saw and what it made of it.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static LOG: OnceLock<(Mutex<std::fs::File>, Instant)> = OnceLock::new();

/// Past this the log starts over when the view opens.
const MAX: u64 = 1 << 20;

/// `events.log` beside the cursor file.
pub fn default_path() -> Option<PathBuf> {
    crate::lsp::default_cursor_path().map(|p| p.with_file_name("events.log"))
}

/// Starts logging to `path`; until then `event` does nothing.
pub fn open(path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let big = std::fs::metadata(path).is_ok_and(|m| m.len() > MAX);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    if big {
        file.set_len(0)?;
    }
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    writeln!(
        file,
        "--- wkey {} started, unix time {unix}",
        std::process::id()
    )?;
    let _ = LOG.set((Mutex::new(file), Instant::now()));
    Ok(())
}

/// One line, stamped with the seconds since the view opened.
pub fn event(line: std::fmt::Arguments) {
    if let Some((file, start)) = LOG.get()
        && let Ok(mut file) = file.lock()
    {
        let _ = writeln!(file, "{:>9.3} {line}", start.elapsed().as_secs_f64());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn events_land_in_the_log_once_opened() {
        let dir = std::env::temp_dir().join(format!("wkey-log-{}", std::process::id()));
        let path = dir.join("events.log");
        super::open(&path).unwrap();
        super::event(format_args!("zed   ctrl-k -> Prefix"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("started") && text.contains("zed   ctrl-k -> Prefix"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
