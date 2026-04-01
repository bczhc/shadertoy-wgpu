use bytemuck::{bytes_of, Pod, Zeroable};
use std::time::{Duration, Instant};
use wgpu::{
    include_wgsl, Buffer, BufferDescriptor, BufferUsages, Device, Instance,
    LoadOpDontCare, PipelineCompilationOptions, ShaderModuleDescriptor, ShaderSource, Surface,
    TextureFormat,
};

macro_rules! default {
    () => {
        Default::default()
    };
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Uniforms {
    origin: [f32; 3],
    padding1: f32,
    right: [f32; 3],
    padding2: f32,
    up: [f32; 3],
    padding3: f32,
    forward: [f32; 3],
    padding4: f32,
    screen_size: [f32; 2],
    len: f32,
    padding5: f32,
}

pub struct State {
    surface: Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub size: (u32, u32),
    pipeline: wgpu::RenderPipeline,
    uniform_bind_group: wgpu::BindGroup,
    texture_format: wgpu::TextureFormat,
    start: Instant,
    frame_n: u32,
    /* --- input uniforms --- */
    i_time_buffer: Buffer,
    i_resolution_buffer: Buffer,
    i_mouse_buffer: Buffer,
    i_frame_buffer: Buffer,
}

impl State {
    pub fn configure_surface(&self) {
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: self.texture_format,
            view_formats: vec![self.texture_format],
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            width: self.size.0,
            height: self.size.1,
            desired_maximum_frame_latency: 2,
            present_mode: wgpu::PresentMode::AutoVsync,
        };
        self.surface.configure(&self.device, &surface_config);
    }

    pub async fn new(
        instance: Instance,
        surface: Surface<'static>,
        size: (u32, u32),
        code: &str,
    ) -> Self {
        let adapter = instance.request_adapter(&default!()).await.unwrap();

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .unwrap();

        let surface_caps = surface.get_capabilities(&adapter);

        // Do not use srgb suffix. This makes wgpu think all colors we give are already in a
        // non-linear sRGB space and do not do an automatic gamma correction.
        let mut texture_format = TextureFormat::Bgra8Unorm;
        if !surface_caps.formats.iter().any(|x| x == &texture_format) {
            texture_format = surface_caps.formats[0].remove_srgb_suffix();
        }

        let shadertoy_code = code;
        let shader_fs = device.create_shader_module(ShaderModuleDescriptor {
            label: None,
            source: parse_shadertoy_code(&shadertoy_code).unwrap(),
        });
        let shader_vs = device.create_shader_module(include_wgsl!("quad.wgsl"));

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader_vs,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_fs,
                entry_point: None,
                compilation_options: PipelineCompilationOptions {
                    zero_initialize_workgroup_memory: default!(),
                    constants: &[],
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format: texture_format,
                    blend: None,
                    write_mask: Default::default(),
                })],
            }),
            multiview_mask: None,
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            cache: None,
        });

        let i_time_buffer = Self::create_uniform_buffer(&device, 4);
        let i_resolution_buffer = Self::create_uniform_buffer(&device, 12);
        let i_mouse_buffer = Self::create_uniform_buffer(&device, 16);
        let i_frame_buffer = Self::create_uniform_buffer(&device, 4);

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: i_resolution_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: i_time_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: i_mouse_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: i_frame_buffer.as_entire_binding(),
                },
            ],
            label: Some("uniform_bind_group"),
        });

        let state = Self {
            surface,
            device,
            queue,
            size,
            pipeline,
            uniform_bind_group,
            texture_format,
            start: Instant::now(),
            frame_n: 0,
            i_time_buffer,
            i_resolution_buffer,
            i_mouse_buffer,
            i_frame_buffer,
        };
        state.configure_surface();
        state
    }

    fn create_uniform_buffer(device: &Device, size: u64) -> Buffer {
        device.create_buffer(&BufferDescriptor {
            label: None,
            size,
            mapped_at_creation: false,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        })
    }

    pub fn resize(&mut self, new_size: (u32, u32)) {
        self.size = new_size;

        // reconfigure the surface
        self.configure_surface();
    }

    fn write_uniforms(&self) {
        let i_time = [self.start.elapsed().as_secs_f32()];
        let i_resolution = [self.size.0 as f32, self.size.1 as f32, 1f32];
        let i_mouse = [0_f32 /* placeholder */; 4];
        self.queue
            .write_buffer(&self.i_time_buffer, 0, bytes_of(&i_time));
        self.queue
            .write_buffer(&self.i_resolution_buffer, 0, bytes_of(&i_resolution));
        self.queue
            .write_buffer(&self.i_mouse_buffer, 0, bytes_of(&i_mouse));
        self.queue
            .write_buffer(&self.i_frame_buffer, 0, bytes_of(&[self.frame_n]));
    }

    pub fn frame(
        &mut self,
        before_submit_callback: impl FnOnce(),
    ) -> Result<(), wgpu::SurfaceError> {
        self.write_uniforms();

        let surface_texture = self.surface.get_current_texture()?;

        let texture_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                format: Some(self.texture_format),
                ..Default::default()
            });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &texture_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::DontCare(LoadOpDontCare::default()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        let command_buffer = encoder.finish();

        before_submit_callback();
        self.queue.submit([command_buffer]);
        surface_texture.present();

        self.frame_n += 1;
        Ok(())
    }
}

pub struct Fps {
    instant: Instant,
    counter: usize,
}

impl Fps {
    pub fn new() -> Self {
        Self {
            instant: Instant::now(),
            counter: 0,
        }
    }

    pub fn hint_and_get(&mut self) -> (Duration, f32) {
        self.counter += 1;
        let duration = self.instant.elapsed();
        (
            duration,
            (self.counter as f64 / duration.as_secs_f64()) as f32,
        )
    }
}

pub fn parse_shadertoy_code(code: &str) -> anyhow::Result<ShaderSource> {
    use naga::ShaderStage;
    use naga::back::wgsl::WriterFlags;
    use naga::front::glsl::Options;
    use naga::valid::{Capabilities, ValidationFlags, Validator};

    let template = include_str!("full_glsl.frag");
    let full_glsl_code = template.replace("MAIN_IMAGE;", code);

    let module = naga::front::glsl::Frontend::default().parse(
        &Options {
            stage: ShaderStage::Fragment,
            defines: Default::default(),
        },
        &full_glsl_code,
    )?;

    let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
    let module_info = validator.validate(&module)?;

    let wgsl_code = naga::back::wgsl::write_string(&module, &module_info, WriterFlags::all())?;
    Ok(ShaderSource::Wgsl(wgsl_code.into()))
}
