use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::thread;

use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

use crate::domain::{Command, Corner, TaskState};
use crate::platform::windows;
use crate::storage::Store;
use crate::tasks::{self, Cancellation, Event, Runner, SubmitError};
use crate::ui::{self, MimoTray, MimoWindow};

enum Update {
    Task(Event),
    Settings(Result<Corner, ()>),
}

struct ActiveTask {
    id: u64,
    state: TaskState,
    progress: u8,
    cancel: Cancellation,
    cancelling: bool,
}

struct App {
    corner: Corner,
    save_corner: Option<Sender<Corner>>,
    runner: Runner,
    active: VecDeque<ActiveTask>,
    next_id: u64,
    last_result: Option<TaskState>,
}

impl App {
    fn submit(&mut self, window: &MimoWindow, command: Command) -> bool {
        if self.active.len() >= tasks::CAPACITY {
            window.set_message("Очередь заполнена".into());
            return false;
        }
        let id = self.next_id;
        match self.runner.submit(id, command) {
            Ok(cancel) => {
                self.next_id += 1;
                self.active.push_back(ActiveTask {
                    id,
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
            Update::Settings(result) => {
                window.set_saving(false);
                match result {
                    Ok(corner) => {
                        self.corner = corner;
                        window.set_selected_corner(ui::corner_index(corner));
                        window.set_settings_message("".into());
                        place_window(window, corner);
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
                    self.last_result = Some(event.state);
                    self.active.pop_front();
                }
                self.render_tasks(window);
            }
        }
    }

    fn render_tasks(&self, window: &MimoWindow) {
        window.set_can_submit(self.active.len() < tasks::CAPACITY);
        if let Some(task) = self.active.front() {
            window.set_task_name("Проверка отклика".into());
            let status = if task.cancelling {
                "Отмена запрошена"
            } else {
                ui::status_text(task.state)
            };
            window.set_task_status(status.into());
            window.set_can_cancel(!task.cancelling);
            window.set_show_progress(task.state == TaskState::Running);
            window.set_task_progress(f32::from(task.progress) / 100.0);
            let mut summary = format!("В очереди: {}", self.active.len() - 1);
            if let Some(last) = self.last_result {
                summary.push_str(&format!(" · Последняя: {}", ui::status_text(last)));
            }
            window.set_message(summary.into());
        } else {
            window.set_can_cancel(false);
            window.set_show_progress(false);
            if let Some(state) = self.last_result {
                window.set_task_name("Проверка отклика".into());
                window.set_task_status(ui::status_text(state).into());
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
    let runner = Runner::start({
        let publish = publish.clone();
        move |event| publish(Update::Task(event))
    })?;
    let (save_corner, settings_worker) = start_settings(publish)?;
    let app = Rc::new(RefCell::new(App {
        corner: Corner::default(),
        save_corner: Some(save_corner),
        runner,
        active: VecDeque::new(),
        next_id: 1,
        last_result: None,
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
    bind_shell(&window, &tray, &app);
    let event_loop = window
        .show()
        .and_then(|()| tray.show())
        .and_then(|()| slint::run_event_loop());

    {
        let mut app = app.borrow_mut();
        app.save_corner.take();
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
            window.set_message("Доступная команда: «проверка отклика»".into());
            return false;
        };
        state.borrow_mut().submit(&window, command)
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

fn bind_shell(window: &MimoWindow, tray: &MimoTray, app: &Rc<RefCell<App>>) {
    let weak = window.as_weak();
    let state = Rc::clone(app);
    window.on_toggle_panel(move || {
        if let Some(window) = weak.upgrade() {
            set_expanded(&window, !window.get_expanded(), state.borrow().corner);
        }
    });
    ui::bind_close(window);

    let weak = window.as_weak();
    let state = Rc::clone(app);
    tray.on_open_panel(move || {
        if let Some(window) = weak.upgrade()
            && window.show().is_ok()
        {
            set_expanded(&window, true, state.borrow().corner);
            window
                .window()
                .with_winit_window(|native| native.focus_window());
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
            if state
                .borrow()
                .save_corner
                .as_ref()
                .is_some_and(|sender| sender.send(corner).is_ok())
            {
                window.set_saving(true);
                window.set_settings_message("Сохранение…".into());
            } else {
                window.set_settings_message("Настройки недоступны".into());
            }
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
                    place_window(&window, state.borrow().corner);
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
}

fn place_window(window: &MimoWindow, corner: Corner) {
    if window.window().is_visible()
        && let Err(error) = windows::place_window(window.window(), corner, window.get_expanded())
    {
        eprintln!("MIMO window placement: {error}");
    }
}

fn quit() {
    let _ = slint::quit_event_loop();
}

fn start_settings(
    publish: impl Fn(Update) + Send + 'static,
) -> io::Result<(Sender<Corner>, thread::JoinHandle<()>)> {
    let (sender, requests) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("mimo-settings".into())
        .spawn(move || {
            publish(Update::Settings(
                Store::open().and_then(|store| store.load()).map_err(|_| ()),
            ));
            for corner in requests {
                let result = Store::open().and_then(|store| store.save(corner));
                publish(Update::Settings(result.map(|()| corner).map_err(|_| ())));
            }
        })?;
    Ok((sender, worker))
}
