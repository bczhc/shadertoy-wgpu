#![feature(try_blocks)]

use anyhow::anyhow;
use clap::Parser;
use shadertoy_wgpu::{wgpu_things, Fps, RenderTargetInfo, State};
use std::env;
use std::fs::File;
use std::io::{stdout, Read, Write};
use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};
use winit::{
    event::*,
    event_loop::{ControlFlow, EventLoop},
};

struct Config {
    code: String,
    args: Arc<Args>,
}

#[derive(Default)]
struct App {
    pub state: Option<State>,
    pub window: Option<Arc<Window>>,
    pub fps: Option<Fps>,
    pub frame_counter: usize,
    pub config: Option<Config>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let config = self.config.as_ref().expect("Config is missing");

        // Create window object
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_resizable(!config.args.fixed_window)
                        .with_inner_size(PhysicalSize::new(config.args.width, config.args.height)),
                )
                .unwrap(),
        );

        pollster::block_on(async {
            let result: anyhow::Result<()> = try {
                let size = window.inner_size();
                // let size = (1024, 1024);
                let (instance, device, queue, adapter) = wgpu_things();
                let surface = instance.create_surface(Arc::clone(&window))?;
                let state = State::new(
                    device,
                    queue,
                    adapter,
                    (size.width, size.height),
                    &config.code,
                    RenderTargetInfo::Surface(surface),
                )
                .await;
                self.state = Some(state);
            };
            result
        })
        .unwrap();

        window.request_redraw();
        self.window = Some(window);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let state = self.state.as_mut().unwrap();
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(physical_size) => {
                state.resize((physical_size.width, physical_size.height))
            }
            WindowEvent::RedrawRequested => {
                let Some(w) = &self.window else {
                    return;
                };

                match state.frame(|| w.pre_present_notify()) {
                    Ok(_) => {}
                    Err(wgpu::SurfaceError::Lost) => state.configure_surface(),
                    Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                    Err(wgpu::SurfaceError::Outdated) => state.configure_surface(),
                    Err(e) => eprintln!("{:?}", e),
                }
                self.frame_counter += 1;
                if self.frame_counter == 10 {
                    // event_loop.exit();
                }

                // print the FPS
                if let Some(f) = &mut self.fps {
                    let (d, fps) = f.hint_and_get();
                    if d.as_secs_f64() > 1.0 {
                        eprintln!("FPS: {}", fps);
                        self.fps = Some(Fps::new());
                    }
                } else {
                    self.fps = Some(Fps::new());
                }

                w.request_redraw();
            }
            _ => {}
        }
    }
}

#[derive(Parser, Default)]
struct Args {
    /// Shadertoy fragment shader code
    #[arg(value_hint = clap::ValueHint::FilePath, default_value = "shadertoy.frag")]
    code: PathBuf,
    /// Window width
    #[arg(long, default_value = "1280")]
    width: u32,
    /// Window height
    #[arg(long, default_value = "720")]
    height: u32,
    /// Whether to make the window unresizable.
    #[arg(long, default_value = "false")]
    fixed_window: bool,
    /// Framerate. This will affect how `iTime` is calculated. Only use along with `offscreen`.
    #[arg(short, long)]
    framerate: Option<u32>,
    /// Offscreen rendering mode, producing png stream.
    #[arg(short, long)]
    offscreen: bool,
}

fn start_window(config: Config) -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::default();
    app.config = Some(config);
    event_loop.run_app(&mut app)?;
    Ok(())
}

fn render_offscreen(config: Config) -> anyhow::Result<()> {
    let (instance, device, queue, adapter) = wgpu_things();

    let state = State::new(
        device,
        queue,
        adapter,
        (config.args.width, config.args.height),
        &config.code,
        RenderTargetInfo::Offscreen {
            framerate: config.args.framerate.unwrap_or(0),
            size: (config.args.width, config.args.height),
        },
    );
    let mut state = pollster::block_on(state);

    loop {
        eprintln!("Offscreen frame: {}", state.frame_n);
        let mut stdout = stdout();
        let result = state.frame_offscreen(|image_buf| {
            // output the raw buffer to stdout, allowing being piped to ffmpeg
            // buffer format: bgra8888
            stdout.write_all(image_buf).unwrap();
        });
        result?;
        eprintln!("Offscreen frame: {}", state.frame_n);
    }

    Ok(())
}

pub fn main() -> anyhow::Result<()> {
    unsafe {
        env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    let args = Arc::new(Args::parse());
    if !args.code.exists() {
        return Err(anyhow!(
            "Shadertoy shader file '{}' does not exist.",
            args.code.display()
        ));
    }
    let mut shadertoy_code = String::new();
    File::open(&args.code)?.read_to_string(&mut shadertoy_code)?;
    let config = Config {
        code: shadertoy_code,
        args: Arc::clone(&args),
    };

    if !args.offscreen {
        start_window(config)?;
    } else {
        render_offscreen(config)?;
    }

    Ok(())
}
