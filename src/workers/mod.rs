//! Background workers: FPS capture and native sensor polling.
//!
//! Each worker is a plain thread plus a stop closure that kills any child
//! process it owns. `Workers::stop` signals shutdown, kills the children (so
//! pipes reach EOF and the threads wake up), then joins.

use std::thread::JoinHandle;

use crate::app::shared::SharedHandle;

pub mod fps;
pub mod native;

pub struct WorkerHandle {
    join: JoinHandle<()>,
    stop: Box<dyn Fn() + Send + Sync>,
}

impl WorkerHandle {
    pub fn new(join: JoinHandle<()>, stop: Box<dyn Fn() + Send + Sync>) -> Self {
        Self { join, stop }
    }
}

pub struct Workers {
    handles: Vec<WorkerHandle>,
}

impl Workers {
    pub fn start(shared: &SharedHandle) -> Self {
        let handles = vec![native::start(shared), fps::start(shared)];
        Self { handles }
    }

    pub fn stop(self, shared: &SharedHandle) {
        shared.stop();
        for handle in &self.handles {
            (handle.stop)();
        }
        for handle in self.handles {
            let _ = handle.join.join();
        }
    }
}
