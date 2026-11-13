use crate::domain::Command;

// Only typed capabilities can reach this gate. Adding a command requires an explicit decision.
pub fn validate(command: Command) {
    match command {
        Command::CheckResponsiveness => (),
    }
}
