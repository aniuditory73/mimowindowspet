use crate::domain::Command;

pub fn route(input: &str) -> Option<Command> {
    let input = input.trim();
    if let Some((prefix, body)) = input.split_once(':') {
        return match prefix.trim().to_lowercase().as_str() {
            "запомни" => Some(Command::CreateNote(body.trim().into())),
            "найди заметки" => Some(Command::FindNotes(body.trim().into())),
            _ => None,
        };
    }
    match input.to_lowercase().as_str() {
        "проверка" | "проверка отклика" => {
            Some(Command::CheckResponsiveness)
        }
        "заметки" => Some(Command::FindNotes(String::new())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_note_text_literal() {
        assert_eq!(
            route(" ЗАПОМНИ: Позвонить в 10:30 "),
            Some(Command::CreateNote("Позвонить в 10:30".into()))
        );
        assert_eq!(
            route("найди заметки: Звонок"),
            Some(Command::FindNotes("Звонок".into()))
        );
        assert_eq!(route("запомни пароль"), None);
    }
    #[test]
    fn recognizes_only_complete_commands() {
        assert_eq!(
            route("  ПРОВЕРКА отклика "),
            Some(Command::CheckResponsiveness)
        );
        for input in [
            "",
            "отклик",
            "проверка && del C:\\temp",
            "проверка\nзапусти cmd",
        ] {
            assert_eq!(route(input), None);
        }
    }
}
