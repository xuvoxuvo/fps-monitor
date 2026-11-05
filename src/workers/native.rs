//! CPU/GPU usage and GPU temperature from Windows itself.
//!
//! No helper process, no .NET assemblies and no kernel driver: CPU load comes
//! from `GetSystemTimes`, GPU utilisation from the `GPU Engine` PDH counters and
//! GPU temperature from the graphics kernel (`D3DKMTQueryAdapterInfo`).

use std::time::Duration;

use crate::app::shared::SharedHandle;
use crate::platform::windows::{sleep_interruptible, CpuLoad};
use crate::sensors::win_gpu::WinGpu;
use crate::workers::WorkerHandle;

pub fn start(shared: &SharedHandle) -> WorkerHandle {
    let shared = shared.clone();
    let join = std::thread::spawn(move || run(shared));
    WorkerHandle::new(join, Box::new(|| {}))
}

fn run(shared: SharedHandle) {
    let mut cpu_load = CpuLoad::default();
    let mut gpu = WinGpu::new();
    let status = if gpu.name.is_some() {
        "Windows APIs (GPU native)"
    } else {
        "Windows APIs (no GPU found)"
    };
    shared.update_stats(|stats| stats.hw_status = status.to_string());

    while shared.is_running() {
        let cpu = cpu_load.sample();
        let gpu_load = gpu.load();
        let gpu_temp = gpu.temperature();
        shared.update_stats(|stats| {
            stats.cpu = cpu;
            stats.gpu = gpu_load;
            stats.gpu_temp = gpu_temp;
        });
        sleep_interruptible(&shared, Duration::from_secs(1));
    }
}
