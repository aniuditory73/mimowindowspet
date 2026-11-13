#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    CheckResponsiveness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    Queued,
    Validating,
    // Reserved for task-scoped approval in the next phase.
    #[allow(dead_code)]
    AwaitingApproval,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn can_transition_to(self, next: Self) -> bool {
        use TaskState::*;
        matches!(
            (self, next),
            (Queued, Validating | Cancelled | Failed)
                | (Validating, AwaitingApproval | Running | Failed | Cancelled)
                | (AwaitingApproval, Running | Failed | Cancelled)
                | (Running, Succeeded | Failed | Cancelled)
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub const ALL: [Self; 4] = [
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub corner: Corner,
    pub pinned: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            corner: Corner::default(),
            pinned: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TaskState::*;

    #[test]
    fn task_cannot_skip_validation_or_leave_terminal_state() {
        assert!(!Queued.can_transition_to(Running));
        assert!(!Validating.can_transition_to(Succeeded));
        assert!(Validating.can_transition_to(AwaitingApproval));
        assert!(AwaitingApproval.can_transition_to(Cancelled));
        for state in [Succeeded, Failed, Cancelled] {
            assert!(state.is_terminal());
            assert!(!state.can_transition_to(Running));
        }
    }
}
