use bytemuck::bytes_of;
use image::{ImageBuffer, Rgb};
use std::time::{Duration, Instant};
use wgpu::wgt::PollType;
use wgpu::{
    include_wgsl, Adapter, Backends, Buffer, BufferDescriptor, BufferUsages, Device, Extent3d,
    Instance, InstanceDescriptor, InstanceFlags, LoadOpDontCare, MapMode, PipelineCompilationOptions,
    Queue, ShaderModuleDescriptor, ShaderSource, Surface, TexelCopyBufferInfo,
    TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureDescriptor, TextureDimension,
    TextureFormat, TextureUsages, TextureView, TextureViewDescriptor,
};

macro_rules! default {
    () => {
        Default::default()
    };
}

pub struct State {
    target: RenderTarget,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub size: (u32, u32),
    pipeline: wgpu::RenderPipeline,
    uniform_bind_group: wgpu::BindGroup,
    texture_format: wgpu::TextureFormat,
    start: Instant,
    pub frame_n: u32,
    /* --- input uniforms --- */
    i_time_buffer: Buffer,
    i_resolution_buffer: Buffer,
    i_mouse_buffer: Buffer,
    i_frame_buffer: Buffer,
}

pub fn wgpu_instance_from_envs() -> Instance {
    Instance::new(&InstanceDescriptor {
        backends: Backends::from_env().unwrap_or_default(),
        flags: InstanceFlags::from_env_or_default(),
        memory_budget_thresholds: Default::default(),
        backend_options: Default::default(),
    })
}

pub fn wgpu_things() -> (Instance, Device, Queue, Adapter) {
    let instance = wgpu_instance_from_envs();
    let adapter = pollster::block_on(instance.request_adapter(&default!())).unwrap();

    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    (instance, device, queue, adapter)
}

fn pick_texture_format(surface: &Surface, adapter: &Adapter) -> TextureFormat {
    let surface_caps = surface.get_capabilities(adapter);

    // Do not use srgb suffix. This makes wgpu think all colors we give are already in a
    // non-linear sRGB space and do not do an automatic gamma correction.
    let mut texture_format = TextureFormat::Bgra8Unorm;
    if !surface_caps.formats.iter().any(|x| x == &texture_format) {
        texture_format = surface_caps.formats[0].remove_srgb_suffix();
    }
    texture_format
}

pub enum RenderTarget {
    Null,
    Surface(Surface<'static>),
    Offscreen {
        texture: Texture,
        view: TextureView,
        framerate: u32,
        stage_buffer: Buffer,
        size: (u32, u32),
        per_row_size_padded: u32,
        output_image_buffer: Vec<image::Rgb<u8>>,
    },
}

pub enum RenderTargetInfo {
    Offscreen { framerate: u32, size: (u32, u32) },
    Surface(Surface<'static>),
}

impl State {
    pub fn configure_surface(&self) {
        let RenderTarget::Surface(surface) = &self.target else {
            return;
        };

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
        surface.configure(&self.device, &surface_config);
    }

    pub async fn new(
        device: Device,
        queue: Queue,
        adapter: Adapter,
        size: (u32, u32),
        code: &str,
        target_info: RenderTargetInfo,
    ) -> Self {
        let shadertoy_code = code;
        let shader_fs = device.create_shader_module(ShaderModuleDescriptor {
            label: None,
            source: parse_shadertoy_code(&shadertoy_code).unwrap(),
        });
        let shader_vs = device.create_shader_module(include_wgsl!("quad.wgsl"));

        let format;
        let target;
        match target_info {
            RenderTargetInfo::Offscreen { framerate, size } => {
                // do not do gamma correction
                let ofs_format = TextureFormat::Bgra8Unorm;
                let texture = Self::create_offscreen_texture(&device, size, ofs_format);
                let view = texture.create_view(&TextureViewDescriptor {
                    format: None,
                    ..default!()
                });
                let per_row_size = size.0 * 4;
                let per_row_size_padded = ((per_row_size + 256 - 1) / 256) * 256;
                let rows = size.1;
                let stage_buffer = device.create_buffer(&BufferDescriptor {
                    label: None,
                    size: (per_row_size_padded * rows) as _,
                    usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let output_image_buffer =
                    vec![image::Rgb(default!()); size.0 as usize * size.1 as usize];

                format = ofs_format;
                target = RenderTarget::Offscreen {
                    framerate,
                    texture,
                    view,
                    stage_buffer,
                    size,
                    per_row_size_padded,
                    output_image_buffer,
                }
            }
            RenderTargetInfo::Surface(s) => {
                format = pick_texture_format(&s, &adapter);
                target = RenderTarget::Surface(s);
            }
        }

        let render_pipeline_desc = &mut wgpu::RenderPipelineDescriptor {
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
                    format,
                    blend: None,
                    write_mask: Default::default(),
                })],
            }),
            multiview_mask: None,
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            cache: None,
        };
        let pipeline = device.create_render_pipeline(render_pipeline_desc);

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
            target,
            device,
            queue,
            size,
            pipeline,
            uniform_bind_group,
            texture_format: format,
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

    fn create_offscreen_texture(
        device: &Device,
        size: (u32, u32),
        format: TextureFormat,
    ) -> Texture {
        let ofs_texture = device.create_texture(&TextureDescriptor {
            size: Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            label: None,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::COPY_SRC | TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
            sample_count: 1,
        });
        ofs_texture
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
        let i_time;
        match self.target {
            RenderTarget::Surface(_) | RenderTarget::Null => {
                i_time = [self.start.elapsed().as_secs_f32()];
            }
            RenderTarget::Offscreen { framerate, .. } => {
                i_time = [(self.frame_n as f64 / framerate as f64) as f32];
            }
        }

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

    pub fn frame_offscreen(&mut self) -> anyhow::Result<ImageBuffer<Rgb<u8>, Vec<u8>>> {
        self.write_uniforms();

        let RenderTarget::Offscreen {
            texture,
            stage_buffer,
            per_row_size_padded,
            size,
            output_image_buffer,
            ..
        } = &mut self.target
        else {
            return Err(anyhow::anyhow!("Offscreen render target not set up"));
        };

        let view = texture.create_view(&TextureViewDescriptor { ..default!() });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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

        encoder.copy_texture_to_buffer(
            TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: Default::default(),
                aspect: Default::default(),
            },
            TexelCopyBufferInfo {
                buffer: stage_buffer,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(*per_row_size_padded),
                    rows_per_image: Some(size.1),
                },
            },
            Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );

        let command_buffer = encoder.finish();

        self.queue.submit([command_buffer]);

        self.frame_n += 1;

        loop {
            if self
                .device
                .poll(PollType::Wait {
                    timeout: None,
                    submission_index: None,
                })?
                .wait_finished()
            {
                break;
            }
        }

        let channel = oneshot::channel();
        stage_buffer.map_async(MapMode::Read, .., move |r| {
            channel.0.send(r).unwrap();
        });
        self.device.poll(PollType::Wait {
            timeout: None,
            submission_index: None,
        })?;
        channel.1.recv()??;

        let mapped = stage_buffer.get_mapped_range(..);
        let gpu_buf = &*mapped;
        for row_n in 0..size.1 {
            for x in 0..size.0 as usize {
                let pix = &mut output_image_buffer[size.0 as usize * row_n as usize + x];
                let start = (*per_row_size_padded * row_n) as usize + x * 4;
                let b = gpu_buf[start];
                let g = gpu_buf[start + 1];
                let r = gpu_buf[start + 2];
                let _a = gpu_buf[start + 3];
                *pix = image::Rgb([r, g, b]);
            }
        }
        drop(mapped);
        stage_buffer.unmap();

        let image = image::RgbImage::from_fn(size.0, size.1, |x, y| {
            output_image_buffer[y as usize * size.0 as usize + x as usize]
        });
        Ok(image)
    }

    pub fn frame(
        &mut self,
        before_submit_callback: impl FnOnce(),
    ) -> Result<(), wgpu::SurfaceError> {
        self.write_uniforms();

        let RenderTarget::Surface(surface) = &self.target else {
            return Ok(());
        };

        let surface_texture = surface.get_current_texture()?;

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
