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

fn sleep_now() -> Instant {
    #[cfg(test)]
    {
        return test_support::now();
    }

    #[cfg(not(test))]
    {
        Instant::now()
    }
}

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
            when: sleep_now() + duration,
            registered: false,
        }
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<()> {
        let this = self.get_mut();
        if sleep_now() > this.when {
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

#[cfg(test)]
mod test_support {
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    #[derive(Default)]
    struct ClockOverride {
        now: Option<Instant>,
    }

    static TEST_CLOCK: OnceLock<Mutex<ClockOverride>> = OnceLock::new();

    /// Returns the current time for `Sleep`.
    ///
    /// Tests can freeze this value so they can assert exact first-poll behavior
    /// without depending on nanosecond-level timing between `Sleep::new()` and
    /// `Sleep::poll()`. Production code always falls back to `Instant::now()`.
    pub(super) fn now() -> Instant {
        TEST_CLOCK
            .get_or_init(|| Mutex::new(ClockOverride::default()))
            .lock()
            .unwrap()
            .now
            .unwrap_or_else(Instant::now)
    }

    /// Overrides the clock observed by `Sleep` in tests.
    ///
    /// This exists so duration-edge-case tests, especially `Duration::ZERO`,
    /// can check intended semantics deterministically instead of racing the
    /// system clock.
    pub(super) fn set_now(now: Instant) {
        TEST_CLOCK
            .get_or_init(|| Mutex::new(ClockOverride::default()))
            .lock()
            .unwrap()
            .now = Some(now);
    }

    pub(super) fn reset_now() {
        TEST_CLOCK
            .get_or_init(|| Mutex::new(ClockOverride::default()))
            .lock()
            .unwrap()
            .now = None;
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use super::test_support;

    use std::{
        collections::BinaryHeap,
        sync::{
            Arc, Mutex, MutexGuard, Once, OnceLock,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Context, Poll, Wake, Waker},
    };

    static TIMER_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    static SHARED_TIMER_DRIVER: Once = Once::new();

    #[derive(Default)]
    struct CountingWake {
        wakes: AtomicUsize,
    }

    impl Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.wakes.fetch_add(1, Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.wakes.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn timer_test_guard() -> MutexGuard<'static, ()> {
        TIMER_TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap()
    }

    fn shared_timer() -> Arc<Timer> {
        TIMER.get_or_init(|| Arc::new(Timer::new())).clone()
    }

    fn reset_shared_timer(timer: &Timer) {
        timer.heap.lock().unwrap().clear();
    }

    fn ensure_shared_timer_driver() {
        let timer = shared_timer();
        SHARED_TIMER_DRIVER.call_once(|| start_timer_driver(timer));
    }

    fn counting_waker() -> (Waker, Arc<CountingWake>) {
        let counter = Arc::new(CountingWake::default());
        (Waker::from(counter.clone()), counter)
    }

    fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            if predicate() {
                return;
            }

            thread::sleep(Duration::from_millis(1));
        }
    }

    fn set_sleep_clock(now: Instant) {
        test_support::set_now(now);
    }

    fn reset_sleep_clock() {
        test_support::reset_now();
    }

    #[test]
    fn test_timer_entry_min_heap_ordering() {
        let (earlier_waker, _) = counting_waker();
        let (later_waker, _) = counting_waker();

        let earlier = TimerEntry {
            when: Instant::now(),
            waker: earlier_waker,
        };

        let later = TimerEntry {
            when: Instant::now() + Duration::from_secs(10),
            waker: later_waker,
        };

        assert!(
            earlier > later,
            "earlier deadline should have higher heap priority"
        );

        let mut heap = BinaryHeap::new();
        heap.push(later);
        heap.push(earlier);
        let top = heap.peek().unwrap();

        assert!(
            top.when <= Instant::now() + Duration::from_millis(100),
            "min-heap should surface the earliest deadline first"
        );
    }

    #[test]
    fn test_timer_register_adds_entry_to_heap() {
        let timer = Timer::new();
        let (waker, _) = counting_waker();
        let when = Instant::now() + Duration::from_secs(5);

        timer.register(when, waker);

        let heap = timer.heap.lock().unwrap();

        assert_eq!(heap.len(), 1);
        assert_eq!(heap.peek().unwrap().when, when);
    }

    #[test]
    fn test_timer_driver_wakes_expired_entries() {
        let timer = Arc::new(Timer::new());
        let (waker, counter) = counting_waker();
        timer.register(Instant::now() - Duration::from_millis(1), waker);
        start_timer_driver(timer.clone());
        wait_until(Duration::from_millis(50), || {
            counter.wakes.load(Ordering::SeqCst) == 1
        });

        assert_eq!(
            counter.wakes.load(Ordering::SeqCst),
            1,
            "expired entry should have been woken"
        );
    }

    #[test]
    fn test_timer_driver_does_not_wake_future_entries() {
        let timer = Arc::new(Timer::new());
        let (waker, counter) = counting_waker();
        timer.register(Instant::now() + Duration::from_millis(50), waker);
        start_timer_driver(timer.clone());
        thread::sleep(Duration::from_millis(10));

        assert_eq!(
            counter.wakes.load(Ordering::SeqCst),
            0,
            "future entry should not be woken before its deadline"
        );
    }

    #[test]
    fn test_timer_driver_wakes_multiple_expired_entries_same_tick() {
        let timer = Arc::new(Timer::new());
        let (waker, counter) = counting_waker();

        timer.register(Instant::now() - Duration::from_millis(2), waker.clone());
        timer.register(Instant::now() - Duration::from_millis(1), waker);
        start_timer_driver(timer.clone());

        wait_until(Duration::from_millis(50), || {
            counter.wakes.load(Ordering::SeqCst) == 2
        });

        assert_eq!(
            counter.wakes.load(Ordering::SeqCst),
            2,
            "all expired entries should be woken in the same driver pass"
        );
    }

    #[test]
    fn test_sleep_returns_ready_if_deadline_already_passed() {
        let _guard = timer_test_guard();
        reset_sleep_clock();
        let (waker, _) = counting_waker();
        let mut sleep = Sleep {
            when: Instant::now() - Duration::from_millis(1),
            registered: false,
        };
        let mut cx = Context::from_waker(&waker);

        assert!(matches!(Pin::new(&mut sleep).poll(&mut cx), Poll::Ready(())));
    }

    #[test]
    fn test_sleep_returns_pending_on_first_poll() {
        let _guard = timer_test_guard();
        reset_sleep_clock();
        let timer = shared_timer();
        reset_shared_timer(&timer);

        let (waker, _) = counting_waker();
        let mut sleep = Sleep::new(Duration::from_secs(1));
        let mut cx = Context::from_waker(&waker);

        assert!(matches!(
            Pin::new(&mut sleep).poll(&mut cx),
            Poll::Pending
        ));
    }

    #[test]
    fn test_sleep_registers_waker_only_once() {
        let _guard = timer_test_guard();
        reset_sleep_clock();
        let timer = shared_timer();
        reset_shared_timer(&timer);

        let (waker, _) = counting_waker();
        let mut sleep = Sleep::new(Duration::from_secs(60));
        let mut cx = Context::from_waker(&waker);

        assert!(matches!(
            Pin::new(&mut sleep).poll(&mut cx),
            Poll::Pending
        ));
        assert_eq!(timer.heap.lock().unwrap().len(), 1);

        assert!(matches!(
            Pin::new(&mut sleep).poll(&mut cx),
            Poll::Pending
        ));
        assert_eq!(
            timer.heap.lock().unwrap().len(),
            1,
            "sleep should only register its waker once"
        );
    }

    #[test]
    fn test_sleep_resolves_after_duration() {
        let _guard = timer_test_guard();
        reset_sleep_clock();
        let timer = shared_timer();
        reset_shared_timer(&timer);
        ensure_shared_timer_driver();

        let (waker, counter) = counting_waker();
        let mut sleep = Sleep::new(Duration::from_millis(10));
        let mut cx = Context::from_waker(&waker);

        assert!(matches!(
            Pin::new(&mut sleep).poll(&mut cx),
            Poll::Pending
        ));

        wait_until(Duration::from_millis(100), || {
            counter.wakes.load(Ordering::SeqCst) > 0
        });

        assert!(
            counter.wakes.load(Ordering::SeqCst) > 0,
            "sleep should arrange to wake its waker after the deadline"
        );
        assert!(matches!(Pin::new(&mut sleep).poll(&mut cx), Poll::Ready(())));
    }

    #[test]
    fn test_sleep_zero_duration_resolves_immediately() {
        let _guard = timer_test_guard();
        let now = Instant::now();
        set_sleep_clock(now);
        let (waker, _) = counting_waker();
        let mut sleep = Sleep::new(Duration::ZERO);
        set_sleep_clock(now + Duration::from_nanos(1));
        let mut cx = Context::from_waker(&waker);

        assert!(matches!(Pin::new(&mut sleep).poll(&mut cx), Poll::Ready(())));
        reset_sleep_clock();
    }
}
