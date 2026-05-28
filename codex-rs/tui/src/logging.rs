use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

const TUI_LOG_FILE_NAME: &str = "codex-tui.log";
const TUI_LOG_BACKUP_FILE_NAME: &str = "codex-tui.log.1";
const TUI_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

pub(crate) fn open_tui_log_writer(log_dir: &Path) -> io::Result<RotatingLogWriter> {
    RotatingLogWriter::new(log_dir, TUI_LOG_MAX_BYTES)
}

pub(crate) struct RotatingLogWriter {
    log_dir: PathBuf,
    file: File,
    current_len: u64,
    max_bytes: u64,
}

impl RotatingLogWriter {
    fn new(log_dir: &Path, max_bytes: u64) -> io::Result<Self> {
        fs::create_dir_all(log_dir)?;
        rotate_oversized_log(log_dir, max_bytes)?;
        let log_path = log_dir.join(TUI_LOG_FILE_NAME);
        let current_len = fs::metadata(&log_path).map_or(0, |metadata| metadata.len());
        let file = private_append_options().open(log_path)?;
        Ok(Self {
            log_dir: log_dir.to_path_buf(),
            file,
            current_len,
            max_bytes,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        rotate_log(&self.log_dir, self.max_bytes)?;
        self.file = private_append_options().open(self.log_dir.join(TUI_LOG_FILE_NAME))?;
        self.current_len = 0;
        Ok(())
    }
}

impl Write for RotatingLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let incoming_len = u64::try_from(buf.len()).unwrap_or(u64::MAX);
        if self.current_len.saturating_add(incoming_len) > self.max_bytes {
            self.rotate()?;
        }

        let max_bytes = usize::try_from(self.max_bytes).unwrap_or(usize::MAX);
        let bytes_to_write = if buf.len() > max_bytes {
            &buf[buf.len() - max_bytes..]
        } else {
            buf
        };
        self.file.write_all(bytes_to_write)?;
        self.current_len = self
            .current_len
            .saturating_add(u64::try_from(bytes_to_write.len()).unwrap_or(u64::MAX));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn rotate_oversized_log(log_dir: &Path, max_bytes: u64) -> io::Result<()> {
    let log_path = log_dir.join(TUI_LOG_FILE_NAME);
    let metadata = match fs::metadata(&log_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    if metadata.len() <= max_bytes {
        return Ok(());
    }

    rotate_log(log_dir, max_bytes)
}

fn rotate_log(log_dir: &Path, max_bytes: u64) -> io::Result<()> {
    let log_path = log_dir.join(TUI_LOG_FILE_NAME);
    let backup_path = log_dir.join(TUI_LOG_BACKUP_FILE_NAME);
    match fs::remove_file(&backup_path) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    let metadata = fs::metadata(&log_path)?;
    let keep_bytes = max_bytes.min(metadata.len());
    let mut source = File::open(&log_path)?;
    let seek_offset = i64::try_from(keep_bytes).unwrap_or(i64::MAX);
    source.seek(SeekFrom::End(-seek_offset))?;

    {
        let mut backup = private_truncate_options().open(&backup_path)?;
        let mut tail = source.take(keep_bytes);
        io::copy(&mut tail, &mut backup)?;
    }

    private_truncate_options().open(log_path)?;
    Ok(())
}

fn private_append_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    apply_private_mode(&mut options);
    options
}

fn private_truncate_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    apply_private_mode(&mut options);
    options
}

fn apply_private_mode(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[test]
    fn open_tui_log_writer_preserves_existing_log_under_limit() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let log_path = temp_dir.path().join(TUI_LOG_FILE_NAME);
        fs::write(&log_path, "old")?;

        let mut writer = RotatingLogWriter::new(temp_dir.path(), 8)?;
        writer.write_all(b"new")?;
        drop(writer);

        assert_eq!(fs::read_to_string(log_path)?, "oldnew");
        assert!(!temp_dir.path().join(TUI_LOG_BACKUP_FILE_NAME).exists());
        Ok(())
    }

    #[test]
    fn open_tui_log_writer_rotates_oversized_startup_log_to_bounded_tail() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let log_path = temp_dir.path().join(TUI_LOG_FILE_NAME);
        let backup_path = temp_dir.path().join(TUI_LOG_BACKUP_FILE_NAME);
        fs::write(&log_path, "0123456789abcdef")?;
        fs::write(&backup_path, "stale-backup")?;

        let mut writer = RotatingLogWriter::new(temp_dir.path(), 8)?;
        writer.write_all(b"fresh")?;
        drop(writer);

        assert_eq!(fs::read_to_string(backup_path)?, "89abcdef");
        assert_eq!(fs::read_to_string(log_path)?, "fresh");
        Ok(())
    }

    #[test]
    fn open_tui_log_writer_rotates_when_write_would_exceed_limit() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let log_path = temp_dir.path().join(TUI_LOG_FILE_NAME);
        let backup_path = temp_dir.path().join(TUI_LOG_BACKUP_FILE_NAME);

        let mut writer = RotatingLogWriter::new(temp_dir.path(), 8)?;
        writer.write_all(b"1234567")?;
        writer.write_all(b"89")?;
        drop(writer);

        assert_eq!(fs::read_to_string(backup_path)?, "1234567");
        assert_eq!(fs::read_to_string(log_path)?, "89");
        Ok(())
    }

    #[test]
    fn open_tui_log_writer_bounds_single_large_write() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let log_path = temp_dir.path().join(TUI_LOG_FILE_NAME);

        let mut writer = RotatingLogWriter::new(temp_dir.path(), 8)?;
        writer.write_all(b"0123456789abcdef")?;
        drop(writer);

        assert_eq!(fs::read_to_string(log_path)?, "89abcdef");
        Ok(())
    }
}
