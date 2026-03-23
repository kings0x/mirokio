use std::sync::Arc;
use std::{iter, usize};

use std::cell::Cell;
use std::thread::{self, JoinHandle};

use crossbeam::deque::{Injector, Stealer, Worker};
use crossbeam::sync::Unparker;
use opentelemetry::global;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

thread_local! {}

fn main() {
    println!("Hello, world!");
}

thread_local! {
    static CUREENT_RUNTIME:Cell<Option< *mut Worker<Arc<Task>>>> = Cell::new(None);
    static CURRENT_WORKER:Cell<Option<*mut Worker<Arc<Task>>>> =Cell::new(None);
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

        let global = Arc::new(Injector::new());
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

        Self {
            workers: os_threads,
        }
    }
}

pub fn worker_loop(
    global: Arc<Injector<Arc<Task>>>,
    local: Worker<Arc<Task>>,
    stealers: Arc<Vec<Stealer<Arc<Task>>>>,
) {
    let boxed = Box::new(local);
    let worker: *mut Worker<Arc<Task>> = Box::into_raw(boxed);
    CUREENT_RUNTIME.with(|val| {
        val.set(Some(worker));
    });

    CURRENT_WORKER.with(|val| {
        val.set(Some(worker));
    });

    loop {
        if let Some(task) = unsafe { &mut *worker }.pop().or_else(|| {
            iter::repeat_with(|| {
                global
                    .steal_batch_and_pop(unsafe { &*worker })
                    .or_else(|| stealers.iter().map(|s| s.steal()).collect())
            })
            .find(|s| !s.is_retry())
            .and_then(|s| s.success())
        }) {
            task.poll();
        }
    }
}

pub fn spawn<F>(future: F) {
    let global = CUREENT_RUNTIME.with(|val| val.get());

    let local = CURRENT_WORKER.with(|val| val.get());

    if let Some(queue) = local {
        Task::spawn(future, queue);
    } else {
        if let Some(queue) = global {
            Task::spawn(future, queue);
        }
    }
}
pub struct Task {
    task_future: TaskFuture,
}

impl Task {
    pub fn schedule(self: Arc<Self>, sender: *mut Worker<Arc<Task>>) {
        unsafe {
            (*sender).push(self.clone());
        }
    }

    pub fn spawn<F>(future: F, sender: *mut Worker<Arc<Task>>) {
        let task_future = TaskFuture {};

        let task = Arc::new(Task { task_future });

        task.schedule(sender);
    }

    fn poll(&self) {}
}

pub struct TaskFuture {}
