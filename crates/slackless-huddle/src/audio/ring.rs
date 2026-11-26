//! Zero-dependency lock-free single-producer single-consumer (SPSC) ring buffer for real-time
//! audio callbacks (`CoreAudio` / `cpal` -> worker threads and decoder thread -> render callbacks).
//!
//! Producer methods (`push_slice`) touch only `tail` (with `Release` store after writing) and load
//! `head` (`Acquire`). Consumer methods (`pop_slice`, `skip`) touch only `head` (with `Release`
//! store after reading) and load `tail` (`Acquire`). Neither side ever blocks or allocates.

use std::cell::UnsafeCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

/// Lock-free wake-up from a real-time audio callback to the one worker thread serving its ring.
#[derive(Default)]
pub(crate) struct WorkerWake {
    worker: OnceLock<std::thread::Thread>,
    parked: AtomicBool,
}

impl WorkerWake {
    /// Called once from the worker thread so real-time callbacks can unpark it.
    pub(crate) fn register(&self) {
        let _ = self.worker.set(std::thread::current());
    }

    /// Real-time side: wake the worker only if it is currently parked.
    pub(crate) fn wake_if_parked(&self) {
        if self.parked.swap(false, Ordering::AcqRel)
            && let Some(t) = self.worker.get()
        {
            t.unpark();
        }
    }

    /// Unconditional wake used on shutdown/hot-plug so the worker reacts immediately.
    pub(crate) fn wake_now(&self) {
        self.parked.store(false, Ordering::Release);
        if let Some(t) = self.worker.get() {
            t.unpark();
        }
    }

    /// Worker side: park for up to `timeout` while `idle()` holds. `idle` is re-checked after
    /// arming the flag, so a wake between the first check and the park is never lost.
    pub(crate) fn park_while(&self, timeout: Duration, idle: impl Fn() -> bool) {
        if idle() {
            self.parked.store(true, Ordering::Release);
            if idle() {
                std::thread::park_timeout(timeout);
            }
            self.parked.store(false, Ordering::Release);
        }
    }

    /// Worker side: register the calling thread and park it until `running` is cleared. The
    /// owner clears the flag, then calls [`Self::wake_now`], so the worker exits at once instead
    /// of on its next poll.
    pub(crate) fn park_until_cleared(&self, running: &AtomicBool) {
        self.register();
        while running.load(Ordering::Acquire) {
            self.park_while(Duration::from_secs(1), || running.load(Ordering::Acquire));
        }
    }
}

/// Fixed-capacity lock-free SPSC ring buffer of `Copy` samples.
///
/// # Concurrency invariant
/// At most one thread may call producer methods (`push_slice`) at a time, and at most one thread
/// may call consumer methods (`pop_slice`, `skip`, `clear_consumer`) at a time.
pub struct SpscRing<T: Copy> {
    buf: Box<[UnsafeCell<T>]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

// SAFETY: the ring owns its slots, so moving it moves `T: Send` values to another thread.
unsafe impl<T: Copy + Send> Send for SpscRing<T> {}
// SAFETY: Access to disjoint ring slots is synchronized via acquire-release atomic indices
// `head` (consumer-owned) and `tail` (producer-owned), and `T: Copy + Send`; the one-producer,
// one-consumer rule above keeps each index to a single writer.
unsafe impl<T: Copy + Send> Sync for SpscRing<T> {}

impl<T: Copy + Default> SpscRing<T> {
    /// Allocate a ring buffer with power-of-two capacity at least `min_capacity` (minimum 2).
    pub(crate) fn new(min_capacity: usize) -> Self {
        let cap = min_capacity.max(2).next_power_of_two();
        let buf: Vec<UnsafeCell<T>> = (0..cap).map(|_| UnsafeCell::new(T::default())).collect();
        Self {
            buf: buf.into_boxed_slice(),
            mask: cap - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    #[inline]
    pub(crate) fn capacity(&self) -> usize {
        self.mask + 1
    }

    /// Number of items currently readable in the ring.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        tail.wrapping_sub(head)
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Producer: write as many elements of `src` as fit without blocking; returns count written.
    pub(crate) fn push_slice(&self, src: &[T]) -> usize {
        if src.is_empty() {
            return 0;
        }
        let cap = self.capacity();
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        let used = tail.wrapping_sub(head);
        let free = cap.saturating_sub(used);
        let to_write = src.len().min(free);
        if to_write == 0 {
            return 0;
        }

        let start = tail & self.mask;
        let first = to_write.min(cap - start);
        let second = to_write - first;

        // SAFETY: Slots in `[tail, tail + to_write)` are exclusively owned by the producer until
        // `self.tail` is advanced with `Release` below.
        unsafe {
            let base = self.buf.as_ptr() as *mut T;
            std::ptr::copy_nonoverlapping(src.as_ptr(), base.add(start), first);
            if second > 0 {
                std::ptr::copy_nonoverlapping(src.as_ptr().add(first), base, second);
            }
        }
        self.tail
            .store(tail.wrapping_add(to_write), Ordering::Release);
        to_write
    }

    /// Consumer: read up to `dst.len()` elements into `dst`; returns count read.
    pub(crate) fn pop_slice(&self, dst: &mut [T]) -> usize {
        if dst.is_empty() {
            return 0;
        }
        let cap = self.capacity();
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        let avail = tail.wrapping_sub(head);
        let to_read = dst.len().min(avail);
        if to_read == 0 {
            return 0;
        }

        let start = head & self.mask;
        let first = to_read.min(cap - start);
        let second = to_read - first;

        // SAFETY: Slots in `[head, head + to_read)` were published by the producer before its
        // `Release` store to `self.tail` and remain immutable until `self.head` advances.
        unsafe {
            let base = self.buf.as_ptr() as *const T;
            std::ptr::copy_nonoverlapping(base.add(start), dst.as_mut_ptr(), first);
            if second > 0 {
                std::ptr::copy_nonoverlapping(base, dst.as_mut_ptr().add(first), second);
            }
        }
        self.head
            .store(head.wrapping_add(to_read), Ordering::Release);
        to_read
    }

    /// Consumer: discard up to `count` oldest elements without copying; returns count skipped.
    pub(crate) fn skip(&self, count: usize) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        let avail = tail.wrapping_sub(head);
        let n = count.min(avail);
        if n > 0 {
            self.head.store(head.wrapping_add(n), Ordering::Release);
        }
        n
    }

    /// Consumer: discard all currently queued elements.
    pub(crate) fn clear_consumer(&self) {
        let tail = self.tail.load(Ordering::Acquire);
        self.head.store(tail, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn spsc_ring_wraparound_skip_and_bounded_capacity() {
        let ring = SpscRing::<i16>::new(5); // rounds up to 8
        assert_eq!(ring.capacity(), 8);
        assert!(ring.is_empty());

        assert_eq!(ring.push_slice(&[1, 2, 3, 4, 5, 6]), 6);
        assert_eq!(ring.len(), 6);

        let mut out = [0i16; 4];
        assert_eq!(ring.pop_slice(&mut out), 4);
        assert_eq!(out, [1, 2, 3, 4]);
        assert_eq!(ring.len(), 2);

        // Push across wraparound boundary; caps at free space (6).
        assert_eq!(ring.push_slice(&[7, 8, 9, 10, 11, 12, 13, 14]), 6);
        assert_eq!(ring.len(), 8);

        assert_eq!(ring.skip(3), 3); // drops 5, 6, 7

        let mut rest = [0i16; 8];
        assert_eq!(ring.pop_slice(&mut rest), 5);
        assert_eq!(&rest[..5], &[8, 9, 10, 11, 12]);
        assert!(ring.is_empty());
    }

    #[test]
    fn spsc_ring_transfers_data_cleanly_across_threads() {
        let ring = Arc::new(SpscRing::<u32>::new(256));
        let prod = Arc::clone(&ring);
        let total = 20_000u32;

        let writer = std::thread::spawn(move || {
            let mut next = 0u32;
            let mut chunk = [0u32; 37];
            while next < total {
                let n = ((total - next) as usize).min(chunk.len());
                for (i, slot) in chunk[..n].iter_mut().enumerate() {
                    *slot = next + i as u32;
                }
                let wrote = prod.push_slice(&chunk[..n]);
                next += wrote as u32;
                if wrote == 0 {
                    std::thread::yield_now();
                }
            }
        });

        let mut expected = 0u32;
        let mut buf = [0u32; 29];
        while expected < total {
            let got = ring.pop_slice(&mut buf);
            for &v in &buf[..got] {
                assert_eq!(v, expected);
                expected += 1;
            }
            if got == 0 {
                std::thread::yield_now();
            }
        }
        writer.join().unwrap();
        assert!(ring.is_empty());
    }
}
