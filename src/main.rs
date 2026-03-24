use std::sync::{Arc, Mutex};
use std::time::Instant;

use std::cell::RefCell;
use std::thread::{self, JoinHandle};

use crossbeam::deque::{Injector, Stealer, Worker};

use crossbeam::sync::Unparker;
use opentelemetry::{Context, global};
use std::collections::BinaryHeap;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{self, RawWaker, RawWakerVTable, Wake, Waker};
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

//This is used to determine wether the task should be sent to either the global queue if its on the main thread or the worker thread if its on the worker thread
thread_local! {
    static CURRENT_WORKER: RefCell<Option<WorkerHandle>> = RefCell::new(None);
}

thread_local! {
    static GLOBAL_INJECTOR: Arc<Injector<Arc<Task>>> = Arc::new(Injector::new());
}

static TIMER: OnceLock<Arc<Timer>> = OnceLock::new();

#[derive(Clone)]
pub struct WorkerHandle {
    queue: Arc<Worker<Arc<Task>>>,
}

fn main() {
    println!("Hello, world!");
}

pub struct mirokio {
    workers: Vec<JoinHandle<()>>,
}

impl mirokio {
    pub fn new(cap: usize) -> Self {
        global::set_text_map_propagator(opentelemetry_jaeger::Propagator::new());
        let tracer = opentelemetry_jaeger::new_pipeline()
            .with_service_name("mirokio")
            .install_simple()
            .unwrap();

        let opentelemetry = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing_subscriber::registry()
            .with(opentelemetry)
            .with(fmt::Layer::default())
            .try_init()
            .unwrap();

        let global = GLOBAL_INJECTOR.with(|val| val.clone());

        let mut os_threads: Vec<JoinHandle<()>> = Vec::with_capacity(cap);

        //this is the stealer queue
        let mut stealers = Vec::<Stealer<Arc<Task>>>::new();
        let mut workers = Vec::new();

        for _ in 0..cap {
            //our very own worker queue
            let worker: Worker<Arc<Task>> = Worker::new_lifo();
            stealers.push(worker.stealer());

            workers.push(worker);
        }

        let stealers = Arc::new(stealers);

        for worker in workers.into_iter() {
            let stealers = stealers.clone();
            let global = global.clone();

            let os_thread = thread::spawn(move || {
                worker_loop(global, worker, stealers);
            });

            os_threads.push(os_thread);
        }

        let timer = Arc::new(Timer::new());

        TIMER.set(timer.clone()).unwrap();

        start_timer_driver(timer.clone());

        Self {
            workers: os_threads,
        }
    }
}

fn find_task(
    local: &Worker<Arc<Task>>,
    global: &Injector<Arc<Task>>,
    stealers: &[Stealer<Arc<Task>>],
) -> Option<Arc<Task>> {
    local
        .pop()
        .or_else(|| global.steal_batch_and_pop(local).success())
        .or_else(|| {
            stealers
                .iter()
                .map(|s| s.steal())
                .find(|s| s.is_success())
                .and_then(|s| s.success())
        })
}

pub fn worker_loop(
    global: Arc<Injector<Arc<Task>>>,
    local: Worker<Arc<Task>>,
    stealers: Arc<Vec<Stealer<Arc<Task>>>>,
) {
    let handle = WorkerHandle {
        queue: Arc::new(local),
    };

    CURRENT_WORKER.with(|slot| *slot.borrow_mut() = Some(handle.clone()));

    loop {
        if let Some(task) = find_task(&handle.queue, &global, &stealers) {
            task.poll();
        } else {
            std::thread::yield_now();
        }
    }
}

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

enum TaskQueue {
    Worker,
    Global(Arc<Injector<Arc<Task>>>),
}
pub struct Task {
    future: Mutex<Pin<Box<dyn Future<Output = ()> + Send>>>,
    queue: Mutex<Option<TaskQueue>>,
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

    fn poll(self: Arc<Self>) {
        let waker = task_waker(self.clone());
        let mut cx = task::Context::from_waker(&waker);
        let mut future = self.future.lock().unwrap();
        if let task::Poll::Pending = future.as_mut().poll(&mut cx) {
            //The future would wake it self later
        }
    }

    fn set_queue(&self, queue: TaskQueue) {
        let mut lock = self.queue.lock().unwrap();
        *lock = Some(queue)
    }
}

//This is just a helper to hold the task
struct TaskWaker {
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
        TaskWaker::wake_task(arc);
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

//===========
//Async sleep Timer System
//===========

//we need to know when to wake so we store the time to
//wake and the waker when, waker
//then a seperate thread would watch the time, so when its its time is reached it calls waker.wake()

use std::cmp::Ordering;
use std::time::Duration;

#[derive(Debug)]
struct TimerEntry {
    when: Instant,
    waker: Waker,
}

impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        //reverse for min-heap
        other.when.cmp(&self.when)
    }
}

impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.when == other.when
    }
}

impl Eq for TimerEntry {}

#[derive(Debug)]
struct Timer {
    heap: Mutex<BinaryHeap<TimerEntry>>,
}

impl Timer {
    fn new() -> Self {
        Self {
            heap: Mutex::new(BinaryHeap::new()),
        }
    }

    fn register(&self, when: Instant, waker: Waker) {
        let mut heap = self.heap.lock().unwrap();
        heap.push(TimerEntry { when, waker });
    }
}

fn start_timer_driver(timer: Arc<Timer>) {
    thread::spawn(move || {
        loop {
            let now = Instant::now();

            let mut heap = timer.heap.lock().unwrap();

            while let Some(entry) = heap.peek() {
                if entry.when <= now {
                    let entry = heap.pop().unwrap();
                    entry.waker.wake(); //this is the magic
                } else {
                    break;
                }
            }

            drop(heap);

            thread::sleep(std::time::Duration::from_millis(1));
        }
    });
}

pub struct Sleep {
    when: Instant,
    registered: bool,
}

impl Sleep {
    pub fn new(duration: Duration) -> Self {
        Self {
            when: Instant::now() + duration,
            registered: false,
        }
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<()> {
        let this = self.get_mut();
        if Instant::now() > this.when {
            return task::Poll::Ready(());
        }

        if !this.registered {
            let waker = cx.waker().clone();

            if let Some(timer) = TIMER.get() {
                timer.register(this.when, waker);
            }

            this.registered = true;
        }

        task::Poll::Pending
    }
}

pub fn sleep(duration: Duration) -> Sleep {
    Sleep::new(duration)
}
