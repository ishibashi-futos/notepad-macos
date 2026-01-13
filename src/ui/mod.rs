use glyphon::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer,
};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use wgpu::SurfaceError;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::Window;

pub struct Ui {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    font_system: FontSystem,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    cache: SwashCache,
    tab_buffer: Buffer,
    search_buffer: Buffer,
    search_nav_buffer: Buffer,
    line_number_buffer: Buffer,
    buffer: Buffer,
    line_number_width: f32,
    line_number_digits: usize,
    search_visible: bool,
    search_nav_visible: bool,
    selection_rects: Vec<(f32, f32, f32, f32)>,
    selection_vertices: Vec<SelectionVertex>,
    selection_buffer: wgpu::Buffer,
    selection_vertex_count: u32,
    selection_pipeline: wgpu::RenderPipeline,
}

const FONT_SIZE: f32 = 18.0;
const LINE_HEIGHT: f32 = 24.0;
const PADDING_X: f32 = 16.0;
const PADDING_Y: f32 = 16.0;
const GUTTER_PADDING_LEFT: f32 = 8.0;
const GUTTER_PADDING_RIGHT: f32 = 12.0;
const CHAR_WIDTH_FACTOR: f32 = 0.6;
const TAB_FONT_SIZE: f32 = 14.0;
const TAB_LINE_HEIGHT: f32 = 20.0;
const TAB_BAR_HEIGHT: f32 = 28.0;
const SEARCH_BAR_HEIGHT: f32 = 24.0;
const SEARCH_NAV_HEIGHT: f32 = 24.0;
const SELECTION_COLOR: [f32; 4] = [0.2, 0.45, 0.9, 0.35];
const SELECTION_SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(input.position, 0.0, 1.0);
    out.color = input.color;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SelectionVertex {
    position: [f32; 2],
    color: [f32; 4],
}

impl Ui {
    pub async fn new(window: &Window) -> Self {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..Default::default()
        });
        let surface = instance.create_surface(window).unwrap();
        // Safety: the window is kept alive for the duration of the app.
        let surface = unsafe {
            std::mem::transmute::<wgpu::Surface<'_>, wgpu::Surface<'static>>(surface)
        };
        let adapter = if let Some(adapter) = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
        {
            adapter
        } else {
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: true,
                })
                .await
                .expect("Failed to find an appropriate adapter")
        };

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .expect("Failed to create device");

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps.formats[0];
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: surface_caps.present_modes[0],
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let mut font_system = FontSystem::new();
        let cache = SwashCache::new();
        let mut text_atlas = TextAtlas::new(&device, &queue, config.format);
        let text_renderer =
            TextRenderer::new(&mut text_atlas, &device, wgpu::MultisampleState::default(), None);
        let selection_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("selection shader"),
            source: wgpu::ShaderSource::Wgsl(SELECTION_SHADER.into()),
        });
        let selection_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("selection pipeline layout"),
                bind_group_layouts: &[],
                push_constant_ranges: &[],
            });
        let selection_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("selection pipeline"),
            layout: Some(&selection_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &selection_shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SelectionVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 8,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &selection_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let selection_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("selection buffer"),
            size: 1,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut tab_buffer = Buffer::new(&mut font_system, Metrics::new(TAB_FONT_SIZE, TAB_LINE_HEIGHT));
        tab_buffer.set_size(
            &mut font_system,
            size.width as f32,
            TAB_BAR_HEIGHT,
        );
        tab_buffer.set_text(
            &mut font_system,
            "",
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );

        let mut search_buffer = Buffer::new(&mut font_system, Metrics::new(TAB_FONT_SIZE, TAB_LINE_HEIGHT));
        search_buffer.set_size(
            &mut font_system,
            size.width as f32,
            SEARCH_BAR_HEIGHT,
        );
        search_buffer.set_text(
            &mut font_system,
            "",
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );

        let mut search_nav_buffer =
            Buffer::new(&mut font_system, Metrics::new(TAB_FONT_SIZE, TAB_LINE_HEIGHT));
        search_nav_buffer.set_size(
            &mut font_system,
            size.width as f32,
            SEARCH_NAV_HEIGHT,
        );
        search_nav_buffer.set_text(
            &mut font_system,
            "",
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );

        let line_number_digits = 1;
        let line_number_width = line_number_width_for_digits(line_number_digits);
        let mut line_number_buffer = Buffer::new(&mut font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        line_number_buffer.set_size(&mut font_system, line_number_width, size.height as f32);
        line_number_buffer.set_text(
            &mut font_system,
            "",
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );

        let mut buffer = Buffer::new(&mut font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        let text_width = (size.width as f32 - (PADDING_X + line_number_width)).max(1.0);
        buffer.set_size(&mut font_system, text_width, size.height as f32);
        buffer.set_text(
            &mut font_system,
            "",
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );

        let mut ui = Self {
            surface,
            device,
            queue,
            config,
            size,
            font_system,
            text_atlas,
            text_renderer,
            cache,
            tab_buffer,
            search_buffer,
            search_nav_buffer,
            line_number_buffer,
            buffer,
            line_number_width,
            line_number_digits,
            search_visible: false,
            search_nav_visible: false,
            selection_rects: Vec::new(),
            selection_vertices: Vec::new(),
            selection_buffer,
            selection_vertex_count: 0,
            selection_pipeline,
        };
        ui.update_layout_sizes();
        ui
    }

    pub fn size(&self) -> PhysicalSize<u32> {
        self.size
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.size = new_size;
        self.config.width = new_size.width;
        self.config.height = new_size.height;
        self.surface.configure(&self.device, &self.config);
        self.update_layout_sizes();
        self.update_selection_vertices();
    }

    pub fn set_text(&mut self, text: &str) {
        self.buffer.set_text(
            &mut self.font_system,
            text,
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
    }

    pub fn set_line_numbers(&mut self, text: &str, digits: usize) {
        let digits = digits.max(1);
        if digits != self.line_number_digits {
            self.line_number_digits = digits;
            self.line_number_width = line_number_width_for_digits(digits);
            let text_width =
                (self.size.width as f32 - (PADDING_X + self.line_number_width)).max(1.0);
            let content_height = self.content_height();
            self.line_number_buffer.set_size(
                &mut self.font_system,
                self.line_number_width.max(1.0),
                content_height,
            );
            self.buffer
                .set_size(&mut self.font_system, text_width, content_height);
        }
        self.line_number_buffer.set_text(
            &mut self.font_system,
            text,
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
    }

    pub fn set_tabs(&mut self, text: &str) {
        self.tab_buffer.set_text(
            &mut self.font_system,
            text,
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
    }

    pub fn set_search(&mut self, text: &str, visible: bool) {
        let visibility_changed = self.search_visible != visible;
        self.search_visible = visible;
        if visibility_changed {
            self.update_layout_sizes();
        }
        self.search_buffer.set_text(
            &mut self.font_system,
            text,
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
    }

    pub fn set_search_navigation(&mut self, text: &str, visible: bool) {
        let visibility_changed = self.search_nav_visible != visible;
        self.search_nav_visible = visible;
        if visibility_changed {
            self.update_layout_sizes();
        }
        self.search_nav_buffer.set_text(
            &mut self.font_system,
            text,
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
    }

    pub fn set_selection_rects(&mut self, rects: &[(f32, f32, f32, f32)]) {
        self.selection_rects.clear();
        self.selection_rects.extend_from_slice(rects);
        self.update_selection_vertices();
    }

    pub fn caret_rect(&self, line: usize, col: usize) -> (f64, f64, f64, f64) {
        let char_width = FONT_SIZE * CHAR_WIDTH_FACTOR;
        let x = PADDING_X + self.line_number_width + (col as f32 * char_width);
        let y = self.content_top() + (line as f32 * LINE_HEIGHT);
        (x as f64, y as f64, char_width as f64, LINE_HEIGHT as f64)
    }

    pub fn selection_rect(
        &self,
        line: usize,
        start_col: usize,
        end_col: usize,
    ) -> (f32, f32, f32, f32) {
        let char_width = FONT_SIZE * CHAR_WIDTH_FACTOR;
        let x = PADDING_X + self.line_number_width + (start_col as f32 * char_width);
        let y = self.content_top() + (line as f32 * LINE_HEIGHT);
        let width = (end_col.saturating_sub(start_col) as f32) * char_width;
        (x, y, width, LINE_HEIGHT)
    }

    pub fn line_number_hit_test(
        &self,
        position: PhysicalPosition<f64>,
        line_count: usize,
    ) -> Option<usize> {
        let x = position.x as f32;
        let y = position.y as f32;
        let gutter_left = PADDING_X;
        let gutter_right = PADDING_X + self.line_number_width;
        if x < gutter_left || x > gutter_right {
            return None;
        }
        let top = self.content_top();
        if y < top || y > (self.content_top() + self.content_height()) {
            return None;
        }
        let line = ((y - top) / LINE_HEIGHT).floor() as usize;
        if line >= line_count.max(1) {
            return None;
        }
        Some(line)
    }

    pub fn render(&mut self) -> Result<(), SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let content_top = self.content_top();
        let content_bottom_y = self.content_bottom_y();
        let search_nav_top = self.search_nav_top();

        self.text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.text_atlas,
                Resolution {
                    width: self.size.width,
                    height: self.size.height,
                },
                if self.search_visible || self.search_nav_visible {
                    vec![
                        TextArea {
                            buffer: &self.tab_buffer,
                            left: PADDING_X,
                            top: PADDING_Y,
                            scale: 1.0,
                            bounds: TextBounds {
                                left: 0,
                                top: 0,
                                right: self.size.width as i32,
                                bottom: (PADDING_Y + TAB_BAR_HEIGHT) as i32,
                            },
                            default_color: Color::rgb(180, 190, 200),
                        },
                        TextArea {
                            buffer: &self.search_buffer,
                            left: PADDING_X,
                            top: PADDING_Y + TAB_BAR_HEIGHT,
                            scale: 1.0,
                            bounds: TextBounds {
                                left: 0,
                                top: TAB_BAR_HEIGHT as i32,
                                right: self.size.width as i32,
                                bottom: (PADDING_Y + TAB_BAR_HEIGHT + SEARCH_BAR_HEIGHT) as i32,
                            },
                            default_color: Color::rgb(200, 210, 170),
                        },
                        TextArea {
                            buffer: &self.search_nav_buffer,
                            left: PADDING_X,
                            top: search_nav_top,
                            scale: 1.0,
                            bounds: TextBounds {
                                left: 0,
                                top: search_nav_top as i32,
                                right: self.size.width as i32,
                                bottom: (self.size.height as f32 - PADDING_Y) as i32,
                            },
                            default_color: Color::rgb(170, 190, 210),
                        },
                        TextArea {
                            buffer: &self.line_number_buffer,
                            left: PADDING_X,
                            top: content_top,
                            scale: 1.0,
                            bounds: TextBounds {
                                left: 0,
                                top: (TAB_BAR_HEIGHT + SEARCH_BAR_HEIGHT) as i32,
                                right: (PADDING_X + self.line_number_width) as i32,
                                bottom: content_bottom_y as i32,
                            },
                            default_color: Color::rgb(120, 130, 140),
                        },
                        TextArea {
                            buffer: &self.buffer,
                            left: PADDING_X + self.line_number_width,
                            top: content_top,
                            scale: 1.0,
                            bounds: TextBounds {
                                left: (PADDING_X + self.line_number_width) as i32,
                                top: (TAB_BAR_HEIGHT + SEARCH_BAR_HEIGHT) as i32,
                                right: self.size.width as i32,
                                bottom: content_bottom_y as i32,
                            },
                            default_color: Color::rgb(230, 230, 230),
                        },
                    ]
                } else {
                    vec![
                        TextArea {
                            buffer: &self.tab_buffer,
                            left: PADDING_X,
                            top: PADDING_Y,
                            scale: 1.0,
                            bounds: TextBounds {
                                left: 0,
                                top: 0,
                                right: self.size.width as i32,
                                bottom: (PADDING_Y + TAB_BAR_HEIGHT) as i32,
                            },
                            default_color: Color::rgb(180, 190, 200),
                        },
                        TextArea {
                            buffer: &self.line_number_buffer,
                            left: PADDING_X,
                            top: content_top,
                            scale: 1.0,
                            bounds: TextBounds {
                                left: 0,
                                top: TAB_BAR_HEIGHT as i32,
                                right: (PADDING_X + self.line_number_width) as i32,
                                bottom: content_bottom_y as i32,
                            },
                            default_color: Color::rgb(120, 130, 140),
                        },
                        TextArea {
                            buffer: &self.buffer,
                            left: PADDING_X + self.line_number_width,
                            top: content_top,
                            scale: 1.0,
                            bounds: TextBounds {
                                left: (PADDING_X + self.line_number_width) as i32,
                                top: TAB_BAR_HEIGHT as i32,
                                right: self.size.width as i32,
                                bottom: content_bottom_y as i32,
                            },
                            default_color: Color::rgb(230, 230, 230),
                        },
                    ]
                },
                &mut self.cache,
            )
            .expect("prepare text");

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.08,
                            g: 0.09,
                            b: 0.1,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            if self.selection_vertex_count > 0 {
                render_pass.set_pipeline(&self.selection_pipeline);
                render_pass.set_vertex_buffer(0, self.selection_buffer.slice(..));
                render_pass.draw(0..self.selection_vertex_count, 0..1);
            }

            self.text_renderer
                .render(&self.text_atlas, &mut render_pass)
                .expect("render text");
        }

        self.queue.submit(Some(encoder.finish()));
        output.present();
        Ok(())
    }

    fn content_top(&self) -> f32 {
        PADDING_Y + TAB_BAR_HEIGHT + if self.search_visible { SEARCH_BAR_HEIGHT } else { 0.0 }
    }

    fn content_bottom_inset(&self) -> f32 {
        if self.search_nav_visible {
            PADDING_Y + SEARCH_NAV_HEIGHT
        } else {
            0.0
        }
    }

    fn content_height(&self) -> f32 {
        (self.size.height as f32 - self.content_top() - self.content_bottom_inset()).max(1.0)
    }

    fn content_bottom_y(&self) -> f32 {
        self.size.height as f32 - self.content_bottom_inset()
    }

    fn search_nav_top(&self) -> f32 {
        self.size.height as f32 - SEARCH_NAV_HEIGHT - PADDING_Y
    }

    fn update_selection_vertices(&mut self) {
        self.selection_vertices.clear();
        if self.selection_rects.is_empty() {
            self.selection_vertex_count = 0;
            return;
        }
        let width = self.size.width as f32;
        let height = self.size.height as f32;
        if width <= 0.0 || height <= 0.0 {
            self.selection_vertex_count = 0;
            return;
        }
        for &(x, y, w, h) in &self.selection_rects {
            self.selection_vertices
                .extend_from_slice(&self.rect_to_vertices(x, y, w, h, width, height));
        }
        self.selection_vertex_count = self.selection_vertices.len() as u32;
        if self.selection_vertex_count == 0 {
            return;
        }
        self.selection_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("selection buffer"),
                contents: bytemuck::cast_slice(&self.selection_vertices),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
    }

    fn rect_to_vertices(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        width: f32,
        height: f32,
    ) -> [SelectionVertex; 6] {
        let left = (x / width) * 2.0 - 1.0;
        let right = ((x + w) / width) * 2.0 - 1.0;
        let top = 1.0 - (y / height) * 2.0;
        let bottom = 1.0 - ((y + h) / height) * 2.0;
        let color = SELECTION_COLOR;
        [
            SelectionVertex {
                position: [left, top],
                color,
            },
            SelectionVertex {
                position: [right, top],
                color,
            },
            SelectionVertex {
                position: [right, bottom],
                color,
            },
            SelectionVertex {
                position: [left, top],
                color,
            },
            SelectionVertex {
                position: [right, bottom],
                color,
            },
            SelectionVertex {
                position: [left, bottom],
                color,
            },
        ]
    }

    fn update_layout_sizes(&mut self) {
        let content_height = self.content_height();
        self.tab_buffer.set_size(
            &mut self.font_system,
            self.size.width as f32,
            TAB_BAR_HEIGHT,
        );
        self.search_buffer.set_size(
            &mut self.font_system,
            self.size.width as f32,
            SEARCH_BAR_HEIGHT,
        );
        self.search_nav_buffer.set_size(
            &mut self.font_system,
            self.size.width as f32,
            SEARCH_NAV_HEIGHT,
        );
        self.line_number_buffer.set_size(
            &mut self.font_system,
            self.line_number_width.max(1.0),
            content_height,
        );
        let text_width = (self.size.width as f32 - (PADDING_X + self.line_number_width)).max(1.0);
        self.buffer
            .set_size(&mut self.font_system, text_width, content_height);
    }
}

fn line_number_width_for_digits(digits: usize) -> f32 {
    let char_width = FONT_SIZE * CHAR_WIDTH_FACTOR;
    (digits as f32 * char_width) + GUTTER_PADDING_LEFT + GUTTER_PADDING_RIGHT
}
