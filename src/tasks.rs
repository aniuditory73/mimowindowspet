use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::domain::{Command, TaskState};
use crate::{notes, policy, storage::Store};

pub const CAPACITY: usize = 3;
const CHECK_STEPS: u8 = 8;

pub struct Event {
    pub id: u64,
    pub state: TaskState,
    pub progress: u8,
    pub output: Option<Output>,
}

pub enum Output {
    Error(&'static str),
    Notes(notes::Matches),
}

pub struct Cancellation(mpsc::Sender<()>);

impl Cancellation {
    pub fn request(&self) {
        let _ = self.0.send(());
    }
}

struct Job {
    id: u64,
    command: Command,
    cancel: Receiver<()>,
}

pub enum SubmitError {
    Full,
    Unavailable,
}

pub struct Runner {
    sender: Option<SyncSender<Job>>,
    worker: Option<JoinHandle<()>>,
}

impl Runner {
    pub fn start(publish: impl Fn(Event) + Send + 'static) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(CAPACITY);
        let worker = thread::Builder::new()
            .name("mimo-tasks".into())
            .spawn(move || {
                for job in receiver {
                    run(job, &publish);
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }

    pub fn submit(&self, id: u64, command: Command) -> Result<Cancellation, SubmitError> {
        let sender = self.sender.as_ref().ok_or(SubmitError::Unavailable)?;
        let (cancel, receiver) = mpsc::channel();
        match sender.try_send(Job {
            id,
            command,
            cancel: receiver,
        }) {
            Ok(()) => Ok(Cancellation(cancel)),
            Err(TrySendError::Full(_)) => Err(SubmitError::Full),
            Err(TrySendError::Disconnected(_)) => Err(SubmitError::Unavailable),
        }
    }

    pub fn shutdown(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(job: Job, publish: &impl Fn(Event)) {
    let send = |state, progress, output| {
        publish(Event {
            id: job.id,
            state,
            progress,
            output,
        })
    };
    let cancelled = || !matches!(job.cancel.try_recv(), Err(mpsc::TryRecvError::Empty));
    if cancelled() {
        send(TaskState::Cancelled, 0, None);
        return;
    }

    send(TaskState::Validating, 0, None);
    if let Err(message) = policy::validate(&job.command) {
        send(TaskState::Failed, 0, Some(Output::Error(message)));
        return;
    }
    if cancelled() {
        send(TaskState::Cancelled, 0, None);
        return;
    }

    send(TaskState::Running, 0, None);
    match job.command {
        Command::CheckResponsiveness => {
            check_responsiveness(&job.cancel, |state, progress| send(state, progress, None))
        }
        Command::CreateNote(text) => report_note_result(
            Store::open()
                .and_then(|store| notes::create(&store, &text, &cancelled))
                .map(|()| None),
            send,
        ),
        Command::FindNotes(query) => report_note_result(
            Store::open()
                .and_then(|store| notes::find(&store, &query, &cancelled))
                .map(|found| Some(Output::Notes(found))),
            send,
        ),
    }
}

fn report_note_result(
    result: io::Result<Option<Output>>,
    send: impl Fn(TaskState, u8, Option<Output>),
) {
    match result {
        Ok(output) => send(TaskState::Succeeded, 0, output),
        Err(error) if error.kind() == io::ErrorKind::Interrupted => {
            send(TaskState::Cancelled, 0, None)
        }
        Err(_) => send(
            TaskState::Failed,
            0,
            Some(Output::Error("Не удалось получить доступ к заметкам")),
        ),
    }
}

fn check_responsiveness(cancel: &Receiver<()>, send: impl Fn(TaskState, u8)) {
    let deadline = Instant::now() + Duration::from_secs(15);
    for step in 1..=CHECK_STEPS {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let wait = remaining.min(Duration::from_secs(1));
        let previous_progress = progress_after(step - 1);
        if !matches!(cancel.recv_timeout(wait), Err(RecvTimeoutError::Timeout)) {
            send(TaskState::Cancelled, previous_progress);
            return;
        }
        if Instant::now() >= deadline {
            send(TaskState::Failed, previous_progress);
            return;
        }
        let progress = progress_after(step);
        if step == CHECK_STEPS {
            send(TaskState::Succeeded, progress);
        } else {
            send(TaskState::Running, progress);
        }
    }
}

fn progress_after(step: u8) -> u8 {
    (u16::from(step) * 100 / u16::from(CHECK_STEPS)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancels_running_and_queued_jobs_with_one_worker() {
        let (events, receive) = mpsc::channel();
        let mut runner = Runner::start(move |event| {
            let _ = events.send(event);
        })
        .unwrap();
        let command = Command::CheckResponsiveness;
        let running = runner.submit(1, command.clone()).ok().unwrap();
        let next = || receive.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(next().state, TaskState::Validating);
        assert_eq!(next().state, TaskState::Running);

        let queued = runner.submit(2, command).ok().unwrap();
        queued.request();
        running.request();
        let first_terminal = loop {
            let event = next();
            if event.state.is_terminal() {
                break event;
            }
        };
        assert_eq!(
            (first_terminal.id, first_terminal.state),
            (1, TaskState::Cancelled)
        );
        let second = next();
        assert_eq!((second.id, second.state), (2, TaskState::Cancelled));
        runner.shutdown();
    }
}
