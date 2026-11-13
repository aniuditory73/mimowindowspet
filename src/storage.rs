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

use crate::domain::{Corner, Settings};
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

    pub fn record_task(
        &self,
        kind: crate::domain::TaskKind,
        state: crate::domain::TaskState,
    ) -> io::Result<()> {
        use std::time::{SystemTime, UNIX_EPOCH};
        let directory = self.root.join("history-v1");
        match fs::create_dir(&directory) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
        let _guard = open_directory(&directory)?;
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_micros();
        let sequence = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!("{time:020}-{}-{sequence}.txt", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(path)?;
        writeln!(file, "{kind:?} · {state:?}")?;
        file.sync_all()
    }

    pub fn history(&self) -> io::Result<Vec<String>> {
        let directory = self.root.join("history-v1");
        let _guard = match open_directory(&directory) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut paths = std::collections::BinaryHeap::new();
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "txt") {
                paths.push(std::cmp::Reverse(path));
                if paths.len() > 40 {
                    paths.pop();
                }
            }
        }
        let mut rows = Vec::new();
        for std::cmp::Reverse(path) in paths.into_sorted_vec() {
            let mut text = String::new();
            open_regular_file(&path)?
                .take(128)
                .read_to_string(&mut text)?;
            if let Some((kind, state)) = text.trim_end().split_once(" · ") {
                let kind = match kind {
                    "Check" => "Проверка отклика",
                    "CreateNote" => "Создание заметки",
                    "ReviseNote" => "Редактирование заметки",
                    "FindNotes" => "Поиск заметок",
                    "PreviewFiles" => "Просмотр папки",
                    "MoveFiles" => "Сортировка файлов",
                    "UndoFiles" => "Возврат файлов",
                    "CheckCodex" => "Проверка Codex CLI",
                    "RunCodex" => "Задача Codex",
                    _ => continue,
                };
                let state = match state {
                    "Succeeded" => "Выполнено",
                    "Failed" => "Ошибка",
                    "Cancelled" => "Отменено",
                    _ => continue,
                };
                rows.push(format!("{kind} · {state}"));
            }
        }
        Ok(rows)
    }

    pub fn notes_directory(&self, create: bool) -> io::Result<(PathBuf, File)> {
        let path = self.root.join("notes-v1");
        if create {
            match fs::create_dir(&path) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error),
            }
        }
        let directory = open_directory(&path)?;
        Ok((path, directory))
    }

    pub(crate) fn open_under(app_data: &Path) -> io::Result<Self> {
        let mut parents = guard_directories(app_data)?;
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

    pub fn load(&self) -> io::Result<Settings> {
        let file = match open_regular_file(&self.root.join(CONFIG)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
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
        parse_settings(&source)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unknown settings format"))
    }

    pub fn save(&self, settings: Settings) -> io::Result<()> {
        // Check the format before replacing the configuration, including unknown versions.
        self.load()?;
        let name = match settings.corner {
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
        let pinned = settings.pinned;
        let written = writeln!(file, "version=1\ncorner={name}\npinned={pinned}")
            .and_then(|()| file.sync_all());
        drop(file);
        let result = written.and_then(|()| fs::rename(&temp, self.root.join(CONFIG)));
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

fn parse_settings(source: &str) -> Option<Settings> {
    let mut lines = source.lines();
    if lines.next()? != "version=1" {
        return None;
    }
    let corner = match lines.next()? {
        "corner=top-left" => Corner::TopLeft,
        "corner=top-right" => Corner::TopRight,
        "corner=bottom-left" => Corner::BottomLeft,
        "corner=bottom-right" => Corner::BottomRight,
        _ => return None,
    };
    let pinned = match lines.next() {
        None | Some("pinned=true") => true,
        Some("pinned=false") => false,
        _ => return None,
    };
    lines
        .next()
        .is_none()
        .then_some(Settings { corner, pinned })
}

pub(crate) fn guard_directories(path: &Path) -> io::Result<Vec<File>> {
    // Hold each directory open without sharing deletion so it cannot be replaced mid-operation.
    path.ancestors()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .filter(|parent| parent.has_root())
        .map(open_directory)
        .collect()
}

pub(crate) fn open_directory(path: &Path) -> io::Result<File> {
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

pub(crate) fn open_regular_file(path: &Path) -> io::Result<File> {
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
            "unsafe data file",
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
            assert_eq!(store.load().unwrap(), Settings::default());
            store
                .save(Settings {
                    corner: Corner::BottomLeft,
                    pinned: true,
                })
                .unwrap();
            store
                .save(Settings {
                    corner: Corner::TopLeft,
                    pinned: false,
                })
                .unwrap();
        }
        {
            let store = Store::open_under(&base).unwrap();
            assert_eq!(
                store.load().unwrap(),
                Settings {
                    corner: Corner::TopLeft,
                    pinned: false
                }
            );
            fs::write(&config, "version=1\r\ncorner=bottom-right\r\n").unwrap();
            assert_eq!(
                store.load().unwrap(),
                Settings {
                    corner: Corner::BottomRight,
                    pinned: true
                }
            );
            fs::write(&config, "version=2\n").unwrap();
            assert!(store.save(Settings::default()).is_err());
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
