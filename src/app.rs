use std::path::PathBuf;
use std::time::Duration;

use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Event, Ime, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowBuilder;

use crate::core::{Core, CoreError, TextEncoding};
use crate::ui::Ui;

#[derive(Debug)]
enum AppEvent {
    BackgroundTick(u64),
    OpenResult {
        request_id: u64,
        path: PathBuf,
        result: Result<Vec<u8>, CoreError>,
    },
    SaveResult {
        request_id: u64,
        path: PathBuf,
        encoding: TextEncoding,
        result: Result<(), CoreError>,
    },
}

pub struct App;

impl App {
    pub fn run() {
        let event_loop = EventLoopBuilder::<AppEvent>::with_user_event()
            .build()
            .expect("failed to build event loop");
        let window = WindowBuilder::new()
            .with_title("Notepad Prototype")
            .with_inner_size(PhysicalSize::new(900, 600))
            .build(&event_loop)
            .expect("failed to build window");
        window.set_ime_allowed(true);

        let mut core = Core::new();
        core.insert_str("Notepad prototype\nType here...");

        let mut ui = pollster::block_on(Ui::new(&window));
        ui.set_text(&core.display_text());
        update_title(&window, &core);
        update_ime_cursor_area(&window, &core, &ui);

        let proxy = event_loop.create_proxy();
        let bg_proxy = proxy.clone();
        std::thread::spawn(move || {
            for tick in 0.. {
                std::thread::sleep(Duration::from_secs(2));
                if bg_proxy.send_event(AppEvent::BackgroundTick(tick)).is_err() {
                    break;
                }
            }
        });

        let mut needs_redraw = true;
        let mut modifiers = winit::keyboard::ModifiersState::default();
        let mut next_request_id: u64 = 1;
        let mut active_open_request: Option<u64> = None;
        let mut active_save_request: Option<u64> = None;

        let result = event_loop.run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::Wait);
            match event {
                Event::UserEvent(AppEvent::BackgroundTick(tick)) => {
                    println!("[bg] tick={tick}");
                }
                Event::UserEvent(AppEvent::OpenResult {
                    request_id,
                    path,
                    result,
                }) => {
                    if active_open_request != Some(request_id) {
                        return;
                    }
                    active_open_request = None;
                    match result {
                        Ok(bytes) => match core.load_from_bytes(&bytes) {
                            Ok(_) => {
                                core.set_path(Some(path));
                                ui.set_text(&core.display_text());
                                update_title(&window, &core);
                                update_ime_cursor_area(&window, &core, &ui);
                                needs_redraw = true;
                            }
                            Err(err) => report_error(&err),
                        },
                        Err(err) => report_error(&err),
                    }
                }
                Event::UserEvent(AppEvent::SaveResult {
                    request_id,
                    path,
                    encoding,
                    result,
                }) => {
                    if active_save_request != Some(request_id) {
                        return;
                    }
                    active_save_request = None;
                    match result {
                        Ok(()) => {
                            core.mark_saved(path, encoding);
                            update_title(&window, &core);
                            needs_redraw = true;
                        }
                        Err(err) => report_error(&err),
                    }
                }
                Event::WindowEvent { event, window_id } if window_id == window.id() => {
                    match event {
                        WindowEvent::CloseRequested => elwt.exit(),
                        WindowEvent::Resized(size) => {
                            ui.resize(size);
                            needs_redraw = true;
                        }
                        WindowEvent::ScaleFactorChanged {
                            mut inner_size_writer,
                            ..
                        } => {
                            let size = window.inner_size();
                            let _ = inner_size_writer.request_inner_size(size);
                            ui.resize(size);
                            needs_redraw = true;
                        }
                        WindowEvent::ModifiersChanged(state) => {
                            modifiers = state.state();
                        }
                        WindowEvent::Ime(ime) => {
                            log_ime_event(&ime);
                            match ime {
                                Ime::Enabled => {
                                    update_ime_cursor_area(&window, &core, &ui);
                                }
                                Ime::Disabled => {
                                    core.clear_preedit();
                                }
                                Ime::Preedit(text, cursor) => {
                                    core.set_preedit(text, cursor);
                                }
                                Ime::Commit(text) => {
                                    core.commit_preedit(&text);
                                }
                            }
                            ui.set_text(&core.display_text());
                            update_title(&window, &core);
                            update_ime_cursor_area(&window, &core, &ui);
                            needs_redraw = true;
                        }
                        WindowEvent::KeyboardInput { event, .. } => {
                            if event.state == ElementState::Pressed {
                                let mut changed = false;
                                let command_key =
                                    modifiers.super_key() || modifiers.control_key();
                                match event.logical_key {
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("o") =>
                                    {
                                        if let Some(path) = pick_open_path() {
                                            let request_id = next_request_id;
                                            next_request_id += 1;
                                            active_open_request = Some(request_id);
                                            start_open_task(proxy.clone(), request_id, path);
                                        }
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("s") =>
                                    {
                                        if modifiers.shift_key() {
                                            if let Some(path) = pick_save_path(core.path()) {
                                                let request_id = next_request_id;
                                                next_request_id += 1;
                                                active_save_request = Some(request_id);
                                                start_save_task(
                                                    proxy.clone(),
                                                    request_id,
                                                    path,
                                                    core.encoding(),
                                                    core.text(),
                                                );
                                            }
                                        } else if let Some(path) = core.path().map(PathBuf::from) {
                                            let request_id = next_request_id;
                                            next_request_id += 1;
                                            active_save_request = Some(request_id);
                                            start_save_task(
                                                proxy.clone(),
                                                request_id,
                                                path,
                                                core.encoding(),
                                                core.text(),
                                            );
                                        } else if let Some(path) = pick_save_path(core.path()) {
                                            let request_id = next_request_id;
                                            next_request_id += 1;
                                            active_save_request = Some(request_id);
                                            start_save_task(
                                                proxy.clone(),
                                                request_id,
                                                path,
                                                core.encoding(),
                                                core.text(),
                                            );
                                        }
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("z") =>
                                    {
                                        if modifiers.shift_key() {
                                            changed = core.redo();
                                        } else {
                                            changed = core.undo();
                                        }
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("y") =>
                                    {
                                        changed = core.redo();
                                    }
                                    Key::Character(ref ch)
                                        if command_key && modifiers.shift_key()
                                            && ch.eq_ignore_ascii_case("e") =>
                                    {
                                        core.set_encoding(core.encoding().next());
                                        update_title(&window, &core);
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch == "1" =>
                                    {
                                        core.set_encoding(TextEncoding::Utf8);
                                        update_title(&window, &core);
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch == "2" =>
                                    {
                                        core.set_encoding(TextEncoding::Utf16Le);
                                        update_title(&window, &core);
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch == "3" =>
                                    {
                                        core.set_encoding(TextEncoding::Utf16Be);
                                        update_title(&window, &core);
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch == "4" =>
                                    {
                                        core.set_encoding(TextEncoding::ShiftJis);
                                        update_title(&window, &core);
                                    }
                                    Key::Named(NamedKey::Backspace) => {
                                        core.backspace();
                                        changed = true;
                                    }
                                    Key::Named(NamedKey::ArrowLeft) => {
                                        changed = move_cursor(
                                            &mut core,
                                            Direction::Left,
                                            modifiers.shift_key(),
                                        );
                                    }
                                    Key::Named(NamedKey::ArrowRight) => {
                                        changed = move_cursor(
                                            &mut core,
                                            Direction::Right,
                                            modifiers.shift_key(),
                                        );
                                    }
                                    Key::Named(NamedKey::ArrowUp) => {
                                        changed =
                                            move_cursor(&mut core, Direction::Up, modifiers.shift_key());
                                    }
                                    Key::Named(NamedKey::ArrowDown) => {
                                        changed = move_cursor(
                                            &mut core,
                                            Direction::Down,
                                            modifiers.shift_key(),
                                        );
                                    }
                                    Key::Named(NamedKey::Enter) => {
                                        core.insert_str("\n");
                                        changed = true;
                                    }
                                    _ => {}
                                }

                                if !changed {
                                    if let Some(text) = event.text.as_ref() {
                                        if !modifiers.control_key()
                                            && !modifiers.alt_key()
                                            && !modifiers.super_key()
                                        {
                                            core.insert_str(text);
                                            changed = true;
                                        }
                                    }
                                }

                                if changed {
                                    ui.set_text(&core.display_text());
                                    update_title(&window, &core);
                                    update_ime_cursor_area(&window, &core, &ui);
                                    needs_redraw = true;
                                }
                            }
                        }
                        WindowEvent::RedrawRequested => {
                            if let Err(err) = ui.render() {
                                match err {
                                    wgpu::SurfaceError::Lost => ui.resize(ui.size()),
                                    wgpu::SurfaceError::OutOfMemory => elwt.exit(),
                                    wgpu::SurfaceError::Timeout => {}
                                    wgpu::SurfaceError::Outdated => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Event::AboutToWait => {
                    if needs_redraw {
                        window.request_redraw();
                        needs_redraw = false;
                    }
                }
                _ => {}
            }
        });
        if let Err(err) = result {
            eprintln!("event loop error: {err}");
        }
    }
}

fn pick_open_path() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_file()
}

fn pick_save_path(current_path: Option<&std::path::Path>) -> Option<PathBuf> {
    let dialog = rfd::FileDialog::new();
    let dialog = if let Some(path) = current_path {
        dialog.set_directory(path.parent().unwrap_or(path))
    } else {
        dialog
    };
    dialog.save_file()
}

fn start_open_task(proxy: EventLoopProxy<AppEvent>, request_id: u64, path: PathBuf) {
    std::thread::spawn(move || {
        let result = std::fs::read(&path)
            .map_err(|err| CoreError::from_io(format!("read {}", path.display()), err));
        let _ = proxy.send_event(AppEvent::OpenResult {
            request_id,
            path,
            result,
        });
    });
}

fn start_save_task(
    proxy: EventLoopProxy<AppEvent>,
    request_id: u64,
    path: PathBuf,
    encoding: TextEncoding,
    text: String,
) {
    std::thread::spawn(move || {
        let bytes = Core::encode_text(&text, encoding);
        let result = std::fs::write(&path, bytes)
            .map_err(|err| CoreError::from_io(format!("write {}", path.display()), err));
        let _ = proxy.send_event(AppEvent::SaveResult {
            request_id,
            path,
            encoding,
            result,
        });
    });
}

fn update_title(window: &winit::window::Window, core: &Core) {
    let name = core
        .path()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled");
    let dirty = if core.is_dirty() { "*" } else { "" };
    let cursor = core.cursor();
    window.set_title(&format!(
        "{name}{dirty} — {} (Ln {}, Col {})",
        core.encoding().label(),
        cursor.line + 1,
        cursor.col + 1
    ));
}

fn update_ime_cursor_area(window: &winit::window::Window, core: &Core, ui: &Ui) {
    let cursor = core.cursor_for_char(core.ime_cursor_char());
    let (x, y, w, h) = ui.caret_rect(cursor.line, cursor.col);
    window.set_ime_cursor_area(
        PhysicalPosition::new(x, y),
        PhysicalSize::new(w as u32, h as u32),
    );
}

#[derive(Debug, Clone, Copy)]
enum Direction {
    Left,
    Right,
    Up,
    Down,
}

fn move_cursor(core: &mut Core, direction: Direction, extend: bool) -> bool {
    let before_cursor = core.cursor();
    let before_selection = core.selection_range();
    match direction {
        Direction::Left => core.move_left(extend),
        Direction::Right => core.move_right(extend),
        Direction::Up => core.move_up(extend),
        Direction::Down => core.move_down(extend),
    }
    core.cursor() != before_cursor || core.selection_range() != before_selection
}

fn log_ime_event(ime: &Ime) {
    match ime {
        Ime::Enabled => println!("[ime] enabled"),
        Ime::Disabled => println!("[ime] disabled"),
        Ime::Preedit(text, cursor) => {
            println!("[ime] preedit text={text:?} cursor={cursor:?}");
        }
        Ime::Commit(text) => println!("[ime] commit text={text:?}"),
    }
}

fn report_error(err: &CoreError) {
    eprintln!("{}", err.describe());
}
