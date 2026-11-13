use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_DELETE_ON_CLOSE, MoveFileExW};

use crate::policy::MAX_NOTE_BYTES;
use crate::storage::{Store, open_regular_file};

const MAX_ENTRIES: usize = 2000;
pub const MAX_RESULTS: usize = 30;
static NEXT_NOTE: AtomicU32 = AtomicU32::new(0);

pub struct Matches {
    pub notes: Vec<Note>,
    pub truncated: bool,
}

pub struct Note {
    pub id: String,
    pub text: String,
}

pub fn revise(
    store: &Store,
    id: &str,
    expected: &str,
    new_text: &str,
    cancelled: &impl Fn() -> bool,
) -> io::Result<()> {
    if !is_note_name(id) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let (directory, _guard) = store.notes_directory(false)?;
    let lock_path = directory.join("edit.lock");
    let _editor = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(io::ErrorKind::ResourceBusy.into());
        }
        Err(error) => return Err(error),
    };
    let original = directory.join(id);
    let sequence = NEXT_NOTE.fetch_add(1, Ordering::Relaxed);
    let temporary = directory.join(format!("edit-{}-{sequence}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&temporary)?;
    let result = file
        .write_all(new_text.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    let result = result
        .and_then(|()| check_cancelled(cancelled))
        .and_then(|()| {
            let current = read_text(&original)?;
            if current != expected {
                return Err(io::ErrorKind::InvalidData.into());
            }
            fs::rename(&temporary, &original)
        });
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn create(store: &Store, text: &str, cancelled: &impl Fn() -> bool) -> io::Result<()> {
    check_cancelled(cancelled)?;
    let (directory, _guard) = store.notes_directory(true)?;
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?;
    let process = std::process::id();
    let sequence = NEXT_NOTE.fetch_add(1, Ordering::Relaxed);
    let id = format!("{:020}-{process:08x}-{sequence:08x}", time.as_micros());
    let temp = directory.join(format!("{id}.tmp"));
    let destination = directory.join(format!("{id}.txt"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&temp)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    let result = written
        .and_then(|()| check_cancelled(cancelled))
        .and_then(|()| publish(&temp, &destination));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn find(store: &Store, query: &str, cancelled: &impl Fn() -> bool) -> io::Result<Matches> {
    let (directory, _guard) = match store.notes_directory(false) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Matches {
                notes: Vec::new(),
                truncated: false,
            });
        }
        Err(error) => return Err(error),
    };
    let mut paths = Vec::new();
    for (index, entry) in fs::read_dir(directory)?.enumerate() {
        check_cancelled(cancelled)?;
        if index == MAX_ENTRIES {
            return Err(io::Error::other("notes directory exceeds the entry limit"));
        }
        let entry = entry?;
        if is_note_name(&entry.file_name().to_string_lossy()) {
            paths.push(entry.path());
        }
    }
    paths.sort_unstable_by(|a, b| b.cmp(a));
    let query = query.to_lowercase();
    let mut notes = Vec::new();
    for path in paths {
        check_cancelled(cancelled)?;
        let text = read_text(&path)?;
        if query.is_empty() || text.to_lowercase().contains(&query) {
            if notes.len() == MAX_RESULTS {
                return Ok(Matches {
                    notes,
                    truncated: true,
                });
            }
            let id = path
                .file_name()
                .ok_or(io::ErrorKind::InvalidData)?
                .to_string_lossy()
                .into_owned();
            notes.push(Note { id, text });
        }
    }
    Ok(Matches {
        notes,
        truncated: false,
    })
}

fn read_text(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    open_regular_file(path)?
        .take(MAX_NOTE_BYTES as u64 + 1)
        .read_to_string(&mut text)?;
    if text.len() > MAX_NOTE_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(text)
}

fn is_note_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 42
        && bytes[20] == b'-'
        && bytes[29] == b'-'
        && &bytes[38..] == b".txt"
        && bytes[..20].iter().all(u8::is_ascii_digit)
        && bytes[21..29]
            .iter()
            .chain(&bytes[30..38])
            .all(u8::is_ascii_hexdigit)
}

fn check_cancelled(cancelled: &impl Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        Err(io::ErrorKind::Interrupted.into())
    } else {
        Ok(())
    }
}

fn publish(from: &Path, to: &Path) -> io::Result<()> {
    let from: Vec<u16> = from.as_os_str().encode_wide().chain([0]).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain([0]).collect();
    // Both paths are NUL-terminated and in the guarded notes directory. Zero flags reject replacement.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_searches_unicode_and_cancels_before_publication() {
        let base = std::env::temp_dir().join(format!("MIMO-заметки-{}", std::process::id()));
        fs::create_dir(&base).unwrap();
        {
            let store = Store::open_under(&base).unwrap();
            create(&store, "Купить хлеб", &|| false).unwrap();
            create(&store, "Позвонить в 10:30", &|| false).unwrap();
            assert_eq!(
                create(&store, "отмена", &|| true).unwrap_err().kind(),
                io::ErrorKind::Interrupted
            );
        }
        {
            let store = Store::open_under(&base).unwrap();
            assert_eq!(
                find(&store, "ХЛЕБ", &|| false).unwrap().notes[0].text,
                "Купить хлеб"
            );
            let found = find(&store, "", &|| false).unwrap();
            assert_eq!(
                found
                    .notes
                    .iter()
                    .map(|note| note.text.as_str())
                    .collect::<Vec<_>>(),
                ["Позвонить в 10:30", "Купить хлеб"]
            );
            assert!(!found.truncated);
            assert!(find(&store, "отмена", &|| false).unwrap().notes.is_empty());
            let note = &found.notes[0];
            revise(&store, &note.id, &note.text, "Позвонить позже", &|| false).unwrap();
            assert!(
                revise(
                    &store,
                    &note.id,
                    &note.text,
                    "Устаревшее изменение",
                    &|| false
                )
                .is_err()
            );
            assert_eq!(
                find(&store, "позже", &|| false).unwrap().notes[0].text,
                "Позвонить позже"
            );
            let (directory, guard) = store.notes_directory(false).unwrap();
            let paths: Vec<_> = fs::read_dir(&directory)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            assert_eq!(paths.len(), 2);
            assert_eq!(
                publish(&paths[0], &paths[1]).unwrap_err().kind(),
                io::ErrorKind::AlreadyExists
            );
            for path in paths {
                fs::remove_file(path).unwrap();
            }
            drop(guard);
            fs::remove_dir(directory).unwrap();
        }
        fs::remove_dir(base.join("MIMO")).unwrap();
        fs::remove_dir(base).unwrap();
    }
}
