use glutin::context::GlProfile;
use vizia_window::WindowDescription;

use std::error::Error;
use std::num::NonZeroU32;
use std::{ffi::CString, sync::Arc};
use winit::raw_window_handle::HasWindowHandle;

use gl::types::*;
use glutin::{
    config::{Config, ConfigTemplateBuilder},
    context::{ContextApi, ContextAttributesBuilder},
    display::GetGlDisplay,
    prelude::*,
    surface::{SurfaceAttributesBuilder, WindowSurface},
};
use glutin_winit::DisplayBuilder;

use skia_safe::{
    gpu::{
        self, backend_render_targets, context_options, gl::FramebufferInfo, ContextOptions,
        DirectContext, SurfaceOrigin,
    },
    ColorSpace, ColorType, PixelGeometry, Surface, SurfaceProps, SurfacePropsFlags,
};

use winit::{
    dpi::PhysicalSize,
    event_loop::ActiveEventLoop,
    window::{Window, WindowId},
};

use vizia_core::prelude::*;

pub struct WinState {
    pub entity: Entity,
    gl_config: Config,
    gl_context: glutin::context::PossiblyCurrentContext,
    pub gl_surface: glutin::surface::Surface<glutin::surface::WindowSurface>,
    pub id: WindowId,
    pub gr_context: DirectContext,
    pub window: Arc<Window>,
    pub surface: skia_safe::Surface,
    pub dirty_surface: skia_safe::Surface,
    pub should_close: bool,
    #[cfg(target_os = "windows")]
    pub is_initially_cloaked: bool,
    pub is_moving_or_resizing: bool,
}

impl Drop for WinState {
    fn drop(&mut self) {
        self.gl_context.make_current(&self.gl_surface).unwrap();
    }
}

impl WinState {
    pub fn new(
        event_loop: &ActiveEventLoop,
        window: Arc<Window>,
        _window_description: &WindowDescription,
        entity: Entity,
    ) -> Result<Self, Box<dyn Error>> {
        let template = ConfigTemplateBuilder::new().with_alpha_size(8).with_transparency(true);
        let display_builder = DisplayBuilder::new();

        let (_, gl_config) = display_builder
            .build(event_loop, template, |configs| {
                // Find the config with the maximum number of samples, so our triangle will
                // be smooth.
                configs
                    .reduce(|accum, config| {
                        let transparency_check = config.supports_transparency().unwrap_or(false)
                            & !accum.supports_transparency().unwrap_or(false);

                        if transparency_check || config.num_samples() < accum.num_samples() {
                            config
                        } else {
                            accum
                        }
                    })
                    .unwrap()
            })
            .unwrap();

        let raw_window_handle = window.window_handle().unwrap().as_raw();

        let gl_display = gl_config.display();

        let context_attributes = ContextAttributesBuilder::new()
            .with_profile(GlProfile::Core)
            .with_context_api(ContextApi::OpenGl(None))
            .build(Some(raw_window_handle));

        let fallback_context_attributes = ContextAttributesBuilder::new()
            .with_profile(GlProfile::Core)
            .with_context_api(ContextApi::Gles(None))
            .build(Some(raw_window_handle));

        let not_current_gl_context = unsafe {
            gl_display.create_context(&gl_config, &context_attributes).unwrap_or_else(|_| {
                gl_display
                    .create_context(&gl_config, &fallback_context_attributes)
                    .expect("failed to create context")
            })
        };

        let (width, height): (u32, u32) = window.inner_size().into();

        let attrs = SurfaceAttributesBuilder::<WindowSurface>::new().with_srgb(Some(true)).build(
            raw_window_handle,
            NonZeroU32::new(width.max(1)).unwrap(),
            NonZeroU32::new(height.max(1)).unwrap(),
        );

        let gl_surface =
            unsafe { gl_config.display().create_window_surface(&gl_config, &attrs).unwrap() };

        let gl_context = not_current_gl_context.make_current(&gl_surface).unwrap();

        // if window_description.vsync {
        //     gl_surface
        //         .set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::new(1).unwrap()))
        //         .expect("Failed to set vsync");
        // }

        // Build skia renderer
        gl::load_with(|s| {
            gl_config.display().get_proc_address(CString::new(s).unwrap().as_c_str())
        });

        let interface = skia_safe::gpu::gl::Interface::new_load_with(|name| {
            if name == "eglGetCurrentDisplay" {
                return std::ptr::null();
            }
            gl_config.display().get_proc_address(CString::new(name).unwrap().as_c_str())
        })
        .expect("Could not create interface");

        // https://github.com/rust-skia/rust-skia/issues/476
        let mut context_options = ContextOptions::new();
        context_options.skip_gl_error_checks = context_options::Enable::Yes;

        let mut gr_context = skia_safe::gpu::direct_contexts::make_gl(interface, &context_options)
            .expect("Could not create direct context");

        let fb_info = {
            let mut fboid: GLint = 0;
            unsafe { gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut fboid) };

            FramebufferInfo {
                fboid: fboid.try_into().unwrap(),
                format: skia_safe::gpu::gl::Format::RGBA8.into(),
                ..Default::default()
            }
        };

        let num_samples = gl_config.num_samples() as usize;
        let stencil_size = gl_config.stencil_size() as usize;

        let mut surface =
            create_surface(&window, fb_info, &mut gr_context, num_samples, stencil_size);

        let inner_size = window.inner_size();

        let dirty_surface = surface
            .new_surface_with_dimensions((inner_size.width as i32, inner_size.height as i32))
            .unwrap();

        // Build our window
        Ok(WinState {
            entity,
            gl_config,
            gl_context,
            id: window.id(),
            gr_context,
            gl_surface,
            window,
            surface,
            dirty_surface,
            should_close: false,
            #[cfg(target_os = "windows")]
            is_initially_cloaked: true,
            is_moving_or_resizing: false,
        })
    }

    // Returns a reference to the winit window
    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn make_current(&mut self) {
        self.gl_context.make_current(&self.gl_surface).unwrap();
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) -> bool {
        self.gl_context.make_current(&self.gl_surface).unwrap();
        let (width, height): (u32, u32) = size.into();

        if width == 0 || height == 0 {
            return false;
        }

        let fb_info = {
            let mut fboid: GLint = 0;
            unsafe { gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut fboid) };

            FramebufferInfo {
                fboid: fboid.try_into().unwrap(),
                format: skia_safe::gpu::gl::Format::RGBA8.into(),
                ..Default::default()
            }
        };

        self.surface = create_surface(
            &self.window,
            fb_info,
            &mut self.gr_context,
            self.gl_config.num_samples() as usize,
            self.gl_config.stencil_size() as usize,
        );

        self.dirty_surface = self
            .surface
            .new_surface_with_dimensions((width.max(1) as i32, height.max(1) as i32))
            .unwrap();

        self.gl_surface.resize(
            &self.gl_context,
            NonZeroU32::new(width.max(1)).unwrap(),
            NonZeroU32::new(height.max(1)).unwrap(),
        );

        true
    }

    pub fn surfaces_mut(&mut self) -> Option<(&mut skia_safe::Surface, &mut skia_safe::Surface)> {
        Some((&mut self.surface, &mut self.dirty_surface))
    }

    pub fn swap_buffers(&mut self, _dirty_rect: BoundingBox) {
        self.gr_context.flush_and_submit();
        self.gl_surface.swap_buffers(&self.gl_context).expect("Failed to swap buffers");
    }
}

pub fn create_surface(
    window: &Window,
    fb_info: FramebufferInfo,
    gr_context: &mut DirectContext,
    num_samples: usize,
    stencil_size: usize,
) -> Surface {
    let size = window.inner_size();
    let size = (
        size.width.try_into().expect("Could not convert width"),
        size.height.try_into().expect("Could not convert height"),
    );

    let backend_render_target =
        backend_render_targets::make_gl(size, num_samples, stencil_size, fb_info);

    let surface_props = SurfaceProps::new_with_text_properties(
        SurfacePropsFlags::default(),
        PixelGeometry::default(),
        0.5,
        0.0,
    );

    gpu::surfaces::wrap_backend_render_target(
        gr_context,
        &backend_render_target,
        SurfaceOrigin::BottomLeft,
        ColorType::RGBA8888,
        ColorSpace::new_srgb(),
        Some(surface_props).as_ref(),
        // None,
    )
    .expect("Could not create skia surface")
}
