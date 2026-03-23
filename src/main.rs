use std::arch::global_asm;
use std::sync::{Arc, Mutex};
use std::{future, iter, usize};

use std::cell::RefCell;
use std::thread::{self, JoinHandle};

use crossbeam::deque::{Injector, Stealer, Worker};
use crossbeam::queue;
use crossbeam::sync::Unparker;
use opentelemetry::{Context, global};
use std::pin::Pin;
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

    fn poll(self: Arc<Self>) {}

    pub fn set_queue(&self, queue: TaskQueue) {
        let mut lock = self.queue.lock().unwrap();
        *lock = Some(queue)
    }
}
