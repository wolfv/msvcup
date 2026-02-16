use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs;
use std::path::PathBuf;

pub struct LockFile {
    path: PathBuf,
    file: fs::File,
}

impl LockFile {
    pub fn lock(path: &str) -> Result<LockFile> {
        let path = PathBuf::from(path);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .with_context(|| format!("creating lock file directory '{}'", dir.display()))?;
        }
        let file = fs::File::create(&path)
            .with_context(|| format!("creating lock file '{}'", path.display()))?;
        file.lock_exclusive()
            .with_context(|| format!("locking file '{}'", path.display()))?;

        // Write PID to lock file for debugging
        let pid = std::process::id();
        use std::io::Write;
        let mut f = &file;
        let _ = write!(f, "{}", pid);
        let _ = f.flush();

        Ok(LockFile { path, file })
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        let _ = fs::remove_file(&self.path);
    }
}
