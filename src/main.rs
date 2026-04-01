#![feature(try_blocks)]

use anyhow::anyhow;
use clap::Parser;
use shadertoy_wgpu::{Fps, State};
use std::env;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use wgpu::{Backends, Instance, InstanceDescriptor, InstanceFlags};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};
use winit::{
    event::*,
    event_loop::{ControlFlow, EventLoop},
};

struct Config {
    code: String,
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
                        .with_inner_size(LogicalSize::new(1024 / 2, 1024 / 2)),
                )
                .unwrap(),
        );

        pollster::block_on(async {
            let result: anyhow::Result<()> = try {
                let size = window.inner_size();
                // let size = (1024, 1024);
                let instance = Instance::new(&InstanceDescriptor {
                    backends: Backends::from_env().unwrap_or_default(),
                    flags: InstanceFlags::from_env_or_default(),
                    memory_budget_thresholds: Default::default(),
                    backend_options: Default::default(),
                });
                let surface = instance.create_surface(Arc::clone(&window))?;
                let state =
                    State::new(instance, surface, (size.width, size.height), &config.code).await;
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
                        println!("FPS: {}", fps);
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
}

pub fn main() -> anyhow::Result<()> {
    unsafe {
        env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    let args = Args::parse();
    if !args.code.exists() {
        return Err(anyhow!(
            "Shadertoy shader file '{}' does not exist.",
            args.code.display()
        ));
    }
    let mut shadertoy_code = String::new();
    File::open(args.code)?.read_to_string(&mut shadertoy_code)?;

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::default();
    app.config = Some(Config {
        code: shadertoy_code,
    });
    event_loop.run_app(&mut app)?;

    Ok(())
}
