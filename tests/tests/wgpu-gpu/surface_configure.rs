//! Tests of `Surface::configure` on a Metal layer, which needs no window.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use wgpu::{Backends, BufferDescriptor, BufferUsages, MapMode, PollType};
use wgpu_test::{gpu_test, FailureCase, GpuTestConfiguration, GpuTestInitializer, TestParameters};

pub fn all_tests(vec: &mut Vec<GpuTestInitializer>) {
    vec.push(REFUSED_CONFIGURATION_FIRES_THE_MAP_CALLBACKS_OF_ITS_WAIT);
}

/// A configuration waits for the GPU, which completes the maps of the work it waited for. A
/// configuration that is refused after that wait still fires their callbacks: a callback that
/// never fires leaves its buffer mapped, and the owner of the buffer never learns it.
#[gpu_test]
static REFUSED_CONFIGURATION_FIRES_THE_MAP_CALLBACKS_OF_ITS_WAIT: GpuTestConfiguration =
    GpuTestConfiguration::new()
        .parameters(TestParameters::default().skip(FailureCase::backend(!Backends::METAL)))
        .run_sync(|ctx| {
            let layer = objc2_quartz_core::CAMetalLayer::new();
            layer.setDrawableSize(objc2_core_foundation::CGSize::new(8.0, 8.0));
            let layer_ptr = objc2::rc::Retained::as_ptr(&layer).cast_mut().cast();
            // SAFETY: `layer` is a `CAMetalLayer` that outlives the surface.
            let surface = unsafe {
                ctx.instance
                    .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer_ptr))
            }
            .unwrap();
            let config = surface.get_default_config(&ctx.adapter, 8, 8).unwrap();
            surface.configure(&ctx.device, &config);
            // A texture of the surface that is not presented makes the next configuration fail.
            let wgpu::CurrentSurfaceTexture::Success(frame) = surface.get_current_texture() else {
                panic!("the surface gave no texture");
            };

            let buffer = ctx.device.create_buffer(&BufferDescriptor {
                label: None,
                size: 4,
                usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = ctx.device.create_command_encoder(&Default::default());
            encoder.clear_buffer(&buffer, 0, None);
            ctx.queue.submit([encoder.finish()]);
            let fired = Arc::new(AtomicBool::new(false));
            buffer.slice(..).map_async(MapMode::Read, {
                let fired = fired.clone();
                move |result| {
                    result.unwrap();
                    fired.store(true, Ordering::SeqCst);
                }
            });

            let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
            surface.configure(&ctx.device, &config);
            let refused = pollster::block_on(scope.pop());
            assert!(refused.is_some(), "the configuration was not refused");
            ctx.device.poll(PollType::wait_indefinitely()).unwrap();
            // The refused configuration also leaves the surface unconfigured, so the texture
            // finds no presentation to go back to.
            let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
            drop(frame);
            let _ = pollster::block_on(scope.pop());

            assert!(fired.load(Ordering::SeqCst), "the map callback never fired");
        });
