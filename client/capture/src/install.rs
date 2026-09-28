use std::path::{Path, PathBuf};

pub const HOST_EXE: &str = "relay-capture.exe";

/// Cartella dove sta il programma di registrazione, dentro i dati dell'app.
pub fn runtime_dir(base: &Path) -> PathBuf {
    base.join("relay-capture")
}

pub fn host_exe(dir: &Path) -> PathBuf {
    dir.join(HOST_EXE)
}

/// Cartella da cui avviare il programma (accanto all'eseguibile).
pub fn spawn_dir(dir: &Path) -> PathBuf {
    dir.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_hang_off_the_data_dir() {
        let base = Path::new("/dati");
        let dir = runtime_dir(base);
        assert_eq!(dir, Path::new("/dati/relay-capture"));
        assert_eq!(
            host_exe(&dir),
            Path::new("/dati/relay-capture/relay-capture.exe")
        );
        assert_eq!(spawn_dir(&dir), dir);
    }
}
