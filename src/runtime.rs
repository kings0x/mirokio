use std::sync::{Arc, OnceLock};

use std::thread::{self, JoinHandle};

use crossbeam::deque::{Injector, Stealer, Worker};

use opentelemetry::global;

use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::executor::task::*;
use crate::executor::workers::*;
use crate::timer::*;

// This is shared across the whole runtime process.
// Spawn calls from outside worker threads should all target the same injector,
// no matter which OS thread initiated the spawn.
pub static GLOBAL_INJECTOR: OnceLock<Arc<Injector<Arc<Task>>>> = OnceLock::new();

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

        let global = GLOBAL_INJECTOR
            .get_or_init(|| Arc::new(Injector::new()))
            .clone();

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
