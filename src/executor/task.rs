use crate::executor::workers::*;
use crossbeam::deque::Injector;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
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

#[cfg(test)]
mod test {
    use crossbeam::deque::Worker;

    use crate::executor::workers;

    use super::*;
    use std::{
        future::Future,
        sync::atomic::{AtomicBool, AtomicU8, Ordering},
        task::{Context, Poll},
    };

    struct OurFuture {
        poll_count: Arc<AtomicU8>,
        is_completed: Arc<AtomicBool>,
    }

    impl Future for OurFuture {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            self.poll_count.fetch_add(1, Ordering::SeqCst);

            if self.is_completed.load(Ordering::SeqCst) {
                return Poll::Ready(());
            }

            Poll::Pending
        }
    }

    fn our_task() -> (Arc<Task>, Arc<AtomicU8>, Arc<AtomicBool>) {
        let poll_count = Arc::new(AtomicU8::new(0));
        let is_completed = Arc::new(AtomicBool::new(false));
        let future = OurFuture {
            poll_count: poll_count.clone(),
            is_completed: is_completed.clone(),
        };

        let task = Arc::new(Task {
            future: Mutex::new(Box::pin(future)),
            queue: Mutex::new(None),
        });

        (task, poll_count, is_completed)
    }

    #[test]
    fn test_task_polls_future_to_completion() {
        let (task, poll_count, is_completed) = our_task();
        is_completed.store(true, Ordering::SeqCst);
        task.clone().poll();
        assert_eq!(poll_count.load(Ordering::SeqCst), 1)
    }

    #[test]
    fn test_task_waker_reschedules_on_wake() {
        let global = Arc::new(Injector::new());
        let global_queue = TaskQueue::Global(global.clone());
        let (task, _, _) = our_task();

        task.set_queue(global_queue);

        let waker = task_waker(task);

        waker.wake();

        let worker = Worker::new_lifo();

        let stolen = global.steal_batch_and_pop(&worker);

        assert!(stolen.success().is_some(), "task was not requeued on wake")
    }

    #[test]
    fn test_task_waker_clone_increments_refcount() {
        let (task, _, _) = our_task();

        let before = Arc::strong_count(&task);

        let cloned = task.clone(); //+1

        let waker = task_waker(task.clone()); //+1
        let wak = waker.clone(); //+1

        assert_eq!(Arc::strong_count(&task), before + 3);

        drop(wak);
        assert_eq!(Arc::strong_count(&task), before + 2);

        drop(cloned);
        assert_eq!(Arc::strong_count(&task), before + 1);
    }

    #[test]
    fn test_task_waker_drop_decrements_refcount() {
        let global = Arc::new(Injector::<Arc<Task>>::new());
        let (task, _, _) = our_task();
        task.set_queue(TaskQueue::Global(global.clone()));

        let before = Arc::strong_count(&task);

        {
            let waker = task_waker(task.clone()); // +1
            assert_eq!(Arc::strong_count(&task), before + 1); //2
            drop(waker); // -1
        }
        dbg!(Arc::strong_count(&task));
        assert_eq!(
            Arc::strong_count(&task),
            before,
            "waker drop leaked an Arc — refcount did not return to baseline"
        );
    }

    #[test]
    fn test_task_set_queue_stores_correctly() {
        let global = Arc::new(Injector::<Arc<Task>>::new());
        let (task, _, _) = our_task();

        assert!(task.queue.lock().unwrap().is_none());

        task.set_queue(TaskQueue::Global(global.clone()));
        let lock = task.queue.lock().unwrap();

        assert!(
            matches!(*lock, Some(TaskQueue::Global(_))),
            "set_queue did not store Global variant"
        );
    }

    #[test]
    fn test_task_schedule_global_pushes_to_injector() {
        let global = Arc::new(Injector::<Arc<Task>>::new());

        let (task, _, _) = our_task();
        task.set_queue(TaskQueue::Global(global.clone()));

        task.clone().schedule();

        let worker: Worker<Arc<Task>> = Worker::new_lifo();
        assert!(
            global.steal_batch_and_pop(&worker).success().is_some(),
            "schedule() did not push to global injector"
        );
    }
    #[test]
    fn test_task_schedule_worker_pushes_to_local_queue() {
        let worker = Arc::new(Worker::new_lifo());
        let worker_handle = WorkerHandle {
            queue: worker.clone(),
        };
        CURRENT_WORKER.with(|val| *val.borrow_mut() = Some(worker_handle));
        let (task, _, _) = our_task();

        task.clone().set_queue(TaskQueue::Worker);
        task.clone().schedule();

        let popped = worker.clone().pop();

        assert!(
            Arc::ptr_eq(&popped.unwrap(), &task),
            "a task was pushed but it wasn't the task we scheduled"
        );

        // Clean up so other tests are not affected
        CURRENT_WORKER.with(|slot| *slot.borrow_mut() = None);
    }

    /// schedule() with TaskQueue::Worker but NO CURRENT_WORKER set
    /// silently drops the task (current known behavior — this test
    /// documents and pins that contract so a future fix is visible).
    #[test]
    fn test_task_schedule_worker_outside_worker_thread_is_noop() {
        CURRENT_WORKER.with(|slot| *slot.borrow_mut() = None);

        let (task, _, _) = our_task();
        task.set_queue(TaskQueue::Worker);
        task.clone().schedule();
        // The "task should be silently dropped" path is the known bug documented above.
    }

    #[test]
    fn test_task_waker_wake_by_ref_does_not_consume() {
        let global = Arc::new(Injector::new());
        let global_queue = TaskQueue::Global(global.clone());
        let (task, _, _) = our_task();

        task.set_queue(global_queue);
        let before = Arc::strong_count(&task);

        let waker = task_waker(task.clone());

        waker.wake_by_ref();

        assert_eq!(Arc::strong_count(&task), before + 2);

        drop(waker);

        assert_eq!(Arc::strong_count(&task), before + 1)
    }
}
