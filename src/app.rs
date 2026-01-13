use std::path::PathBuf;
use std::time::Duration;

use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Event, Ime, MouseButton, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
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
    SearchResult {
        doc_id: u64,
        request_id: u64,
        query: String,
        matches: Vec<usize>,
    },
}

#[derive(Debug, Default)]
struct SearchState {
    query: String,
    matches: Vec<usize>,
    pending: bool,
}

#[derive(Debug)]
struct ClipboardHistory {
    items: Vec<String>,
    max: usize,
    selected_index: usize,
    visible: bool,
    window_start: usize,
}

impl ClipboardHistory {
    fn new(max: usize) -> Self {
        Self {
            items: Vec::new(),
            max,
            selected_index: 0,
            visible: false,
            window_start: 0,
        }
    }

    fn is_visible(&self) -> bool {
        self.visible
    }

    fn visible_count(&self) -> usize {
        self.items.len().min(3)
    }

    fn push(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        if self.items.first().is_some_and(|item| item == text) {
            return false;
        }
        self.items.insert(0, text.to_string());
        if self.items.len() > self.max {
            self.items.truncate(self.max);
        }
        self.selected_index = 0;
        self.window_start = 0;
        if self.selected_index >= self.items.len() {
            self.selected_index = self.items.len().saturating_sub(1);
        }
        true
    }

    fn show(&mut self) -> bool {
        if self.items.is_empty() {
            self.visible = false;
            return false;
        }
        self.visible = true;
        self.selected_index = 0;
        self.window_start = 0;
        true
    }

    fn hide(&mut self) -> bool {
        let changed = self.visible;
        self.visible = false;
        changed
    }

    fn move_up(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
        self.adjust_window();
    }

    fn move_down(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() - 1;
        self.selected_index = (self.selected_index + 1).min(last);
        self.adjust_window();
    }

    fn select_visible_index(&mut self, index: usize) -> bool {
        let offset = self.window_start + index;
        if index < self.visible_count() && offset < self.items.len() {
            self.selected_index = offset;
            self.adjust_window();
            true
        } else {
            false
        }
    }

    fn selected_text(&self) -> Option<&str> {
        self.items.get(self.selected_index).map(|item| item.as_str())
    }

    fn window_range(&self) -> std::ops::Range<usize> {
        let start = self.window_start.min(self.items.len());
        let end = (start + self.visible_count()).min(self.items.len());
        start..end
    }

    fn adjust_window(&mut self) {
        let window_size = self.visible_count().max(1);
        if self.selected_index < self.window_start {
            self.window_start = self.selected_index;
        } else if self.selected_index >= self.window_start + window_size {
            self.window_start = self
                .selected_index
                .saturating_sub(window_size.saturating_sub(1));
        }
        let max_start = self.items.len().saturating_sub(window_size);
        if self.window_start > max_start {
            self.window_start = max_start;
        }
    }
}

struct Document {
    id: u64,
    core: Core,
    active_open_request: Option<u64>,
    active_save_request: Option<u64>,
    active_search_request: Option<u64>,
    search_state: SearchState,
}

impl Document {
    fn new(id: u64) -> Self {
        Self {
            id,
            core: Core::new(),
            active_open_request: None,
            active_save_request: None,
            active_search_request: None,
            search_state: SearchState::default(),
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
        let mut search_query = String::new();
        let mut search_active = false;
        let mut search_preedit: Option<String> = None;
        let mut clipboard_history = ClipboardHistory::new(100);
        let mut fn_pressed = false;
        refresh_ui(
            &mut ui,
            &documents,
            active_doc_index,
            &search_query,
            search_preedit.as_deref(),
            search_active,
            &clipboard_history,
        );
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
        let mut cursor_position: Option<PhysicalPosition<f64>> = None;

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
                        if search_active || !search_query.is_empty() {
                            let effective_query = build_search_effective_query(
                                &search_query,
                                search_preedit.as_deref(),
                            );
                            request_search_update(
                                &mut documents[active_doc_index],
                                &proxy,
                                &mut next_request_id,
                                effective_query,
                                true,
                            );
                        }
                        refresh_ui(
                            &mut ui,
                            &documents,
                            active_doc_index,
                            &search_query,
                            search_preedit.as_deref(),
                            search_active,
                            &clipboard_history,
                        );
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
                Event::UserEvent(AppEvent::SearchResult {
                    doc_id,
                    request_id,
                    query,
                    matches,
                }) => {
                    let active_doc_id = documents
                        .get(active_doc_index)
                        .map(|doc| doc.id)
                        .unwrap_or_default();
                    let Some(doc) = documents.iter_mut().find(|doc| doc.id == doc_id) else {
                        return;
                    };
                    if doc.active_search_request != Some(request_id) {
                        return;
                    }
                    doc.active_search_request = None;
                    doc.search_state.query = query;
                    doc.search_state.matches = matches;
                    doc.search_state.pending = false;
                    if active_doc_id == doc_id {
                        refresh_search_ui(
                            &mut ui,
                            &doc.core,
                            &doc.search_state,
                            &search_query,
                            search_preedit.as_deref(),
                            search_active,
                            &clipboard_history,
                        );
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
                        WindowEvent::CursorMoved { position, .. } => {
                            cursor_position = Some(position);
                        }
                        WindowEvent::MouseInput { state, button, .. } => {
                            if state == ElementState::Pressed && button == MouseButton::Left {
                                if let Some(position) = cursor_position {
                                    let line_count =
                                        documents[active_doc_index].core.line_count();
                                    if let Some(line) =
                                        ui.line_number_hit_test(position, line_count)
                                    {
                                        let changed = {
                                            let doc = &mut documents[active_doc_index];
                                            doc.core.set_cursor_line_col(line, 0, false)
                                        };
                                        if changed {
                                            refresh_ui(
                                                &mut ui,
                                                &documents,
                                                active_doc_index,
                                                &search_query,
                                                search_preedit.as_deref(),
                                                search_active,
                                                &clipboard_history,
                                            );
                                            let doc = &documents[active_doc_index];
                                            update_title(&window, &doc.core);
                                            update_ime_cursor_area(
                                                &window,
                                                &doc.core,
                                                &ui,
                                            );
                                            needs_redraw = true;
                                        }
                                    }
                                }
                            }
                        }
                        WindowEvent::Ime(ime) => {
                            if clipboard_history.is_visible() {
                                return;
                            }
                            log_ime_event(&ime);
                            if search_active {
                                let mut search_dirty = false;
                                match ime {
                                    Ime::Enabled => {}
                                    Ime::Disabled => {
                                        search_preedit = None;
                                        search_dirty = true;
                                    }
                                    Ime::Preedit(text, _) => {
                                        if text.is_empty() {
                                            search_preedit = None;
                                        } else {
                                            search_preedit = Some(text);
                                        }
                                        search_dirty = true;
                                    }
                                    Ime::Commit(text) => {
                                        if !text.is_empty() {
                                            search_query.push_str(&text);
                                            search_dirty = true;
                                        }
                                        search_preedit = None;
                                    }
                                }
                                if search_dirty {
                                    let effective_query = build_search_effective_query(
                                        &search_query,
                                        search_preedit.as_deref(),
                                    );
                                    request_search_update(
                                        &mut documents[active_doc_index],
                                        &proxy,
                                        &mut next_request_id,
                                        effective_query,
                                        true,
                                    );
                                }
                                refresh_search_ui(
                                    &mut ui,
                                    &documents[active_doc_index].core,
                                    &documents[active_doc_index].search_state,
                                    &search_query,
                                    search_preedit.as_deref(),
                                    search_active,
                                    &clipboard_history,
                                );
                                needs_redraw = true;
                            } else {
                                let mut text_changed = false;
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
                                            text_changed = !text.is_empty();
                                        }
                                    }
                                }
                                if text_changed && !search_query.is_empty() {
                                    let effective_query = build_search_effective_query(
                                        &search_query,
                                        search_preedit.as_deref(),
                                    );
                                    request_search_update(
                                        &mut documents[active_doc_index],
                                        &proxy,
                                        &mut next_request_id,
                                        effective_query,
                                        true,
                                    );
                                }
                                refresh_ui(
                                    &mut ui,
                                    &documents,
                                    active_doc_index,
                                    &search_query,
                                    search_preedit.as_deref(),
                                    search_active,
                                    &clipboard_history,
                                );
                                let doc = &documents[active_doc_index];
                                update_title(&window, &doc.core);
                                update_ime_cursor_area(&window, &doc.core, &ui);
                                needs_redraw = true;
                            }
                        }
                        WindowEvent::KeyboardInput { event, .. } => {
                            if matches!(event.logical_key, Key::Named(NamedKey::Fn)) {
                                fn_pressed = event.state == ElementState::Pressed;
                                return;
                            }
                            if event.state == ElementState::Pressed {
                                let mut changed = false;
                                let mut search_dirty = false;
                                let mut history_dirty = false;
                                let mut suppress_editor_input =
                                    search_active || clipboard_history.is_visible();
                                let mut text_changed = false;
                                let mut history_commit: Option<String> = None;
                                let command_key =
                                    modifiers.super_key() || modifiers.control_key();
                                let ctrl_v = is_ctrl_v(event.physical_key, modifiers);
                                let doc_id = documents[active_doc_index].id;
                                if clipboard_history.is_visible() {
                                    match event.logical_key {
                                        Key::Named(NamedKey::Escape) => {
                                            if clipboard_history.hide() {
                                                history_dirty = true;
                                            }
                                            suppress_editor_input = true;
                                        }
                                        Key::Named(NamedKey::Enter) => {
                                            history_commit = clipboard_history
                                                .selected_text()
                                                .map(|text| text.to_string());
                                            if clipboard_history.hide() {
                                                history_dirty = true;
                                            }
                                            suppress_editor_input = true;
                                        }
                                        Key::Named(NamedKey::ArrowUp) => {
                                            clipboard_history.move_up();
                                            history_dirty = true;
                                            suppress_editor_input = true;
                                        }
                                        Key::Named(NamedKey::ArrowDown) => {
                                            clipboard_history.move_down();
                                            history_dirty = true;
                                            suppress_editor_input = true;
                                        }
                                        Key::Character(ref ch)
                                            if matches!(ch.as_str(), "1" | "2" | "3") =>
                                        {
                                            let index = ch.parse::<usize>().unwrap_or(1) - 1;
                                            if clipboard_history.select_visible_index(index) {
                                                history_commit = clipboard_history
                                                    .selected_text()
                                                    .map(|text| text.to_string());
                                                if clipboard_history.hide() {
                                                    history_dirty = true;
                                                }
                                            }
                                            suppress_editor_input = true;
                                        }
                                        _ => {
                                            suppress_editor_input = true;
                                        }
                                    }
                                } else if search_active {
                                    match event.logical_key {
                                        Key::Named(NamedKey::Escape) => {
                                            search_active = false;
                                            search_query.clear();
                                            search_preedit = None;
                                            search_dirty = true;
                                            suppress_editor_input = true;
                                        }
                                        Key::Named(NamedKey::Enter) => {
                                            if search_preedit.is_none() && !search_query.is_empty() {
                                                let doc = &mut documents[active_doc_index];
                                                if modifiers.shift_key() {
                                                    let start = doc.core.cursor_char().saturating_sub(1);
                                                    if let Some(idx) =
                                                        doc.core.find_prev(&search_query, start)
                                                    {
                                                        let cursor = doc.core.cursor_for_char(idx);
                                                        changed = doc
                                                            .core
                                                            .set_cursor_line_col(
                                                                cursor.line,
                                                                cursor.col,
                                                                false,
                                                            );
                                                    }
                                                } else {
                                                    let start = doc.core.cursor_char().saturating_add(1);
                                                    if let Some(idx) =
                                                        doc.core.find_next(&search_query, start)
                                                    {
                                                        let cursor = doc.core.cursor_for_char(idx);
                                                        changed = doc
                                                            .core
                                                            .set_cursor_line_col(
                                                                cursor.line,
                                                                cursor.col,
                                                                false,
                                                            );
                                                    }
                                                }
                                            }
                                            suppress_editor_input = true;
                                        }
                                        Key::Named(NamedKey::Backspace) => {
                                            if search_preedit.is_none() {
                                                search_query.pop();
                                                search_dirty = true;
                                            }
                                            suppress_editor_input = true;
                                        }
                                        _ => {}
                                    }
                                    if let Some(text) = event.text.as_ref() {
                                        if !command_key
                                            && !modifiers.alt_key()
                                            && !modifiers.super_key()
                                        {
                                            if !text.is_empty()
                                                && text.chars().all(|ch| !ch.is_control())
                                            {
                                                search_query.push_str(text);
                                                search_dirty = true;
                                            }
                                        }
                                    }
                                } else {
                                    if ctrl_v {
                                        if clipboard_history.show() {
                                            history_dirty = true;
                                            suppress_editor_input = true;
                                        }
                                    } else {
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
                                        if search_active || !search_query.is_empty() {
                                            let effective_query = build_search_effective_query(
                                                &search_query,
                                                search_preedit.as_deref(),
                                            );
                                            request_search_update(
                                                &mut documents[active_doc_index],
                                                &proxy,
                                                &mut next_request_id,
                                                effective_query,
                                                false,
                                            );
                                        }
                                        refresh_ui(
                                            &mut ui,
                                            &documents,
                                            active_doc_index,
                                            &search_query,
                                            search_preedit.as_deref(),
                                            search_active,
                                            &clipboard_history,
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
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("f") =>
                                    {
                                        search_active = true;
                                        search_dirty = true;
                                        search_preedit = None;
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("a") =>
                                    {
                                        let doc = &mut documents[active_doc_index];
                                        changed = doc.core.select_all();
                                    }
                                    Key::Character(ref ch)
                                        if command_key
                                            && fn_pressed
                                            && ch.eq_ignore_ascii_case("c") =>
                                    {
                                        if let Some(text) =
                                            documents[active_doc_index].core.selected_text()
                                        {
                                            if clipboard_history.push(&text) {
                                                history_dirty = true;
                                            }
                                        }
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("c") =>
                                    {
                                        if let Some(text) =
                                            documents[active_doc_index].core.selected_text()
                                        {
                                            if clipboard_history.push(&text) {
                                                history_dirty = true;
                                            }
                                            if let Err(err) = set_clipboard_text(&text) {
                                                eprintln!("[clipboard] copy failed: {err}");
                                            }
                                        }
                                    }
                                    Key::Character(ref ch)
                                        if command_key
                                            && !modifiers.control_key()
                                            && ch.eq_ignore_ascii_case("v") =>
                                    {
                                        if let Ok(text) = get_clipboard_text() {
                                            if !text.is_empty() {
                                                documents[active_doc_index].core.insert_str(&text);
                                                changed = true;
                                                text_changed = true;
                                            }
                                        }
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("x") =>
                                    {
                                        if let Some(text) =
                                            documents[active_doc_index].core.selected_text()
                                        {
                                            if let Err(err) = set_clipboard_text(&text) {
                                                eprintln!("[clipboard] cut failed: {err}");
                                            } else if documents[active_doc_index]
                                                .core
                                                .delete_selection()
                                            {
                                                changed = true;
                                                text_changed = true;
                                            }
                                        }
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("w") =>
                                    {
                                        close_current_tab(
                                            &mut documents,
                                            &mut active_doc_index,
                                        );
                                        if search_active || !search_query.is_empty() {
                                            let effective_query = build_search_effective_query(
                                                &search_query,
                                                search_preedit.as_deref(),
                                            );
                                            request_search_update(
                                                &mut documents[active_doc_index],
                                                &proxy,
                                                &mut next_request_id,
                                                effective_query,
                                                false,
                                            );
                                        }
                                        refresh_ui(
                                            &mut ui,
                                            &documents,
                                            active_doc_index,
                                            &search_query,
                                            search_preedit.as_deref(),
                                            search_active,
                                            &clipboard_history,
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
                                        if search_active || !search_query.is_empty() {
                                            let effective_query = build_search_effective_query(
                                                &search_query,
                                                search_preedit.as_deref(),
                                            );
                                            request_search_update(
                                                &mut documents[active_doc_index],
                                                &proxy,
                                                &mut next_request_id,
                                                effective_query,
                                                false,
                                            );
                                        }
                                        refresh_ui(
                                            &mut ui,
                                            &documents,
                                            active_doc_index,
                                            &search_query,
                                            search_preedit.as_deref(),
                                            search_active,
                                            &clipboard_history,
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
                                        if search_active || !search_query.is_empty() {
                                            let effective_query = build_search_effective_query(
                                                &search_query,
                                                search_preedit.as_deref(),
                                            );
                                            request_search_update(
                                                &mut documents[active_doc_index],
                                                &proxy,
                                                &mut next_request_id,
                                                effective_query,
                                                false,
                                            );
                                        }
                                        refresh_ui(
                                            &mut ui,
                                            &documents,
                                            active_doc_index,
                                            &search_query,
                                            search_preedit.as_deref(),
                                            search_active,
                                            &clipboard_history,
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
                                                if search_active || !search_query.is_empty() {
                                                    let effective_query =
                                                        build_search_effective_query(
                                                            &search_query,
                                                            search_preedit.as_deref(),
                                                        );
                                                    request_search_update(
                                                        &mut documents[active_doc_index],
                                                        &proxy,
                                                        &mut next_request_id,
                                                        effective_query,
                                                        false,
                                                    );
                                                }
                                                refresh_ui(
                                                    &mut ui,
                                                    &documents,
                                                    active_doc_index,
                                                    &search_query,
                                                    search_preedit.as_deref(),
                                                    search_active,
                                                    &clipboard_history,
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
                                        text_changed = changed;
                                    }
                                    Key::Character(ref ch)
                                        if command_key && ch.eq_ignore_ascii_case("y") =>
                                    {
                                        let doc = &mut documents[active_doc_index];
                                        changed = doc.core.redo();
                                        text_changed = changed;
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
                                        text_changed = true;
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
                                        text_changed = true;
                                    }
                                    _ => {}
                                }
                                    }
                                }

                                if let Some(text) = history_commit {
                                    documents[active_doc_index].core.insert_str(&text);
                                    changed = true;
                                    text_changed = true;
                                }

                                if !search_active && !changed && !suppress_editor_input {
                                    if let Some(text) = event.text.as_ref() {
                                        if !modifiers.control_key()
                                            && !modifiers.alt_key()
                                            && !modifiers.super_key()
                                        {
                                            documents[active_doc_index].core.insert_str(text);
                                            changed = true;
                                            text_changed = true;
                                        }
                                    }
                                }

                                if search_dirty || text_changed {
                                    let effective_query = build_search_effective_query(
                                        &search_query,
                                        search_preedit.as_deref(),
                                    );
                                    request_search_update(
                                        &mut documents[active_doc_index],
                                        &proxy,
                                        &mut next_request_id,
                                        effective_query,
                                        true,
                                    );
                                }

                                if search_dirty {
                                    refresh_search_ui(
                                        &mut ui,
                                        &documents[active_doc_index].core,
                                        &documents[active_doc_index].search_state,
                                        &search_query,
                                        search_preedit.as_deref(),
                                        search_active,
                                        &clipboard_history,
                                    );
                                    needs_redraw = true;
                                }

                                if history_dirty {
                                    refresh_search_ui(
                                        &mut ui,
                                        &documents[active_doc_index].core,
                                        &documents[active_doc_index].search_state,
                                        &search_query,
                                        search_preedit.as_deref(),
                                        search_active,
                                        &clipboard_history,
                                    );
                                    needs_redraw = true;
                                }

                                if changed {
                                    refresh_ui(
                                        &mut ui,
                                        &documents,
                                        active_doc_index,
                                        &search_query,
                                        search_preedit.as_deref(),
                                        search_active,
                                        &clipboard_history,
                                    );
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

fn start_search_task(
    proxy: EventLoopProxy<AppEvent>,
    doc_id: u64,
    request_id: u64,
    query: String,
    text: String,
) {
    std::thread::spawn(move || {
        let matches = crate::core::find_all_in_text(&text, &query);
        let _ = proxy.send_event(AppEvent::SearchResult {
            doc_id,
            request_id,
            query,
            matches,
        });
    });
}

fn request_search_update(
    doc: &mut Document,
    proxy: &EventLoopProxy<AppEvent>,
    next_request_id: &mut u64,
    effective_query: String,
    force: bool,
) {
    if effective_query.is_empty() {
        doc.search_state.query.clear();
        doc.search_state.matches.clear();
        doc.search_state.pending = false;
        doc.active_search_request = None;
        return;
    }
    if !force && doc.search_state.query == effective_query && !doc.search_state.pending {
        return;
    }
    let request_id = *next_request_id;
    *next_request_id += 1;
    doc.active_search_request = Some(request_id);
    doc.search_state.query = effective_query.clone();
    doc.search_state.matches.clear();
    doc.search_state.pending = true;
    let text = doc.core.text();
    start_search_task(proxy.clone(), doc.id, request_id, effective_query, text);
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

fn refresh_ui(
    ui: &mut Ui,
    documents: &[Document],
    active_doc_index: usize,
    search_query: &str,
    search_preedit: Option<&str>,
    search_active: bool,
    clipboard_history: &ClipboardHistory,
) {
    let doc = &documents[active_doc_index];
    let core = &doc.core;
    let (line_numbers, digits) = build_line_numbers_text(core.line_count());
    ui.set_line_numbers(&line_numbers, digits);
    ui.set_text(&core.display_text());
    refresh_search_ui(
        ui,
        core,
        &doc.search_state,
        search_query,
        search_preedit,
        search_active,
        clipboard_history,
    );
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

fn build_search_bar_text(query: &str, preedit: Option<&str>) -> String {
    let mut text = String::from("Search:");
    if !query.is_empty() || preedit.is_some() {
        text.push(' ');
        text.push_str(query);
        if let Some(preedit) = preedit {
            text.push_str(preedit);
        }
    }
    text
}

fn build_search_nav_text(
    core: &Core,
    search_state: &SearchState,
    query: &str,
    preedit: Option<&str>,
) -> String {
    let effective_query = build_search_effective_query(query, preedit);
    let nav_hint = " (Enter: next, Shift+Enter: prev)";
    if effective_query.is_empty() {
        return format!("Matches: 0/0{nav_hint}");
    }
    if search_state.pending || search_state.query != effective_query {
        return format!("Matches: --/--  Searching...{nav_hint}");
    }
    let total = search_state.matches.len();
    let current = current_match_index(
        &search_state.matches,
        core.cursor_char(),
        effective_query.chars().count(),
    );
    format!("Matches: {current}/{total}{nav_hint}")
}

fn build_search_effective_query(query: &str, preedit: Option<&str>) -> String {
    if let Some(preedit) = preedit {
        let mut text = String::with_capacity(query.len() + preedit.len());
        text.push_str(query);
        text.push_str(preedit);
        text
    } else {
        query.to_string()
    }
}

fn current_match_index(matches: &[usize], cursor: usize, query_len: usize) -> usize {
    if matches.is_empty() || query_len == 0 {
        return 0;
    }
    for (index, &pos) in matches.iter().enumerate() {
        if cursor >= pos && cursor < pos + query_len {
            return index + 1;
        }
    }
    for (index, &pos) in matches.iter().enumerate() {
        if pos > cursor {
            return index + 1;
        }
    }
    1
}

fn refresh_search_ui(
    ui: &mut Ui,
    core: &Core,
    search_state: &SearchState,
    search_query: &str,
    search_preedit: Option<&str>,
    search_active: bool,
    clipboard_history: &ClipboardHistory,
) {
    let search_text = build_search_bar_text(search_query, search_preedit);
    let search_visible = search_active || !search_query.is_empty();
    ui.set_search(&search_text, search_visible);
    if clipboard_history.is_visible() {
        if let Some(nav_text) = build_clipboard_nav_text(clipboard_history) {
            ui.set_search_navigation(&nav_text, true);
        } else {
            ui.set_search_navigation("", false);
        }
    } else {
        let nav_text = build_search_nav_text(core, search_state, search_query, search_preedit);
        ui.set_search_navigation(&nav_text, search_visible);
    }
    let selection_rects = build_selection_rects(ui, core);
    ui.set_selection_rects(&selection_rects);
}

fn build_clipboard_nav_text(history: &ClipboardHistory) -> Option<String> {
    if !history.is_visible() || history.items.is_empty() {
        return None;
    }
    let mut lines = Vec::with_capacity(history.visible_count() + 1);
    lines.push("Clipboard:".to_string());
    let range = history.window_range();
    for (offset, item) in history.items[range.clone()].iter().enumerate() {
        let absolute_index = range.start + offset;
        let prefix = if absolute_index == history.selected_index {
            "> "
        } else {
            "  "
        };
        let display = format_clipboard_item(item, 40);
        lines.push(format!("{prefix}[{}] {}", offset + 1, display));
    }
    Some(lines.join("\n"))
}

fn format_clipboard_item(item: &str, limit: usize) -> String {
    let normalized = item.replace('\n', "\\n");
    let mut chars = normalized.chars();
    let mut out = String::with_capacity(limit.min(normalized.len()));
    for _ in 0..limit {
        let Some(ch) = chars.next() else {
            break;
        };
        out.push(ch);
    }
    out
}

fn build_selection_spans(core: &Core) -> Vec<(usize, usize, usize)> {
    let Some((start, end)) = core.selection_range() else {
        return Vec::new();
    };
    let start_cursor = core.cursor_for_char(start);
    let end_cursor = core.cursor_for_char(end);
    let line_count = core.line_count().max(1);
    let start_line = start_cursor.line.min(line_count - 1);
    let end_line = end_cursor.line.min(line_count - 1);
    let mut spans = Vec::new();
    for line in start_line..=end_line {
        let line_len = core.line_len_chars(line);
        let (start_col, end_col) = if line == start_line && line == end_line {
            (start_cursor.col, end_cursor.col)
        } else if line == start_line {
            (start_cursor.col, line_len)
        } else if line == end_line {
            (0, end_cursor.col)
        } else {
            (0, line_len)
        };
        if end_col > start_col {
            spans.push((line, start_col, end_col));
        }
    }
    spans
}

fn build_selection_rects(ui: &Ui, core: &Core) -> Vec<(f32, f32, f32, f32)> {
    let spans = build_selection_spans(core);
    spans
        .into_iter()
        .map(|(line, start_col, end_col)| ui.selection_rect(line, start_col, end_col))
        .collect()
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
    prev.active_search_request = None;
    prev.search_state.pending = false;
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
        documents[0].active_search_request = None;
        documents[0].search_state = SearchState::default();
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

fn is_ctrl_v(physical_key: PhysicalKey, modifiers: ModifiersState) -> bool {
    modifiers.control_key() && matches!(physical_key, PhysicalKey::Code(KeyCode::KeyV))
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

fn set_clipboard_text(text: &str) -> Result<(), arboard::Error> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text.to_string())?;
    Ok(())
}

fn get_clipboard_text() -> Result<String, arboard::Error> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.get_text()
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

    #[test]
    fn build_search_bar_text_formats_query() {
        assert_eq!(build_search_bar_text("", None), "Search:");
        assert_eq!(build_search_bar_text("abc", None), "Search: abc");
        assert_eq!(build_search_bar_text("", Some("かな")), "Search: かな");
        assert_eq!(build_search_bar_text("ab", Some("c")), "Search: abc");
    }

    #[test]
    fn build_search_nav_text_shows_matches() {
        let mut core = Core::new();
        core.insert_str("abc def abc");
        let search_state = SearchState {
            query: "abc".to_string(),
            matches: vec![0, 8],
            pending: false,
        };
        let nav = build_search_nav_text(&core, &search_state, "abc", None);
        assert_eq!(
            nav,
            "Matches: 1/2 (Enter: next, Shift+Enter: prev)"
        );
        core.set_cursor_line_col(0, 9, false);
        let nav = build_search_nav_text(&core, &search_state, "abc", None);
        assert_eq!(
            nav,
            "Matches: 2/2 (Enter: next, Shift+Enter: prev)"
        );
    }

    #[test]
    fn build_search_nav_text_shows_searching_when_pending() {
        let mut core = Core::new();
        core.insert_str("abc def abc");
        let search_state = SearchState {
            query: "abc".to_string(),
            matches: vec![],
            pending: true,
        };
        let nav = build_search_nav_text(&core, &search_state, "abc", None);
        assert_eq!(
            nav,
            "Matches: --/--  Searching... (Enter: next, Shift+Enter: prev)"
        );
    }

    #[test]
    fn build_selection_spans_handles_multiline_selection() {
        let mut core = Core::new();
        core.insert_str("ab\ncd\nef");
        core.set_cursor_line_col(0, 1, false);
        core.set_cursor_line_col(1, 1, true);
        let spans = build_selection_spans(&core);
        assert_eq!(spans, vec![(0, 1, 2), (1, 0, 1)]);
    }

    #[test]
    fn clipboard_history_pushes_and_trims() {
        let mut history = ClipboardHistory::new(3);
        assert!(!history.show());
        assert!(!history.push(""));
        assert!(history.push("a"));
        assert!(!history.push("a"));
        assert!(history.push("b"));
        assert!(history.push("c"));
        assert!(history.push("d"));
        assert_eq!(history.items, vec!["d", "c", "b"]);
        assert_eq!(history.selected_index, 0);
    }

    #[test]
    fn clipboard_history_nav_text_formats_items() {
        let mut history = ClipboardHistory::new(100);
        history.push("hello world");
        history.push("こんにちは\n世界");
        let long = "x".repeat(50);
        history.push(&long);
        assert!(history.show());
        history.move_down();
        let nav = build_clipboard_nav_text(&history).expect("nav text");
        let expected = format!(
            "Clipboard:\n  [1] {}\n> [2] こんにちは\\n世界\n  [3] hello world",
            "x".repeat(40)
        );
        assert_eq!(nav, expected);
    }

    #[test]
    fn clipboard_history_moves_selection_within_bounds() {
        let mut history = ClipboardHistory::new(10);
        history.push("first");
        history.push("second");
        history.show();
        history.move_down();
        history.move_down();
        assert_eq!(history.selected_index, 1);
        assert_eq!(history.window_start, 0);
        history.move_up();
        assert_eq!(history.selected_index, 0);
        assert!(!history.select_visible_index(5));
        assert!(history.select_visible_index(1));
    }

    #[test]
    fn is_ctrl_v_detects_control_v() {
        assert!(is_ctrl_v(
            PhysicalKey::Code(KeyCode::KeyV),
            ModifiersState::CONTROL
        ));
        assert!(!is_ctrl_v(
            PhysicalKey::Code(KeyCode::KeyV),
            ModifiersState::SUPER
        ));
        assert!(!is_ctrl_v(
            PhysicalKey::Code(KeyCode::KeyC),
            ModifiersState::CONTROL
        ));
    }

    #[test]
    fn clipboard_history_scrolls_window() {
        let mut history = ClipboardHistory::new(10);
        history.push("one");
        history.push("two");
        history.push("three");
        history.push("four");
        history.push("five");
        history.show();
        assert_eq!(history.window_start, 0);
        history.move_down();
        history.move_down();
        assert_eq!(history.selected_index, 2);
        assert_eq!(history.window_start, 0);
        history.move_down();
        assert_eq!(history.selected_index, 3);
        assert_eq!(history.window_start, 1);
        history.move_down();
        assert_eq!(history.selected_index, 4);
        assert_eq!(history.window_start, 2);
        let nav = build_clipboard_nav_text(&history).expect("nav");
        assert!(nav.contains("> [3] one"));
    }
}
