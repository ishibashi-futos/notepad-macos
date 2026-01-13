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
        doc_id: u64,
        request_id: u64,
        path: PathBuf,
        result: Result<Vec<u8>, CoreError>,
    },
    SaveResult {
        doc_id: u64,
        request_id: u64,
        path: PathBuf,
        encoding: TextEncoding,
        result: Result<(), CoreError>,
    },
}

struct Document {
    id: u64,
    core: Core,
    active_open_request: Option<u64>,
    active_save_request: Option<u64>,
}

impl Document {
    fn new(id: u64) -> Self {
        Self {
            id,
            core: Core::new(),
            active_open_request: None,
            active_save_request: None,
        }
    }
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

        let mut ui = pollster::block_on(Ui::new(&window));
        let mut next_doc_id: u64 = 1;
        let mut documents = vec![Document::new(next_doc_id)];
        next_doc_id += 1;
        let mut active_doc_index: usize = 0;
        refresh_ui(&mut ui, &documents, active_doc_index);
        update_title(&window, &documents[active_doc_index].core);
        update_ime_cursor_area(&window, &documents[active_doc_index].core, &ui);

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

        let result = event_loop.run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::Wait);
            match event {
                Event::UserEvent(AppEvent::BackgroundTick(tick)) => {
                    println!("[bg] tick={tick}");
                }
                Event::UserEvent(AppEvent::OpenResult {
                    doc_id,
                    request_id,
                    path,
                    result,
                }) => {
                    let active_doc_id = documents
                        .get(active_doc_index)
                        .map(|doc| doc.id)
                        .unwrap_or_default();
                    let mut refresh_active = false;
                    let mut refresh_only_tabs = false;
                    let Some(doc) = documents.iter_mut().find(|doc| doc.id == doc_id) else {
                        return;
                    };
                    if doc.active_open_request != Some(request_id) {
                        return;
                    }
                    doc.active_open_request = None;
                    match result {
                        Ok(bytes) => match doc.core.load_from_bytes(&bytes) {
                            Ok(_) => {
                                doc.core.set_path(Some(path));
                                if active_doc_id == doc_id {
                                    refresh_active = true;
                                } else {
                                    refresh_only_tabs = true;
                                }
                            }
                            Err(err) => report_error(&err),
                        },
                        Err(err) => report_error(&err),
                    }
                    if refresh_active {
                        refresh_ui(&mut ui, &documents, active_doc_index);
                        let doc = &documents[active_doc_index];
                        update_title(&window, &doc.core);
                        update_ime_cursor_area(&window, &doc.core, &ui);
                        needs_redraw = true;
                    } else if refresh_only_tabs {
                        refresh_tabs(&mut ui, &documents, active_doc_index);
                        needs_redraw = true;
                    }
                }
                Event::UserEvent(AppEvent::SaveResult {
                    doc_id,
                    request_id,
                    path,
                    encoding,
                    result,
                }) => {
                    let active_doc_id = documents
                        .get(active_doc_index)
                        .map(|doc| doc.id)
                        .unwrap_or_default();
                    let mut refresh_tabs_only = false;
                    let mut refresh_title = false;
                    let Some(doc) = documents.iter_mut().find(|doc| doc.id == doc_id) else {
                        return;
                    };
                    if doc.active_save_request != Some(request_id) {
                        return;
                    }
                    doc.active_save_request = None;
                    match result {
                        Ok(()) => {
                            doc.core.mark_saved(path, encoding);
                            if active_doc_id == doc_id {
                                refresh_title = true;
                            }
                            refresh_tabs_only = true;
                        }
                        Err(err) => report_error(&err),
                    }
                    if refresh_title {
                        let doc = &documents[active_doc_index];
                        update_title(&window, &doc.core);
                    }
                    if refresh_tabs_only {
                        refresh_tabs(&mut ui, &documents, active_doc_index);
                        needs_redraw = true;
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
                            {
                                let doc = &mut documents[active_doc_index];
                                match ime {
                                    Ime::Enabled => {
                                        update_ime_cursor_area(&window, &doc.core, &ui);
                                    }
                                    Ime::Disabled => {
                                        doc.core.clear_preedit();
                                    }
                                    Ime::Preedit(text, cursor) => {
                                        doc.core.set_preedit(text, cursor);
                                    }
                                    Ime::Commit(text) => {
                                        doc.core.commit_preedit(&text);
                                    }
                                }
                            }
                            refresh_ui(&mut ui, &documents, active_doc_index);
                            let doc = &documents[active_doc_index];
                            update_title(&window, &doc.core);
                            update_ime_cursor_area(&window, &doc.core, &ui);
                            needs_redraw = true;
                        }
                        WindowEvent::KeyboardInput { event, .. } => {
                            if event.state == ElementState::Pressed {
                                let mut changed = false;
                                let command_key =
                                    modifiers.super_key() || modifiers.control_key();
                                let doc_id = documents[active_doc_index].id;
                                match event.logical_key {
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("o") =>
                                    {
                                        if let Some(path) = pick_open_path() {
                                            let request_id = next_request_id;
                                            next_request_id += 1;
                                            documents[active_doc_index].active_open_request =
                                                Some(request_id);
                                            start_open_task(
                                                proxy.clone(),
                                                doc_id,
                                                request_id,
                                                path,
                                            );
                                        }
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("s") =>
                                    {
                                        if modifiers.shift_key() {
                                            if let Some(path) = pick_save_path(
                                                documents[active_doc_index].core.path(),
                                            ) {
                                                let request_id = next_request_id;
                                                next_request_id += 1;
                                                documents[active_doc_index].active_save_request =
                                                    Some(request_id);
                                                start_save_task(
                                                    proxy.clone(),
                                                    doc_id,
                                                    request_id,
                                                    path,
                                                    documents[active_doc_index].core.encoding(),
                                                    documents[active_doc_index].core.text(),
                                                );
                                            }
                                        } else if let Some(path) = documents[active_doc_index]
                                            .core
                                            .path()
                                            .map(PathBuf::from)
                                        {
                                            let request_id = next_request_id;
                                            next_request_id += 1;
                                            documents[active_doc_index].active_save_request =
                                                Some(request_id);
                                            start_save_task(
                                                proxy.clone(),
                                                doc_id,
                                                request_id,
                                                path,
                                                documents[active_doc_index].core.encoding(),
                                                documents[active_doc_index].core.text(),
                                            );
                                        } else if let Some(path) = pick_save_path(
                                            documents[active_doc_index].core.path(),
                                        ) {
                                            let request_id = next_request_id;
                                            next_request_id += 1;
                                            documents[active_doc_index].active_save_request =
                                                Some(request_id);
                                            start_save_task(
                                                proxy.clone(),
                                                doc_id,
                                                request_id,
                                                path,
                                                documents[active_doc_index].core.encoding(),
                                                documents[active_doc_index].core.text(),
                                            );
                                        }
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("t") =>
                                    {
                                        let new_doc = Document::new(next_doc_id);
                                        next_doc_id += 1;
                                        documents.push(new_doc);
                                        let last_index = documents.len() - 1;
                                        switch_to_tab(
                                            &mut documents,
                                            &mut active_doc_index,
                                            last_index,
                                        );
                                        refresh_ui(&mut ui, &documents, active_doc_index);
                                        update_title(
                                            &window,
                                            &documents[active_doc_index].core,
                                        );
                                        update_ime_cursor_area(
                                            &window,
                                            &documents[active_doc_index].core,
                                            &ui,
                                        );
                                        needs_redraw = true;
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("w") =>
                                    {
                                        close_current_tab(
                                            &mut documents,
                                            &mut active_doc_index,
                                        );
                                        refresh_ui(&mut ui, &documents, active_doc_index);
                                        update_title(
                                            &window,
                                            &documents[active_doc_index].core,
                                        );
                                        update_ime_cursor_area(
                                            &window,
                                            &documents[active_doc_index].core,
                                            &ui,
                                        );
                                        needs_redraw = true;
                                    }
                                    Key::Character(ref ch)
                                        if command_key && modifiers.shift_key() && ch == "[" =>
                                    {
                                        let next_index = if active_doc_index == 0 {
                                            documents.len().saturating_sub(1)
                                        } else {
                                            active_doc_index - 1
                                        };
                                        switch_to_tab(
                                            &mut documents,
                                            &mut active_doc_index,
                                            next_index,
                                        );
                                        refresh_ui(&mut ui, &documents, active_doc_index);
                                        update_title(
                                            &window,
                                            &documents[active_doc_index].core,
                                        );
                                        update_ime_cursor_area(
                                            &window,
                                            &documents[active_doc_index].core,
                                            &ui,
                                        );
                                        needs_redraw = true;
                                    }
                                    Key::Character(ref ch)
                                        if command_key && modifiers.shift_key() && ch == "]" =>
                                    {
                                        let next_index =
                                            (active_doc_index + 1) % documents.len();
                                        switch_to_tab(
                                            &mut documents,
                                            &mut active_doc_index,
                                            next_index,
                                        );
                                        refresh_ui(&mut ui, &documents, active_doc_index);
                                        update_title(
                                            &window,
                                            &documents[active_doc_index].core,
                                        );
                                        update_ime_cursor_area(
                                            &window,
                                            &documents[active_doc_index].core,
                                            &ui,
                                        );
                                        needs_redraw = true;
                                    }
                                    Key::Character(ref ch)
                                        if command_key && is_tab_index_key(ch) =>
                                    {
                                        if let Some(index) = tab_index_from_key(ch) {
                                            if index < documents.len() {
                                                switch_to_tab(
                                                    &mut documents,
                                                    &mut active_doc_index,
                                                    index,
                                                );
                                                refresh_ui(
                                                    &mut ui,
                                                    &documents,
                                                    active_doc_index,
                                                );
                                                update_title(
                                                    &window,
                                                    &documents[active_doc_index].core,
                                                );
                                                update_ime_cursor_area(
                                                    &window,
                                                    &documents[active_doc_index].core,
                                                    &ui,
                                                );
                                                needs_redraw = true;
                                            }
                                        }
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("z") =>
                                    {
                                        let doc = &mut documents[active_doc_index];
                                        if modifiers.shift_key() {
                                            changed = doc.core.redo();
                                        } else {
                                            changed = doc.core.undo();
                                        }
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("y") =>
                                    {
                                        let doc = &mut documents[active_doc_index];
                                        changed = doc.core.redo();
                                    }
                                    Key::Character(ref ch)
                                        if command_key && modifiers.shift_key()
                                            && ch.eq_ignore_ascii_case("e") =>
                                    {
                                        let doc = &mut documents[active_doc_index];
                                        doc.core.set_encoding(doc.core.encoding().next());
                                        update_title(&window, &doc.core);
                                        refresh_tabs(&mut ui, &documents, active_doc_index);
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch == "1" =>
                                    {
                                        let doc = &mut documents[active_doc_index];
                                        doc.core.set_encoding(TextEncoding::Utf8);
                                        update_title(&window, &doc.core);
                                        refresh_tabs(&mut ui, &documents, active_doc_index);
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch == "2" =>
                                    {
                                        let doc = &mut documents[active_doc_index];
                                        doc.core.set_encoding(TextEncoding::Utf16Le);
                                        update_title(&window, &doc.core);
                                        refresh_tabs(&mut ui, &documents, active_doc_index);
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch == "3" =>
                                    {
                                        let doc = &mut documents[active_doc_index];
                                        doc.core.set_encoding(TextEncoding::Utf16Be);
                                        update_title(&window, &doc.core);
                                        refresh_tabs(&mut ui, &documents, active_doc_index);
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch == "4" =>
                                    {
                                        let doc = &mut documents[active_doc_index];
                                        doc.core.set_encoding(TextEncoding::ShiftJis);
                                        update_title(&window, &doc.core);
                                        refresh_tabs(&mut ui, &documents, active_doc_index);
                                    }
                                    Key::Named(NamedKey::Backspace) => {
                                        documents[active_doc_index].core.backspace();
                                        changed = true;
                                    }
                                    Key::Named(NamedKey::ArrowLeft) => {
                                        changed = move_cursor(
                                            &mut documents[active_doc_index].core,
                                            Direction::Left,
                                            modifiers.shift_key(),
                                        );
                                    }
                                    Key::Named(NamedKey::ArrowRight) => {
                                        changed = move_cursor(
                                            &mut documents[active_doc_index].core,
                                            Direction::Right,
                                            modifiers.shift_key(),
                                        );
                                    }
                                    Key::Named(NamedKey::ArrowUp) => {
                                        changed = move_cursor(
                                            &mut documents[active_doc_index].core,
                                            Direction::Up,
                                            modifiers.shift_key(),
                                        );
                                    }
                                    Key::Named(NamedKey::ArrowDown) => {
                                        changed = move_cursor(
                                            &mut documents[active_doc_index].core,
                                            Direction::Down,
                                            modifiers.shift_key(),
                                        );
                                    }
                                    Key::Named(NamedKey::Enter) => {
                                        documents[active_doc_index].core.insert_str("\n");
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
                                            documents[active_doc_index].core.insert_str(text);
                                            changed = true;
                                        }
                                    }
                                }

                                if changed {
                                    refresh_ui(&mut ui, &documents, active_doc_index);
                                    let doc = &documents[active_doc_index];
                                    update_title(&window, &doc.core);
                                    update_ime_cursor_area(&window, &doc.core, &ui);
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

fn start_open_task(
    proxy: EventLoopProxy<AppEvent>,
    doc_id: u64,
    request_id: u64,
    path: PathBuf,
) {
    std::thread::spawn(move || {
        let result = std::fs::read(&path)
            .map_err(|err| CoreError::from_io(format!("read {}", path.display()), err));
        let _ = proxy.send_event(AppEvent::OpenResult {
            doc_id,
            request_id,
            path,
            result,
        });
    });
}

fn start_save_task(
    proxy: EventLoopProxy<AppEvent>,
    doc_id: u64,
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
            doc_id,
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

fn refresh_ui(ui: &mut Ui, documents: &[Document], active_doc_index: usize) {
    let core = &documents[active_doc_index].core;
    let (line_numbers, digits) = build_line_numbers_text(core.line_count());
    ui.set_line_numbers(&line_numbers, digits);
    ui.set_text(&core.display_text());
    refresh_tabs(ui, documents, active_doc_index);
}

fn refresh_tabs(ui: &mut Ui, documents: &[Document], active_doc_index: usize) {
    let tab_bar = build_tab_bar(documents, active_doc_index);
    ui.set_tabs(&tab_bar);
}

fn build_tab_bar(documents: &[Document], active_doc_index: usize) -> String {
    let mut parts = Vec::with_capacity(documents.len());
    for (index, doc) in documents.iter().enumerate() {
        let label = doc_label(doc);
        let tab_index = index + 1;
        if index == active_doc_index {
            parts.push(format!("[{tab_index}:{label}]"));
        } else {
            parts.push(format!("{tab_index}:{label}"));
        }
    }
    parts.join("  ")
}

fn build_line_numbers_text(line_count: usize) -> (String, usize) {
    let line_count = line_count.max(1);
    let digits = line_count.to_string().len().max(1);
    let mut text = String::with_capacity(line_count * (digits + 1));
    for line in 1..=line_count {
        if line > 1 {
            text.push('\n');
        }
        text.push_str(&format!("{line:>width$}", width = digits));
    }
    (text, digits)
}

fn doc_label(doc: &Document) -> String {
    let name = doc
        .core
        .path()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled");
    if doc.core.is_dirty() {
        format!("{name}*")
    } else {
        name.to_string()
    }
}

fn switch_to_tab(documents: &mut [Document], active_doc_index: &mut usize, next_index: usize) {
    if *active_doc_index == next_index || documents.is_empty() {
        return;
    }
    let prev = &mut documents[*active_doc_index];
    prev.active_open_request = None;
    prev.active_save_request = None;
    prev.core.clear_preedit();
    *active_doc_index = next_index.min(documents.len() - 1);
}

fn close_current_tab(documents: &mut Vec<Document>, active_doc_index: &mut usize) {
    if documents.is_empty() {
        return;
    }
    if documents.len() == 1 {
        documents[0].core = Core::new();
        documents[0].active_open_request = None;
        documents[0].active_save_request = None;
        return;
    }
    documents.remove(*active_doc_index);
    if *active_doc_index >= documents.len() {
        *active_doc_index = documents.len() - 1;
    }
}

fn is_tab_index_key(ch: &str) -> bool {
    matches!(ch.chars().next(), Some('1'..='9'))
}

fn tab_index_from_key(ch: &str) -> Option<usize> {
    let digit = ch.chars().next()?;
    if !digit.is_ascii_digit() || digit == '0' {
        return None;
    }
    digit.to_digit(10).map(|value| value as usize - 1)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_index_from_key_maps_digits() {
        assert_eq!(tab_index_from_key("1"), Some(0));
        assert_eq!(tab_index_from_key("9"), Some(8));
        assert_eq!(tab_index_from_key("0"), None);
        assert_eq!(tab_index_from_key("a"), None);
    }

    #[test]
    fn build_tab_bar_marks_active_and_dirty() {
        let mut doc1 = Document::new(1);
        doc1.core.set_path(Some(PathBuf::from("/tmp/foo.txt")));
        let mut doc2 = Document::new(2);
        doc2.core.insert_str("x");
        let documents = vec![doc1, doc2];
        let bar = build_tab_bar(&documents, 1);
        assert_eq!(bar, "1:foo.txt  [2:Untitled*]");
    }

    #[test]
    fn build_line_numbers_text_pads_to_widest_digit() {
        let (text, digits) = build_line_numbers_text(12);
        assert_eq!(digits, 2);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], " 1");
        assert_eq!(lines[8], " 9");
        assert_eq!(lines[11], "12");
    }
}
