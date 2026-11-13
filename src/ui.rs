use std::rc::Rc;

use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::domain::{Corner, TaskState};

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

pub fn status_text(state: TaskState) -> &'static str {
    match state {
        TaskState::Queued => "В очереди",
        TaskState::Validating => "Проверка команды",
        TaskState::AwaitingApproval => "Ожидает подтверждения",
        TaskState::Running => "Выполняется",
        TaskState::Succeeded => "Проверка завершена",
        TaskState::Failed => "Превышено время проверки",
        TaskState::Cancelled => "Отменена",
    }
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
