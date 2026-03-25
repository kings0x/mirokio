use std::sync::{Arc, Mutex};
use std::time::Instant;

use std::thread;

use std::collections::BinaryHeap;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{self, Waker};

//===========
//Async sleep Timer System
//===========

//we need to know when to wake so we store the time to
//wake and the waker when, waker
//then a seperate thread would watch the time, so when its its time is reached it calls waker.wake()

use std::cmp::Ordering;
use std::time::Duration;

pub static TIMER: OnceLock<Arc<Timer>> = OnceLock::new();

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
pub struct Timer {
    heap: Mutex<BinaryHeap<TimerEntry>>,
}

impl Timer {
    pub fn new() -> Self {
        Self {
            heap: Mutex::new(BinaryHeap::new()),
        }
    }

    fn register(&self, when: Instant, waker: Waker) {
        let mut heap = self.heap.lock().unwrap();
        heap.push(TimerEntry { when, waker });
    }
}

pub fn start_timer_driver(timer: Arc<Timer>) {
    thread::spawn(move || {
        loop {
            let now = Instant::now();

            let mut heap = timer.heap.lock().unwrap();

            while let Some(entry) = heap.peek() {
                if entry.when <= now {
                    let entry = heap.pop().unwrap();
                    entry.waker.wake();
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
