use std::sync::{Arc, Mutex};

use std::future::Future;
use std::pin::Pin;

use crate::executor::workers::*;

use crossbeam::deque::Injector;
use std::task::{self, RawWaker, RawWakerVTable, Waker};

pub enum TaskQueue {
    Worker,
    Global(Arc<Injector<Arc<Task>>>),
}
pub struct Task {
    pub future: Mutex<Pin<Box<dyn Future<Output = ()> + Send>>>,
    pub queue: Mutex<Option<TaskQueue>>,
}

impl Task {
    pub fn schedule(self: Arc<Self>) {
        if let Some(ref queue) = *self.queue.lock().unwrap() {
            match queue {
                TaskQueue::Global(global) => global.push(self.clone()),

                TaskQueue::Worker => {
                    if let Some(queue) = CURRENT_WORKER.with(|slot| slot.borrow().clone()) {
                        queue.queue.push(self.clone());
                    }
                }
            }
        }
    }

    pub fn poll(self: Arc<Self>) {
        let waker = task_waker(self.clone());
        let mut cx = task::Context::from_waker(&waker);
        let mut future = self.future.lock().unwrap();
        if let task::Poll::Pending = future.as_mut().poll(&mut cx) {
            //The future would wake it self later
        }
    }

    pub fn set_queue(&self, queue: TaskQueue) {
        let mut lock = self.queue.lock().unwrap();
        *lock = Some(queue)
    }
}

//This is just a helper to hold the task
pub struct TaskWaker {
    task: Arc<Task>,
}

impl TaskWaker {
    fn wake_task(task: Arc<Task>) {
        task.schedule();
    }
}

//This creates the rawwaker vtable funtionality and creates the rawtable with them, to be used to build the waker
fn raw_waker(task: Arc<Task>) -> RawWaker {
    unsafe fn clone(data: *const ()) -> RawWaker {
        let arc = unsafe { Arc::<Task>::from_raw(data as *const Task) };
        let cloned = arc.clone();
        std::mem::forget(arc);
        raw_waker(cloned)
    }

    unsafe fn wake(data: *const ()) {
        let arc = unsafe { Arc::<Task>::from_raw(data as *const Task) };
        TaskWaker::wake_task(arc.clone());
    }

    unsafe fn wake_by_ref(data: *const ()) {
        let arc = unsafe { Arc::<Task>::from_raw(data as *const Task) };
        let cloned = arc.clone();
        TaskWaker::wake_task(cloned);
        std::mem::forget(arc);
    }

    unsafe fn drop(data: *const ()) {
        unsafe {
            core::mem::drop(Arc::<Task>::from_raw(data as *const Task));
        }
    }

    RawWaker::new(
        Arc::into_raw(task) as *const (),
        &RawWakerVTable::new(clone, wake, wake_by_ref, drop),
    )
}

//task_waker creates the waker that would now be passed to context and be sent out
fn task_waker(task: Arc<Task>) -> Waker {
    unsafe { Waker::from_raw(raw_waker(task)) }
}
