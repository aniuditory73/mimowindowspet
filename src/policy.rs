use crate::domain::Command;

pub const MAX_NOTE_BYTES: usize = 2048;
pub const MAX_SEARCH_BYTES: usize = 256;

pub fn validate(command: &Command) -> Result<(), &'static str> {
    match command {
        Command::CheckResponsiveness => Ok(()),
        Command::CreateNote(text) if text.trim().is_empty() => Err("Введите текст заметки"),
        Command::CreateNote(text) if text.len() > MAX_NOTE_BYTES || text.contains('\0') => {
            Err("Сократите текст заметки до 2 КБ")
        }
        Command::FindNotes(query) if query.len() > MAX_SEARCH_BYTES || query.contains('\0') => {
            Err("Сократите поисковый запрос")
        }
        Command::CreateNote(_) | Command::FindNotes(_) => Ok(()),
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
