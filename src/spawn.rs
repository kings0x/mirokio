use std::sync::{Arc, Mutex};

use std::future::Future;
use std::pin::Pin;

use crate::executor::task::*;
use crate::executor::workers::*;
use crate::runtime::*;

pub fn spawn<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let future = Mutex::new(Box::pin(future) as Pin<Box<dyn Future<Output = ()> + Send>>);

    let task = Arc::new(Task {
        future,
        queue: Mutex::new(None),
    });

    let queue = if let Some(handle) = CURRENT_WORKER.with(|slot| slot.borrow().clone()) {
        handle.queue.push(task.clone());
        TaskQueue::Worker
    } else {
        GLOBAL_INJECTOR.with(|global| {
            global.push(task.clone());
            TaskQueue::Global(global.clone())
        })
    };

    task.set_queue(queue);
}
