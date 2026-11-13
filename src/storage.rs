use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
};

use crate::domain::Corner;
use crate::platform::windows::local_app_data;

const CONFIG: &str = "settings-v1.txt";
const MAX_BYTES: u64 = 128;
static TEMP_ID: AtomicU32 = AtomicU32::new(0);

pub struct Store {
    root: PathBuf,
    _parents: Vec<File>,
}

impl Store {
    pub fn open() -> io::Result<Self> {
        Self::open_under(&local_app_data()?)
    }

    fn open_under(app_data: &Path) -> io::Result<Self> {
        // Hold each directory open without sharing deletion so it cannot be replaced mid-operation.
        let mut parents = Vec::new();
        for parent in app_data.ancestors().collect::<Vec<_>>().into_iter().rev() {
            if parent.has_root() {
                parents.push(open_directory(parent)?);
            }
        }
        let app_data = app_data.canonicalize()?;
        let root = app_data.join("MIMO");
        match fs::create_dir(&root) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
        parents.push(open_directory(&root)?);
        Ok(Self {
            root,
            _parents: parents,
        })
    }

    pub fn load(&self) -> io::Result<Corner> {
        let file = match open_config(&self.root.join(CONFIG)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Corner::default()),
            Err(error) => return Err(error),
        };
        let mut source = String::new();
        file.take(MAX_BYTES + 1).read_to_string(&mut source)?;
        if source.len() as u64 > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "settings are too large",
            ));
        }
        let mut lines = source.lines();
        match (lines.next(), lines.next(), lines.next()) {
            (Some("version=1"), Some("corner=top-left"), None) => Ok(Corner::TopLeft),
            (Some("version=1"), Some("corner=top-right"), None) => Ok(Corner::TopRight),
            (Some("version=1"), Some("corner=bottom-left"), None) => Ok(Corner::BottomLeft),
            (Some("version=1"), Some("corner=bottom-right"), None) => Ok(Corner::BottomRight),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown settings format",
            )),
        }
    }

    pub fn save(&self, corner: Corner) -> io::Result<()> {
        // Check the format before replacing the configuration, including unknown versions.
        self.load()?;
        let name = match corner {
            Corner::TopLeft => "top-left",
            Corner::TopRight => "top-right",
            Corner::BottomLeft => "bottom-left",
            Corner::BottomRight => "bottom-right",
        };
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let temp = self
            .root
            .join(format!("settings-{}-{id}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(&temp)?;
        let written = writeln!(file, "version=1\ncorner={name}").and_then(|()| file.sync_all());
        drop(file);
        let result = written.and_then(|()| fs::rename(&temp, self.root.join(CONFIG)));
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

fn open_directory(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe settings directory",
        ));
    }
    Ok(file)
}

fn open_config(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    let mut details = BY_HANDLE_FILE_INFORMATION::default();
    // The output points to initialized storage and the file stays open for the call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut details) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || details.nNumberOfLinks != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe settings file",
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_corner_and_preserves_unknown_format_and_hard_links() {
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("MIMO-тест-{}-{id}", std::process::id()));
        fs::create_dir(&base).unwrap();
        let config = base.join("MIMO").join(CONFIG);
        let other = base.join("outside.txt");
        {
            let store = Store::open_under(&base).unwrap();
            assert_eq!(store.load().unwrap(), Corner::TopRight);
            store.save(Corner::BottomLeft).unwrap();
            store.save(Corner::TopLeft).unwrap();
        }
        {
            let store = Store::open_under(&base).unwrap();
            assert_eq!(store.load().unwrap(), Corner::TopLeft);
            fs::write(&config, "version=1\r\ncorner=bottom-right\r\n").unwrap();
            assert_eq!(store.load().unwrap(), Corner::BottomRight);
            fs::write(&config, "version=2\n").unwrap();
            assert!(store.save(Corner::TopRight).is_err());
            assert_eq!(fs::read_to_string(&config).unwrap(), "version=2\n");
            fs::remove_file(&config).unwrap();
            fs::write(&other, "version=1\ncorner=top-left\n").unwrap();
            fs::hard_link(&other, &config).unwrap();
            assert_eq!(
                store.load().unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
        }
        fs::remove_file(config).unwrap();
        fs::remove_file(other).unwrap();
        fs::remove_dir(base.join("MIMO")).unwrap();
        fs::remove_dir(base).unwrap();
    }
}
