//! A GL context that another thread holds past the lock's timeout loses the device: the waiting
//! thread does not abort the process. On Android the engine's decode threads unmap staging buffers
//! while the engine thread presents, and a present that blocks held the lock for 6 s.
//!
//! The test needs an EGL with a GL driver, as Mesa's llvmpipe gives on a Mac
//! (`DYLD_FALLBACK_LIBRARY_PATH=/opt/homebrew/lib`). It skips with no GL adapter.

#![cfg(all(feature = "gles", not(any(windows, target_arch = "wasm32"))))]

use std::time::Duration;

use wgpu_hal::{self as hal, Adapter as _, Device as _, Instance as _};
use wgpu_types as wgt;

/// Longer than the lock's timeout, so the other thread waits past it.
const HELD: Duration = Duration::from_secs(8);

fn device() -> Option<(hal::gles::Instance, hal::OpenDevice<hal::api::Gles>)> {
    let instance = unsafe {
        hal::gles::Instance::init(&hal::InstanceDescriptor {
            name: "gles_context_lock",
            flags: wgt::InstanceFlags::empty(),
            memory_budget_thresholds: wgt::MemoryBudgetThresholds::default(),
            backend_options: wgt::BackendOptions::default(),
            telemetry: None,
            display: None,
        })
    }
    .ok()?;
    let exposed = unsafe { instance.enumerate_adapters(None) }
        .into_iter()
        .next()?;
    let open = unsafe {
        exposed.adapter.open(
            wgt::Features::empty(),
            &wgt::Limits::downlevel_webgl2_defaults(),
            &wgt::MemoryHints::default(),
        )
    }
    .ok()?;
    Some((instance, open))
}

#[test]
fn a_context_held_past_the_timeout_loses_the_device() {
    let Some((_instance, open)) = device() else {
        eprintln!("no GL adapter, skipped");
        return;
    };
    let device = &open.device;
    let buffer = unsafe {
        device.create_buffer(&hal::BufferDescriptor {
            label: Some("staging"),
            size: 256,
            usage: wgt::BufferUses::MAP_WRITE | wgt::BufferUses::COPY_SRC,
            memory_flags: hal::MemoryFlags::empty(),
        })
    }
    .expect("a buffer");
    unsafe { device.map_buffer(&buffer, 0..256) }.expect("a mapping");
    let fence = unsafe { device.create_fence() }.expect("a fence");

    let (held, release) = std::sync::mpsc::channel::<()>();
    let unmapped = std::thread::scope(|scope| {
        scope.spawn(|| {
            let _lock = device.context().lock();
            held.send(()).expect("the test waits");
            std::thread::sleep(HELD);
        });
        release.recv().expect("the lock is held");
        // As a decode thread does when it ends its write.
        scope
            .spawn(|| unsafe {
                device.flush_mapped_ranges(&buffer, core::iter::once(0..256));
                device.unmap_buffer(&buffer);
            })
            .join()
    });
    assert!(
        unmapped.is_ok(),
        "the thread that waited for the context panicked"
    );

    // The device is lost from then on, so wgpu-core reports it and the engine makes another.
    assert!(matches!(
        unsafe { device.get_fence_value(&fence) },
        Err(hal::DeviceError::Lost)
    ));
}
