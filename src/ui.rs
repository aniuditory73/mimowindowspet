use std::rc::Rc;

use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::domain::{Corner, TaskKind, TaskState};
use crate::notes::Matches;

slint::include_modules!();

pub fn set_corner_options(window: &MimoWindow) {
    let labels = [
        "Сверху слева",
        "Сверху справа",
        "Снизу слева",
        "Снизу справа",
    ];
    window.set_corners(Rc::new(VecModel::from(labels.map(Into::into).to_vec())).into());
}

pub fn corner_index(corner: Corner) -> i32 {
    Corner::ALL
        .iter()
        .position(|&value| value == corner)
        .unwrap_or_default() as i32
}

pub fn task_name(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::Check => "Проверка отклика",
        TaskKind::CreateNote => "Создание заметки",
        TaskKind::ReviseNote => "Редактирование заметки",
        TaskKind::FindNotes => "Поиск заметок",
        TaskKind::PreviewFiles => "Просмотр папки",
        TaskKind::MoveFiles => "Сортировка файлов",
        TaskKind::UndoFiles => "Возврат файлов",
        TaskKind::CheckCodex => "Проверка Codex CLI",
        TaskKind::RunCodex => "Задача Codex",
    }
}

pub fn task_status(kind: TaskKind, state: TaskState) -> &'static str {
    match state {
        TaskState::Queued => "В очереди",
        TaskState::Validating => "Проверка команды",
        TaskState::AwaitingApproval => "Ожидает подтверждения",
        TaskState::Running => "Выполняется",
        TaskState::Succeeded if matches!(kind, TaskKind::CreateNote | TaskKind::ReviseNote) => {
            "Заметка сохранена"
        }
        TaskState::Succeeded if kind == TaskKind::Check => "Проверка завершена",
        TaskState::Succeeded if kind == TaskKind::FindNotes => "Поиск завершён",
        TaskState::Succeeded => "Выполнено",
        TaskState::Failed if kind == TaskKind::Check => "Превышено время проверки",
        TaskState::Failed => "Не удалось выполнить задачу",
        TaskState::Cancelled => "Отменена",
    }
}

pub fn show_notes(window: &MimoWindow, found: Matches) {
    let notes = found
        .notes
        .into_iter()
        .map(|note| {
            let text = note.text;
            let first_line = text.lines().next().unwrap_or_default();
            let mut label: String = first_line.chars().take(45).collect();
            if first_line.chars().count() > 45 || text.lines().nth(1).is_some() {
                label.push('…');
            }
            NoteView {
                id: note.id.into(),
                label: label.into(),
                text: text.into(),
            }
        })
        .collect::<Vec<_>>();
    let hint = if notes.is_empty() {
        "Заметки не найдены".to_owned()
    } else if found.truncated {
        format!("Показано {}. Уточните запрос.", notes.len())
    } else {
        format!("Найдено {}", notes.len())
    };
    window.set_note_text("".into());
    window.set_note_id("".into());
    window.set_note_editing(false);
    window.set_notes_summary(hint.into());
    window.set_notes(Rc::new(VecModel::from(notes)).into());
    window.set_settings_open(false);
    window.set_files_open(false);
    window.set_codex_open(false);
    window.set_history_open(false);
    window.set_notes_open(true);
}

pub fn show_history(window: &MimoWindow, rows: Vec<String>) {
    window.set_history_message(
        if rows.is_empty() {
            "История пока пуста"
        } else {
            "Последние задачи"
        }
        .into(),
    );
    window.set_history_rows(
        Rc::new(VecModel::from(
            rows.into_iter().map(Into::into).collect::<Vec<_>>(),
        ))
        .into(),
    );
}

pub fn show_files(window: &MimoWindow, preview: &crate::files::Preview) {
    let rows = preview
        .moves
        .iter()
        .map(|item| format!("{} → {}", item.name, item.destination_label()))
        .collect::<Vec<_>>();
    let summary = format!("Элементов: {} · К переносу: {}", preview.total, rows.len());
    let listing = if rows.is_empty() {
        preview.entries.clone()
    } else {
        rows
    };
    show_file_rows(window, listing);
    window.set_files_message(summary.into());
    window.set_can_move(!preview.moves.is_empty());
}

pub fn show_file_rows(window: &MimoWindow, rows: Vec<String>) {
    window.set_file_rows(
        Rc::new(VecModel::from(
            rows.into_iter().map(Into::into).collect::<Vec<_>>(),
        ))
        .into(),
    );
}

pub fn tray_icon(tray: &MimoTray, window: &MimoWindow) {
    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(32, 32);
    for (index, pixel) in pixels.make_mut_slice().iter_mut().enumerate() {
        let x = (index % 32) as i32;
        let y = (index / 32) as i32;
        let dx = x - x.clamp(9, 22);
        let dy = y - 15;
        let distance = dx * dx + dy * dy;
        if distance <= 64 {
            let color = if distance >= 49 {
                [160, 168, 207, 255]
            } else {
                [42, 45, 65, 255]
            };
            *pixel = color.into();
        }
    }
    let icon = slint::Image::from_rgba8(pixels);
    tray.set_tray_image(icon.clone());
    window.set_window_icon(icon);
}

pub fn bind_close(window: &MimoWindow) {
    let weak = window.as_weak();
    window.window().on_close_requested(move || {
        if let Some(window) = weak.upgrade()
            && window.get_expanded()
        {
            window.invoke_toggle_panel();
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
}
