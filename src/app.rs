use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

use crate::domain::{Command, Corner, Settings, TaskKind, TaskState};
use crate::files::{Batch, Preview, Scope};
use crate::platform::hotkey::Hotkey;
use crate::platform::windows;
use crate::storage::Store;
use crate::tasks::{self, Cancellation, Event, Output, Runner, SubmitError};
use crate::ui::{self, MimoTray, MimoWindow};

enum Update {
    Task(Event),
    Settings(Result<Settings, ()>),
    History(Result<Vec<String>, ()>),
}

enum StorageRequest {
    Save(Settings),
    Record(TaskKind, TaskState),
    History,
}

struct ActiveTask {
    id: u64,
    kind: TaskKind,
    state: TaskState,
    progress: u8,
    cancel: Cancellation,
    cancelling: bool,
}

struct LastResult {
    kind: TaskKind,
    status: &'static str,
}

struct App {
    settings: Settings,
    save_settings: Option<Sender<StorageRequest>>,
    runner: Runner,
    active: VecDeque<ActiveTask>,
    next_id: u64,
    last_result: Option<LastResult>,
    preview: Option<Preview>,
    undo: Option<Batch>,
}

impl App {
    fn submit(&mut self, window: &MimoWindow, command: Command) -> bool {
        if self.active.len() >= tasks::CAPACITY {
            window.set_message("Очередь заполнена".into());
            return false;
        }
        let id = self.next_id;
        let kind = command.kind();
        match self.runner.submit(id, command) {
            Ok(cancel) => {
                self.next_id += 1;
                self.active.push_back(ActiveTask {
                    id,
                    kind,
                    cancel,
                    state: TaskState::Queued,
                    progress: 0,
                    cancelling: false,
                });
                self.render_tasks(window);
                true
            }
            Err(error) => {
                let message = match error {
                    SubmitError::Full => "Очередь заполнена",
                    SubmitError::Unavailable => "Исполнитель недоступен. Перезапустите MIMO.",
                };
                window.set_message(message.into());
                false
            }
        }
    }

    fn update(&mut self, window: &MimoWindow, update: Update) {
        match update {
            Update::History(result) => match result {
                Ok(rows) => ui::show_history(window, rows),
                Err(()) => window.set_history_message("Журнал недоступен".into()),
            },
            Update::Settings(result) => {
                window.set_saving(false);
                match result {
                    Ok(settings) => {
                        self.settings = settings;
                        window.set_selected_corner(ui::corner_index(settings.corner));
                        window.set_pinned(settings.pinned);
                        window.set_settings_message("".into());
                        place_window(window, settings.corner);
                        update_window_pin(window);
                    }
                    Err(()) => window.set_settings_message("Настройки недоступны".into()),
                }
            }
            Update::Task(event) => {
                let Some(task) = self.active.front_mut().filter(|task| task.id == event.id) else {
                    return;
                };
                if task.state != event.state && !task.state.can_transition_to(event.state) {
                    return;
                }
                task.state = event.state;
                task.progress = event.progress;
                if event.state.is_terminal() {
                    let status = match event.output {
                        Some(Output::Error(message)) => {
                            if matches!(
                                task.kind,
                                TaskKind::PreviewFiles | TaskKind::MoveFiles | TaskKind::UndoFiles
                            ) {
                                window.set_files_message(message.into());
                            }
                            if matches!(task.kind, TaskKind::CheckCodex | TaskKind::RunCodex) {
                                window.set_codex_message(message.into());
                            }
                            if task.kind == TaskKind::ReviseNote {
                                window.set_notes_summary(message.into());
                                window.set_note_editing(true);
                            }
                            message
                        }
                        Some(Output::Notes(found)) => {
                            ui::show_notes(window, found);
                            "Поиск завершён"
                        }
                        Some(Output::RevisedNote(text)) => {
                            window.set_note_text(text.into());
                            window.set_note_editing(false);
                            window.set_note_id("".into());
                            window.set_notes_summary(
                                "Заметка сохранена · обновите список поиском".into(),
                            );
                            window.set_notes(Default::default());
                            "Заметка сохранена"
                        }
                        Some(Output::Files(preview)) => {
                            ui::show_files(window, &preview);
                            self.preview = Some(preview);
                            "Просмотр завершён"
                        }
                        Some(Output::FileChanges(result, undo)) => {
                            let changed = result.changed.moves.len();
                            let failed = result.errors;
                            let remaining = result.remaining.moves.len();
                            window.set_files_root(
                                result.changed.scope.path().display().to_string().into(),
                            );
                            ui::show_file_rows(window, result.report);
                            if undo {
                                self.undo = (remaining > 0).then_some(result.remaining);
                            } else if changed > 0 {
                                self.undo = Some(result.changed);
                            }
                            window.set_can_undo(self.undo.is_some());
                            window.set_files_message(
                                format!(
                                    "Изменено: {changed} · Ошибок: {failed} · Осталось: {remaining}"
                                )
                                .into(),
                            );
                            if result.stopped {
                                "Операция остановлена"
                            } else if failed > 0 {
                                "Часть файлов пропущена"
                            } else {
                                "Операция выполнена"
                            }
                        }
                        Some(Output::Codex(output)) => {
                            window.set_codex_output(output.into());
                            window.set_codex_message(
                                if task.kind == TaskKind::RunCodex {
                                    "Ответ Codex"
                                } else {
                                    "Проверка CLI завершена"
                                }
                                .into(),
                            );
                            "Выполнено"
                        }
                        None => {
                            let status = ui::task_status(task.kind, event.state);
                            if matches!(
                                task.kind,
                                TaskKind::PreviewFiles | TaskKind::MoveFiles | TaskKind::UndoFiles
                            ) {
                                window.set_files_message(status.into());
                            }
                            if matches!(task.kind, TaskKind::CheckCodex | TaskKind::RunCodex) {
                                window.set_codex_message(status.into());
                            }
                            status
                        }
                    };
                    self.last_result = Some(LastResult {
                        kind: task.kind,
                        status,
                    });
                    self.active.pop_front();
                }
                self.render_tasks(window);
                window.set_files_busy(self.active.iter().any(|task| {
                    matches!(
                        task.kind,
                        TaskKind::PreviewFiles | TaskKind::MoveFiles | TaskKind::UndoFiles
                    )
                }));
                window.set_codex_busy(
                    self.active
                        .iter()
                        .any(|task| matches!(task.kind, TaskKind::CheckCodex | TaskKind::RunCodex)),
                );
                window.set_note_saving(
                    self.active
                        .iter()
                        .any(|task| task.kind == TaskKind::ReviseNote),
                );
            }
        }
    }

    fn save_settings(&self, window: &MimoWindow, settings: Settings) {
        if self
            .save_settings
            .as_ref()
            .is_some_and(|sender| sender.send(StorageRequest::Save(settings)).is_ok())
        {
            window.set_saving(true);
            window.set_settings_message("Сохранение…".into());
        } else {
            window.set_settings_message("Настройки недоступны".into());
        }
    }

    fn render_tasks(&self, window: &MimoWindow) {
        window.set_can_submit(self.active.len() < tasks::CAPACITY);
        if let Some(task) = self.active.front() {
            window.set_task_name(ui::task_name(task.kind).into());
            let status = if task.cancelling {
                "Отмена запрошена"
            } else {
                ui::task_status(task.kind, task.state)
            };
            window.set_task_status(status.into());
            window.set_can_cancel(!task.cancelling);
            window.set_show_progress(
                task.kind == TaskKind::Check && task.state == TaskState::Running,
            );
            window.set_task_progress(f32::from(task.progress) / 100.0);
            let mut summary = format!("В очереди: {}", self.active.len() - 1);
            if let Some(last) = &self.last_result {
                summary.push_str(&format!(" · {}", last.status));
            }
            window.set_message(summary.into());
        } else {
            window.set_can_cancel(false);
            window.set_show_progress(false);
            if let Some(last) = &self.last_result {
                window.set_task_name(ui::task_name(last.kind).into());
                window.set_task_status(last.status.into());
            } else {
                window.set_task_name("Нет активных задач".into());
                window.set_task_status("".into());
            }
            window.set_message("".into());
        }
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .with_winit_window_attributes_hook(|attributes| {
            attributes.with_active(false).with_resizable(false)
        })
        .select()?;

    let window = MimoWindow::new()?;
    let tray = MimoTray::new()?;
    ui::tray_icon(&tray, &window);
    ui::set_corner_options(&window);

    let (updates, receive_update) = mpsc::channel();
    let weak = window.as_weak();
    let publish = move |event| {
        if updates.send(event).is_ok() {
            let _ = weak.upgrade_in_event_loop(|window| window.invoke_refresh());
        }
    };
    let (save_settings, settings_worker) = start_settings(publish.clone())?;
    let runner = Runner::start({
        let publish = publish.clone();
        let audit = save_settings.clone();
        move |event| {
            if event.state.is_terminal() {
                let _ = audit.send(StorageRequest::Record(event.kind, event.state));
            }
            publish(Update::Task(event));
        }
    })?;
    let app = Rc::new(RefCell::new(App {
        settings: Settings::default(),
        save_settings: Some(save_settings),
        runner,
        active: VecDeque::new(),
        next_id: 1,
        last_result: None,
        preview: None,
        undo: None,
    }));
    app.borrow().render_tasks(&window);
    window.set_saving(true);
    window.set_selected_corner(ui::corner_index(Corner::default()));
    window.set_settings_message("Загрузка настроек…".into());

    let weak = window.as_weak();
    let state = Rc::clone(&app);
    window.on_refresh(move || {
        if let Some(window) = weak.upgrade() {
            for update in receive_update.try_iter() {
                state.borrow_mut().update(&window, update);
            }
        }
    });

    bind_tasks(&window, &app);
    bind_files(&window, &app);
    bind_codex(&window, &app);
    let weak = window.as_weak();
    let state = Rc::clone(&app);
    window.on_load_history(move || {
        if let Some(window) = weak.upgrade() {
            let requested = state
                .borrow()
                .save_settings
                .as_ref()
                .is_some_and(|sender| sender.send(StorageRequest::History).is_ok());
            window.set_history_message(
                if requested {
                    "Загрузка истории…"
                } else {
                    "Журнал недоступен"
                }
                .into(),
            );
        }
    });
    bind_shell(&window, &tray, &app);
    let weak = window.as_weak();
    let hotkey = Hotkey::start(move || {
        let _ = weak.upgrade_in_event_loop(|window| window.invoke_show_panel());
    });
    window.set_hotkey_message(
        if hotkey.is_ok() {
            "Ctrl+Alt+M — открыть MIMO"
        } else {
            "Ctrl+Alt+M недоступна"
        }
        .into(),
    );
    let pin_timer = slint::Timer::default();
    let weak = window.as_weak();
    pin_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            if let Some(window) = weak.upgrade()
                && window.get_pinned()
            {
                update_window_pin(&window);
            }
        },
    );
    let event_loop = window
        .show()
        .and_then(|()| tray.show())
        .and_then(|()| slint::run_event_loop());
    pin_timer.stop();
    drop(hotkey);

    {
        let mut app = app.borrow_mut();
        app.save_settings.take();
        for task in &app.active {
            task.cancel.request();
        }
        app.runner.shutdown();
    }
    drop(tray);
    drop(window);
    drop(app);
    let _ = settings_worker.join();
    event_loop?;
    Ok(())
}

fn bind_tasks(window: &MimoWindow, app: &Rc<RefCell<App>>) {
    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_submit_command(move |input| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        let Some(command) = crate::router::route(&input) else {
            window.set_message("Команды: проверка, заметки, запомни: текст".into());
            return false;
        };
        state.borrow_mut().submit(&window, command)
    });

    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_find_notes(move |query| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .submit(&window, Command::FindNotes(query.trim().into()));
        }
    });
    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_revise_note(move |id, expected, text| {
        if let Some(window) = weak.upgrade() {
            let submitted = state.borrow_mut().submit(
                &window,
                Command::ReviseNote {
                    id: id.into(),
                    expected: expected.into(),
                    text: text.trim().into(),
                },
            );
            if submitted {
                window.set_note_saving(true);
            }
        }
    });

    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_run_check(move || {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .submit(&window, Command::CheckResponsiveness);
        }
    });

    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_cancel_task(move || {
        if let Some(window) = weak.upgrade() {
            let mut state = state.borrow_mut();
            if let Some(task) = state.active.front_mut() {
                task.cancel.request();
                task.cancelling = true;
                state.render_tasks(&window);
            }
        }
    });
}

fn bind_files(window: &MimoWindow, app: &Rc<RefCell<App>>) {
    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_preview_files(move |path| {
        let Some(window) = weak.upgrade() else { return };
        let Some(scope) = Scope::approved(&path) else {
            window.set_files_message("Введите абсолютный путь к папке на локальном диске".into());
            return;
        };
        let mut state = state.borrow_mut();
        if state.submit(&window, Command::PreviewFiles(scope.clone())) {
            state.preview = None;
            window.set_can_move(false);
            window.set_file_rows(Default::default());
            window.set_files_root(scope.path().display().to_string().into());
            window.set_files_busy(true);
            window.set_files_message("Просмотр папки…".into());
        } else {
            window.set_files_message(window.get_message());
        }
    });
    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_preview_rename(move |path, source, destination| {
        let Some(window) = weak.upgrade() else { return };
        let Some(scope) = Scope::approved(&path) else {
            window.set_files_message("Введите абсолютный путь к папке на локальном диске".into());
            return;
        };
        let mut state = state.borrow_mut();
        if !window.get_files_busy()
            && state.submit(
                &window,
                Command::PreviewRename(scope.clone(), source.into(), destination.into()),
            )
        {
            state.preview = None;
            window.set_can_move(false);
            window.set_file_rows(Default::default());
            window.set_files_root(scope.path().display().to_string().into());
            window.set_files_busy(true);
            window.set_files_message("Проверка имени файла…".into());
        }
    });
    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_apply_moves(move || {
        let Some(window) = weak.upgrade() else { return };
        let mut state = state.borrow_mut();
        let Some(preview) = state.preview.as_ref() else {
            return;
        };
        let batch = Batch {
            scope: preview.scope.clone(),
            moves: preview.moves.clone(),
        };
        if !window.get_files_busy() && state.submit(&window, Command::MoveFiles(batch)) {
            state.preview = None;
            window.set_can_move(false);
            window.set_files_busy(true);
            window.set_files_message("Перенос файлов…".into());
        }
    });
    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_undo_moves(move || {
        let Some(window) = weak.upgrade() else { return };
        let mut state = state.borrow_mut();
        let Some(batch) = state.undo.clone() else {
            return;
        };
        if !window.get_files_busy() && state.submit(&window, Command::UndoFiles(batch)) {
            state.preview = None;
            window.set_can_move(false);
            window.set_files_busy(true);
            window.set_files_message("Возврат файлов…".into());
        }
    });
}

fn bind_codex(window: &MimoWindow, app: &Rc<RefCell<App>>) {
    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_check_codex(move || {
        if let Some(window) = weak.upgrade()
            && !window.get_codex_busy()
            && state.borrow_mut().submit(&window, Command::CheckCodex)
        {
            window.set_codex_busy(true);
            window.set_codex_output("".into());
            window.set_codex_message("Проверка установки…".into());
        }
    });
    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_run_codex(move |path, prompt| {
        let Some(window) = weak.upgrade() else { return };
        let Some(scope) = Scope::approved(&path) else {
            window.set_codex_message("Укажите абсолютный путь к локальной папке проекта".into());
            return;
        };
        if !window.get_codex_busy()
            && state
                .borrow_mut()
                .submit(&window, Command::RunCodex(scope, prompt.trim().into()))
        {
            window.set_codex_busy(true);
            window.set_codex_output("".into());
            window.set_codex_message("Задача Codex в очереди…".into());
        }
    });
}

fn bind_shell(window: &MimoWindow, tray: &MimoTray, app: &Rc<RefCell<App>>) {
    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_toggle_panel(move || {
        if let Some(window) = weak.upgrade() {
            set_expanded(
                &window,
                !window.get_expanded(),
                state.borrow().settings.corner,
            );
        }
    });
    ui::bind_close(window);

    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_show_panel(move || {
        if let Some(window) = weak.upgrade()
            && window.show().is_ok()
        {
            set_expanded(&window, true, state.borrow().settings.corner);
            window
                .window()
                .with_winit_window(|native| native.focus_window());
        }
    });
    let weak = window.as_weak();
    tray.on_open_panel(move || {
        if let Some(window) = weak.upgrade() {
            window.invoke_show_panel();
        }
    });
    let weak = window.as_weak();
    tray.on_hide_window(move || {
        if let Some(window) = weak.upgrade() {
            let _ = window.hide();
        }
    });
    tray.on_exit_app(quit);
    window.on_exit_app(quit);

    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_select_corner(move |index| {
        if let Some(window) = weak.upgrade()
            && !window.get_saving()
            && let Some(&corner) = Corner::ALL.get(index as usize)
        {
            let state = state.borrow();
            state.save_settings(
                &window,
                Settings {
                    corner,
                    ..state.settings
                },
            );
        }
    });

    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_toggle_pin(move || {
        if let Some(window) = weak.upgrade()
            && !window.get_saving()
        {
            let state = state.borrow();
            state.save_settings(
                &window,
                Settings {
                    pinned: !state.settings.pinned,
                    ..state.settings
                },
            );
        }
    });

    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.window().on_winit_window_event(move |_, event| {
        if matches!(
            event,
            winit::event::WindowEvent::Resized(_)
                | winit::event::WindowEvent::ScaleFactorChanged { .. }
        ) {
            let weak = weak.clone();
            let state = Rc::clone(&state);
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                if let Some(window) = weak.upgrade() {
                    place_window(&window, state.borrow().settings.corner);
                    update_window_pin(&window);
                }
            });
        }
        EventResult::Propagate
    });
}

fn set_expanded(window: &MimoWindow, expanded: bool, corner: Corner) {
    window.set_expanded(expanded);
    let (width, height) = if expanded {
        (420.0, 286.0)
    } else {
        (100.0, 42.0)
    };
    window
        .window()
        .set_size(slint::LogicalSize::new(width, height));
    place_window(window, corner);
    update_window_pin(window);
}

fn place_window(window: &MimoWindow, corner: Corner) {
    if window.window().is_visible()
        && let Err(error) = windows::place_window(window.window(), corner, window.get_expanded())
    {
        eprintln!("MIMO window placement: {error}");
    }
}

fn update_window_pin(window: &MimoWindow) {
    if window.window().is_visible() {
        let _ = windows::update_window_pin(window.window(), window.get_pinned());
    }
}

fn quit() {
    let _ = slint::quit_event_loop();
}

fn start_settings(
    publish: impl Fn(Update) + Send + 'static,
) -> io::Result<(Sender<StorageRequest>, thread::JoinHandle<()>)> {
    let (sender, requests) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("mimo-settings".into())
        .spawn(move || {
            publish(Update::Settings(
                Store::open().and_then(|store| store.load()).map_err(|_| ()),
            ));
            for request in requests {
                match request {
                    StorageRequest::Save(settings) => {
                        let result = Store::open().and_then(|store| store.save(settings));
                        publish(Update::Settings(result.map(|()| settings).map_err(|_| ())));
                    }
                    StorageRequest::Record(kind, state) => {
                        if Store::open()
                            .and_then(|store| store.record_task(kind, state))
                            .is_err()
                        {
                            eprintln!("MIMO could not record a task result");
                        }
                    }
                    StorageRequest::History => publish(Update::History(
                        Store::open()
                            .and_then(|store| store.history())
                            .map_err(|_| ()),
                    )),
                }
            }
        })?;
    Ok((sender, worker))
}
