use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

const LABEL: &str = "overlay";
const W: f64 = 132.0;
const H: f64 = 36.0;
const MARGIN: f64 = 12.0;

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let w = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("overlay.html".into()))
        .title("Relay - registrazione")
        .inner_size(W, H)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .build()?;

    w.set_ignore_cursor_events(true)?;

    w.set_content_protected(true)?;
    Ok(())
}

pub fn set_visible(app: &AppHandle, show: bool) {
    let Some(w) = app.get_webview_window(LABEL) else {
        return;
    };
    if !show {
        let _ = w.hide();
        return;
    }
    if let Ok(Some(m)) = app.primary_monitor() {
        let scale = m.scale_factor();
        let size = m.size();
        let pos = m.position();
        let x = pos.x + size.width as i32 - ((W + MARGIN) * scale) as i32;
        let y = pos.y + (MARGIN * scale) as i32;
        let _ = w.set_position(PhysicalPosition::new(x, y));
    }
    let _ = w.set_always_on_top(true);

    let _ = w.show();
}
