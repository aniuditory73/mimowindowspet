use crate::domain::Command;

pub fn route(input: &str) -> Option<Command> {
    match input.trim().to_lowercase().as_str() {
        "проверка" | "проверка отклика" => {
            Some(Command::CheckResponsiveness)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
