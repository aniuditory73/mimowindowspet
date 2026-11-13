use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf, Prefix};

use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_RENAME_INFO, FILE_SHARE_READ, FileRenameInfo, GetDriveTypeW, GetFileInformationByHandle,
    SetFileInformationByHandle,
};

use crate::storage::{guard_directories, open_directory};

const MAX_SCAN: usize = 2000;
const MAX_PREVIEW: usize = 30;
const DELETE_ACCESS: u32 = 0x0001_0000;
const DRIVE_FIXED: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope(PathBuf);

impl Scope {
    pub fn approved(input: &str) -> Option<Self> {
        let path = Path::new(input.trim());
        let mut parts = path.components();
        let Component::Prefix(prefix) = parts.next()? else {
            return None;
        };
        let Prefix::Disk(drive) = prefix.kind() else {
            return None;
        };
        if parts.next()? != Component::RootDir {
            return None;
        }
        let mut normalized = PathBuf::from(format!("{}:\\", char::from(drive)));
        for part in parts {
            let Component::Normal(name) = part else {
                return None;
            };
            let name = name.to_str()?;
            if name.is_empty()
                || name.ends_with(['.', ' '])
                || name.contains([':', '\0', '*', '?', '"', '<', '>', '|'])
                || name.chars().any(char::is_control)
            {
                return None;
            }
            normalized.push(name);
        }
        (normalized.parent().is_some()
            && !input
                .split(['\\', '/'])
                .any(|part| part == "." || part == ".."))
        .then_some(Self(normalized))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn guard(&self) -> io::Result<Vec<File>> {
        let drive = self
            .0
            .ancestors()
            .filter(|path| path.has_root())
            .last()
            .ok_or(io::ErrorKind::InvalidInput)?;
        let drive: Vec<u16> = drive.as_os_str().encode_wide().chain([0]).collect();
        // The drive root is NUL-terminated; remote and removable drives are outside this scope.
        if unsafe { GetDriveTypeW(drive.as_ptr()) } != DRIVE_FIXED {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let guards = guard_directories(&self.0)?;
        let resolved = self.0.canonicalize()?;
        let expected = format!("\\\\?\\{}", self.0.display());
        if resolved.to_string_lossy().to_lowercase() != expected.to_lowercase() {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(guards)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    volume: u32,
    index: u64,
    length: u64,
    written: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Move {
    pub name: String,
    pub category: Option<&'static str>,
    pub destination_name: String,
    identity: Identity,
}

impl Move {
    pub fn destination_label(&self) -> String {
        match self.category {
            Some(category) => format!("{category}\\{}", self.destination_name),
            None => self.destination_name.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub scope: Scope,
    pub entries: Vec<String>,
    pub moves: Vec<Move>,
    pub total: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub scope: Scope,
    pub moves: Vec<Move>,
}

pub struct BatchResult {
    pub changed: Batch,
    pub remaining: Batch,
    pub stopped: bool,
    pub errors: usize,
    pub report: Vec<String>,
}

pub fn preview(scope: Scope, cancelled: &impl Fn() -> bool) -> io::Result<Preview> {
    let _guard = scope.guard()?;
    let mut result = Preview {
        scope,
        entries: Vec::new(),
        moves: Vec::new(),
        total: 0,
    };
    for (index, entry) in fs::read_dir(result.scope.path())?.enumerate() {
        if cancelled() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if index == MAX_SCAN {
            return Err(io::Error::other("directory exceeds scan limit"));
        }
        let entry = entry?;
        result.total += 1;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if result.entries.len() < MAX_PREVIEW {
            result.entries.push(name.clone());
        }
        if result.moves.len() == MAX_PREVIEW {
            continue;
        }
        let Some(category) = category(&entry.path()) else {
            continue;
        };
        if let Ok((_, identity)) = checked_file(&entry.path(), false) {
            result.moves.push(Move {
                destination_name: name.clone(),
                name,
                category: Some(category),
                identity,
            });
        }
    }
    result.entries.sort_unstable();
    result
        .moves
        .sort_unstable_by(|left, right| left.name.cmp(&right.name));
    Ok(result)
}

pub fn preview_rename(scope: Scope, source: &str, destination: &str) -> io::Result<Preview> {
    if !valid_filename(source)
        || !valid_filename(destination)
        || source.eq_ignore_ascii_case(destination)
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let _guard = scope.guard()?;
    let (_, identity) = checked_file(&scope.path().join(source), false)?;
    Ok(Preview {
        scope,
        entries: vec![source.to_owned()],
        total: 1,
        moves: vec![Move {
            name: source.into(),
            category: None,
            destination_name: destination.into(),
            identity,
        }],
    })
}

fn valid_filename(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|'])
        || name.chars().any(char::is_control)
    {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    })
}

pub fn execute(batch: Batch, undo: bool, cancelled: &impl Fn() -> bool) -> io::Result<BatchResult> {
    let _guard = batch.scope.guard()?;
    let mut result = BatchResult {
        changed: Batch {
            scope: batch.scope.clone(),
            moves: Vec::new(),
        },
        remaining: Batch {
            scope: batch.scope.clone(),
            moves: Vec::new(),
        },
        stopped: false,
        errors: 0,
        report: Vec::new(),
    };
    let mut moves = batch.moves.into_iter();
    for action in moves.by_ref() {
        if cancelled() {
            result.stopped = true;
            result.remaining.moves.push(action);
            break;
        }
        match move_one(batch.scope.path(), &action, undo) {
            Ok(()) => {
                result.report.push(format!(
                    "{} · {}",
                    action.name,
                    if undo {
                        "возвращён"
                    } else {
                        "перенесён"
                    }
                ));
                result.changed.moves.push(action);
            }
            Err(error) => {
                result.errors += 1;
                let reason = match error.kind() {
                    io::ErrorKind::AlreadyExists => "имя назначения занято",
                    io::ErrorKind::PermissionDenied => "доступ запрещён",
                    io::ErrorKind::NotFound => "файл или папка исчезли",
                    io::ErrorKind::ResourceBusy => "файл занят",
                    io::ErrorKind::InvalidData => "файл изменился после предпросмотра",
                    _ if error.raw_os_error() == Some(32) => "файл занят",
                    _ => "операция Windows отклонена",
                };
                result.report.push(format!("{} · {reason}", action.name));
                result.remaining.moves.push(action);
            }
        }
    }
    result.remaining.moves.extend(moves);
    Ok(result)
}

fn move_one(root: &Path, action: &Move, undo: bool) -> io::Result<()> {
    let (folder, _folder_guard) = match action.category {
        Some(category) => {
            let folder = root.join(category);
            if !undo {
                match fs::create_dir(&folder) {
                    Ok(()) => (),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
                    Err(error) => return Err(error),
                }
            }
            let guard = open_directory(&folder)?;
            (folder, Some(guard))
        }
        None => (root.to_owned(), None),
    };
    let original = root.join(&action.name);
    let organized = folder.join(&action.destination_name);
    let (from, to) = if undo {
        (organized, original)
    } else {
        (original, organized)
    };
    let (file, identity) = checked_file(&from, true)?;
    if identity != action.identity {
        return Err(io::ErrorKind::InvalidData.into());
    }
    rename_without_replacement(&file, &to)
}

fn checked_file(path: &Path, rename: bool) -> io::Result<(File, Identity)> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    if rename {
        options.access_mode(0x8000_0000 | DELETE_ACCESS);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // The file handle and output remain valid for this call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || info.nNumberOfLinks != 1
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok((
        file,
        Identity {
            volume: info.dwVolumeSerialNumber,
            index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            length: metadata.len(),
            written: metadata.last_write_time(),
        },
    ))
}

fn rename_without_replacement(file: &File, destination: &Path) -> io::Result<()> {
    let name: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let size = (offset + name.len() * size_of::<u16>()).max(size_of::<FILE_RENAME_INFO>());
    let mut buffer = vec![0usize; size.div_ceil(size_of::<usize>())];
    // The buffer is aligned, zero-initialized (no replace), and sized for the variable UTF-16 field.
    unsafe {
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        (*info).FileNameLength = ((name.len() - 1) * size_of::<u16>()) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            buffer.as_mut_ptr().cast::<u8>().add(offset).cast(),
            name.len(),
        );
        if SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfo,
            info.cast(),
            size as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn category(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => Some("Изображения"),
        "pdf" | "docx" | "xlsx" | "pptx" | "txt" | "odt" => Some("Документы"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_relative_network_and_traversal_paths() {
        for path in [
            r"C:\",
            r"C:relative",
            r"C:\x\..\y",
            r"C:\x\.",
            r"\\host\share\x",
            r"C:\x:stream",
        ] {
            assert!(Scope::approved(path).is_none(), "accepted {path}");
        }
        assert!(Scope::approved(r"C:\данные\папка").is_some());
    }

    #[test]
    fn rejects_a_link_as_an_approved_root_when_supported() {
        use std::os::windows::fs::symlink_dir;
        let outside = std::env::temp_dir().join(format!("MIMO-link-target-{}", std::process::id()));
        let link = outside.with_extension("link");
        fs::create_dir(&outside).unwrap();
        match symlink_dir(&outside, &link) {
            Ok(()) => {
                let scope = Scope::approved(link.to_str().unwrap()).unwrap();
                assert!(scope.guard().is_err());
                fs::remove_dir(link).unwrap();
            }
            Err(error) if matches!(error.raw_os_error(), Some(5 | 1314)) => {
                eprintln!("symlink creation unavailable")
            }
            Err(error) => panic!("symlink fixture failed: {error}"),
        }
        fs::remove_dir(outside).unwrap();
    }

    #[test]
    fn moves_and_undoes_unchanged_files_without_replacing_collisions() {
        let root = std::env::temp_dir().join(format!("MIMO-файлы-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let scope = Scope::approved(root.to_str().unwrap()).unwrap();
        let source = root.join("Фото.PNG");
        let document = root.join("Запись.txt");
        let locked = root.join("Блокировка.txt");
        fs::write(&source, "image").unwrap();
        fs::write(&document, "old").unwrap();
        fs::write(&locked, "locked").unwrap();
        let preview = preview(scope.clone(), &|| false).unwrap();
        assert_eq!(preview.moves.len(), 3);
        fs::write(&document, "changed since preview").unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)
            .unwrap();
        let category = root.join("Изображения");
        let destination = category.join("Фото.PNG");
        fs::create_dir(&category).unwrap();
        fs::write(&destination, "existing").unwrap();
        let result = execute(
            Batch {
                scope,
                moves: preview.moves,
            },
            false,
            &|| false,
        )
        .unwrap();
        assert_eq!(result.errors, 3);
        assert_eq!(fs::read_to_string(&destination).unwrap(), "existing");
        assert_eq!(fs::read_to_string(&source).unwrap(), "image");
        drop(lock);
        fs::remove_file(&destination).unwrap();
        let result = execute(result.remaining, false, &|| false).unwrap();
        assert_eq!(result.changed.moves.len(), 2);
        assert_eq!(result.errors, 1);
        assert!(!source.exists());
        let result = execute(result.changed, true, &|| false).unwrap();
        assert_eq!(result.errors, 0);
        let renamed = preview_rename(result.changed.scope, "Фото.PNG", "Кадр.PNG").unwrap();
        let result = execute(
            Batch {
                scope: renamed.scope,
                moves: renamed.moves,
            },
            false,
            &|| false,
        )
        .unwrap();
        assert_eq!(result.changed.moves.len(), 1);
        assert_eq!(fs::read_to_string(root.join("Кадр.PNG")).unwrap(), "image");
        let result = execute(result.changed, true, &|| false).unwrap();
        assert_eq!(result.changed.moves.len(), 1);
        assert_eq!(fs::read_to_string(&source).unwrap(), "image");
        assert_eq!(
            fs::read_to_string(&document).unwrap(),
            "changed since preview"
        );
        for path in [source, document, locked] {
            fs::remove_file(path).unwrap();
        }
        fs::remove_dir(category).unwrap();
        fs::remove_dir(root.join("Документы")).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
