use crate::domain::Command;

pub const MAX_NOTE_BYTES: usize = 2048;
pub const MAX_SEARCH_BYTES: usize = 256;

pub fn validate(command: &Command) -> Result<(), &'static str> {
    match command {
        Command::CheckResponsiveness => Ok(()),
        Command::CreateNote(text) | Command::ReviseNote { text, .. } if text.trim().is_empty() => {
            Err("Введите текст заметки")
        }
        Command::CreateNote(text) | Command::ReviseNote { text, .. }
            if text.len() > MAX_NOTE_BYTES || text.contains('\0') =>
        {
            Err("Сократите текст заметки до 2 КБ")
        }
        Command::FindNotes(query) if query.len() > MAX_SEARCH_BYTES || query.contains('\0') => {
            Err("Сократите поисковый запрос")
        }
        Command::CreateNote(_)
        | Command::ReviseNote { .. }
        | Command::FindNotes(_)
        | Command::PreviewFiles(_)
        | Command::PreviewRename(_, _, _) => Ok(()),
        Command::MoveFiles(batch) | Command::UndoFiles(batch) if batch.moves.is_empty() => {
            Err("Нет файлов для операции")
        }
        Command::MoveFiles(_) | Command::UndoFiles(_) => Ok(()),
        Command::RunCodex(_, prompt)
            if prompt.trim().is_empty() || prompt.len() > 8192 || prompt.contains('\0') =>
        {
            Err("Введите запрос для Codex длиной до 8 КБ")
        }
        Command::CheckCodex | Command::RunCodex(_, _) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_note_and_search_sizes() {
        assert!(validate(&Command::CreateNote("  ".into())).is_err());
        assert!(validate(&Command::CreateNote("я".repeat(1025))).is_err());
        assert!(validate(&Command::CreateNote("Купить хлеб".into())).is_ok());
        assert!(validate(&Command::FindNotes(String::new())).is_ok());
        assert!(validate(&Command::FindNotes("x".repeat(257))).is_err());
    }
}
