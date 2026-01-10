use std::time::Duration;

use winit::dpi::PhysicalSize;
use winit::event::{ElementState, Event, Ime, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoopBuilder};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowBuilder;

use crate::core::Core;
use crate::ui::Ui;

#[derive(Debug)]
enum AppEvent {
    BackgroundTick(u64),
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
        ui.set_text(&core.text());
        update_title(&window, &core);

        let proxy = event_loop.create_proxy();
        std::thread::spawn(move || {
            for tick in 0.. {
                std::thread::sleep(Duration::from_secs(2));
                if proxy.send_event(AppEvent::BackgroundTick(tick)).is_err() {
                    break;
                }
            }
        });

        let mut needs_redraw = true;
        let mut modifiers = winit::keyboard::ModifiersState::default();

        let result = event_loop.run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::Wait);
            match event {
                Event::UserEvent(AppEvent::BackgroundTick(tick)) => {
                    println!("[bg] tick={tick}");
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
                        }
                        WindowEvent::KeyboardInput { event, .. } => {
                            if event.state == ElementState::Pressed {
                                let mut changed = false;
                                match event.logical_key {
                                    Key::Named(NamedKey::Backspace) => {
                                        core.backspace();
                                        changed = true;
                                    }
                                    Key::Named(NamedKey::ArrowLeft) => {
                                        core.move_left();
                                        changed = true;
                                    }
                                    Key::Named(NamedKey::ArrowRight) => {
                                        core.move_right();
                                        changed = true;
                                    }
                                    Key::Named(NamedKey::ArrowUp) => {
                                        core.move_up();
                                        changed = true;
                                    }
                                    Key::Named(NamedKey::ArrowDown) => {
                                        core.move_down();
                                        changed = true;
                                    }
                                    Key::Named(NamedKey::Enter) => {
                                        core.insert_char('\n');
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
                                    ui.set_text(&core.text());
                                    update_title(&window, &core);
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

fn update_title(window: &winit::window::Window, core: &Core) {
    let cursor = core.cursor();
    window.set_title(&format!(
        "Notepad Prototype (Ln {}, Col {})",
        cursor.line + 1,
        cursor.col + 1
    ));
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
