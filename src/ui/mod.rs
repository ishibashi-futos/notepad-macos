use glyphon::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer,
};
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
    line_number_buffer: Buffer,
    buffer: Buffer,
    line_number_width: f32,
    line_number_digits: usize,
    caret_line: usize,
    caret_col: usize,
    caret_pipeline: wgpu::RenderPipeline,
    caret_vertex_buffer: wgpu::Buffer,
    caret_uniform_buffer: wgpu::Buffer,
    caret_bind_group: wgpu::BindGroup,
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

        let caret_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("caret shader"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
struct Uniforms {
    screen_size: vec2<f32>,
    color: vec4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let normalized = (input.position / uniforms.screen_size) * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    var out: VertexOutput;
    out.position = vec4<f32>(normalized, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return uniforms.color;
}
"#
                .into(),
            ),
        });

        let caret_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("caret uniforms"),
            size: std::mem::size_of::<CaretUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let caret_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("caret bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<CaretUniforms>() as u64,
                        ),
                    },
                    count: None,
                }],
            });

        let caret_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("caret bind group"),
            layout: &caret_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: caret_uniform_buffer.as_entire_binding(),
            }],
        });

        let caret_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("caret pipeline layout"),
            bind_group_layouts: &[&caret_bind_group_layout],
            push_constant_ranges: &[],
        });

        let caret_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("caret pipeline"),
            layout: Some(&caret_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &caret_shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<[f32; 2]>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &caret_shader,
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

        let caret_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("caret vertices"),
            contents: bytemuck::cast_slice(&[0.0_f32; 12]),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        Self {
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
            line_number_buffer,
            buffer,
            line_number_width,
            line_number_digits,
            caret_line: 0,
            caret_col: 0,
            caret_pipeline,
            caret_vertex_buffer,
            caret_uniform_buffer,
            caret_bind_group,
        }
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
        self.tab_buffer.set_size(
            &mut self.font_system,
            new_size.width as f32,
            TAB_BAR_HEIGHT,
        );
        self.line_number_buffer.set_size(
            &mut self.font_system,
            self.line_number_width.max(1.0),
            new_size.height as f32,
        );
        self.buffer
            .set_size(
                &mut self.font_system,
                (new_size.width as f32 - (PADDING_X + self.line_number_width)).max(1.0),
                new_size.height as f32,
            );
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
            let text_width = (self.size.width as f32 - (PADDING_X + self.line_number_width)).max(1.0);
            self.line_number_buffer.set_size(
                &mut self.font_system,
                self.line_number_width.max(1.0),
                self.size.height as f32,
            );
            self.buffer
                .set_size(&mut self.font_system, text_width, self.size.height as f32);
        }
        self.line_number_buffer.set_text(
            &mut self.font_system,
            text,
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
    }

    pub fn set_caret(&mut self, line: usize, col: usize) {
        self.caret_line = line;
        self.caret_col = col;
    }

    pub fn set_tabs(&mut self, text: &str) {
        self.tab_buffer.set_text(
            &mut self.font_system,
            text,
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
    }

    pub fn caret_rect(&self, line: usize, col: usize) -> (f64, f64, f64, f64) {
        let char_width = FONT_SIZE * CHAR_WIDTH_FACTOR;
        let (x, y) = caret_origin(line, col, self.line_number_width);
        (x as f64, y as f64, char_width as f64, LINE_HEIGHT as f64)
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
        let top = PADDING_Y + TAB_BAR_HEIGHT;
        if y < top || y > self.size.height as f32 {
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
                [
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
                        top: PADDING_Y + TAB_BAR_HEIGHT,
                        scale: 1.0,
                        bounds: TextBounds {
                            left: 0,
                            top: TAB_BAR_HEIGHT as i32,
                            right: (PADDING_X + self.line_number_width) as i32,
                            bottom: self.size.height as i32,
                        },
                        default_color: Color::rgb(120, 130, 140),
                    },
                    TextArea {
                        buffer: &self.buffer,
                        left: PADDING_X + self.line_number_width,
                        top: PADDING_Y + TAB_BAR_HEIGHT,
                        scale: 1.0,
                        bounds: TextBounds {
                            left: (PADDING_X + self.line_number_width) as i32,
                            top: TAB_BAR_HEIGHT as i32,
                            right: self.size.width as i32,
                            bottom: self.size.height as i32,
                        },
                        default_color: Color::rgb(230, 230, 230),
                    },
                ],
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

            self.text_renderer
                .render(&self.text_atlas, &mut render_pass)
                .expect("render text");
        }

        let caret_rect =
            caret_rect_pixels(self.caret_line, self.caret_col, self.line_number_width);
        let vertices = caret_vertices(caret_rect);
        self.queue.write_buffer(
            &self.caret_vertex_buffer,
            0,
            bytemuck::cast_slice(&vertices),
        );
        let uniforms = CaretUniforms {
            screen_size: [self.size.width as f32, self.size.height as f32],
            _padding: [0.0, 0.0],
            color: [0.95, 0.95, 0.95, 1.0],
        };
        self.queue.write_buffer(
            &self.caret_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("caret pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            render_pass.set_pipeline(&self.caret_pipeline);
            render_pass.set_bind_group(0, &self.caret_bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.caret_vertex_buffer.slice(..));
            render_pass.draw(0..6, 0..1);
        }

        self.queue.submit(Some(encoder.finish()));
        output.present();
        Ok(())
    }
}

#[repr(C, align(16))]
#[derive(Copy, Clone)]
struct CaretUniforms {
    screen_size: [f32; 2],
    _padding: [f32; 2],
    color: [f32; 4],
}

unsafe impl bytemuck::Pod for CaretUniforms {}
unsafe impl bytemuck::Zeroable for CaretUniforms {}

fn line_number_width_for_digits(digits: usize) -> f32 {
    let char_width = FONT_SIZE * CHAR_WIDTH_FACTOR;
    (digits as f32 * char_width) + GUTTER_PADDING_LEFT + GUTTER_PADDING_RIGHT
}

fn caret_origin(line: usize, col: usize, line_number_width: f32) -> (f32, f32) {
    let char_width = FONT_SIZE * CHAR_WIDTH_FACTOR;
    let x = PADDING_X + line_number_width + (col as f32 * char_width);
    let y = PADDING_Y + TAB_BAR_HEIGHT + (line as f32 * LINE_HEIGHT);
    (x, y)
}

fn caret_rect_pixels(line: usize, col: usize, line_number_width: f32) -> (f32, f32, f32, f32) {
    let (x, y) = caret_origin(line, col, line_number_width);
    (x, y, 2.0, LINE_HEIGHT)
}

fn caret_vertices(rect: (f32, f32, f32, f32)) -> [f32; 12] {
    let (x, y, w, h) = rect;
    [
        x,
        y,
        x + w,
        y,
        x + w,
        y + h,
        x,
        y,
        x + w,
        y + h,
        x,
        y + h,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_origin_accounts_for_gutter_width() {
        let gutter = line_number_width_for_digits(3);
        let (x, y) = caret_origin(0, 0, gutter);
        assert!((x - (PADDING_X + gutter)).abs() < f32::EPSILON);
        assert!((y - (PADDING_Y + TAB_BAR_HEIGHT)).abs() < f32::EPSILON);
    }

    #[test]
    fn caret_vertices_builds_two_triangles() {
        let vertices = caret_vertices((10.0, 20.0, 2.0, 5.0));
        assert_eq!(vertices.len(), 12);
        assert_eq!(vertices[0], 10.0);
        assert_eq!(vertices[1], 20.0);
        assert_eq!(vertices[10], 10.0);
        assert_eq!(vertices[11], 25.0);
    }
}
