use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    hint::spin_loop,
    sync::{
        atomic::{AtomicU32, AtomicUsize, Ordering},
        Arc,
    },
};

use atomic_wait::{wait, wake_one};

#[derive(Debug)]
struct Channel<T> {
    // Location in memory where
    slots: UnsafeCell<VecDeque<T>>,
    length: AtomicU32,
    sender_count: AtomicUsize,
    cap: usize,
}

unsafe impl<T> Sync for Channel<T> where T: Send {}

impl<T> Channel<T> {
    fn new(cap: usize) -> Self {
        let slots = VecDeque::with_capacity(cap);
        if cap == 0 {
            panic!("Zero sized queues aren't allowed")
        }

        if cap > u32::MAX as usize {
            panic!("Queues are limited to {} elements only.", u32::MAX)
        }

        Self {
            slots: UnsafeCell::new(slots),
            length: AtomicU32::new(0),
            sender_count: AtomicUsize::new(0),
            cap,
        }
    }

    fn len(&self) -> u32 {
        self.length.load(Ordering::Acquire)
    }

    // Send an item through the channel. Blocking.
    // Safety: We are writing to a queue, ensure exclusivity
    unsafe fn send(&self, item: T) {
        (*self.slots.get()).push_back(item);
    }

    // Recv an item from the void. blocking. Returns None if no senders left.
    // Safety: We are reading from the queue, ensure exclusivity
    unsafe fn recv(&self) -> Option<T> {
        (*self.slots.get()).pop_front()
    }
}

pub struct Sender<T> {
    channel: Arc<Channel<T>>,
}

impl<T> Sender<T> {
    pub fn send(&self, item: T) {
        let mut len = self.channel.len();
        let cap = self.channel.cap as u32;

        // Insert element to queue only after checking for vacancy
        loop {
            if cap > len + 1 {
                // wait for an update on the length
                wait(&self.channel.length, cap);
                len = self.channel.len();
                continue;
            }

            // Ensure the update was atomic
            if let Err(e) = self.channel.length.compare_exchange(
                len,
                len + 1,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                len = e;
                continue;
            }

            // Safety: since the update succeeded, we assume this is a safe operation
            return unsafe { self.channel.send(item) };
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        let mut sender_count = self.channel.sender_count.load(Ordering::Acquire);
        loop {
            if let Err(e) = self.channel.sender_count.compare_exchange(
                sender_count,
                sender_count + 1,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                spin_loop();
                sender_count = e;
                continue;
            }

            return Self {
                channel: self.channel.clone(),
            };
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut sender_count = self.channel.sender_count.load(Ordering::Acquire);

        loop {
            let Some(next_count) = sender_count.checked_sub(1) else {
                return;
            };

            if let Err(e) = self.channel.sender_count.compare_exchange(
                sender_count,
                next_count,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                spin_loop();
                sender_count = e;
                continue;
            }

            // Ensure that any thread waiting on recv is forced to reconsider
            wake_one(&self.channel.length);
            // If no Arc left, i.e. neither sender no receiver, the waiting data in queue will also be lost
            return;
        }
    }
}

pub struct Receiver<T> {
    channel: Arc<Channel<T>>,
}

impl<T> Receiver<T> {
    pub fn recv(&self) -> Option<T> {
        let mut len = self.channel.len();

        // Remove element from the queue only after checking for vacancy
        loop {
            if len == 0 {
                if self.channel.sender_count.load(Ordering::Acquire) == 0 {
                    return None;
                }
                // wait for an update on the length
                wait(&self.channel.length, len);
                len = self.channel.len();
                continue;
            }

            // Ensure we correctly account for the element to be popped
            if let Err(e) = self.channel.length.compare_exchange(
                len,
                len - 1,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                spin_loop();
                len = e;
                continue;
            }

            // Safety: since the update succeeded, we assume this is a safe operation
            return unsafe { self.channel.recv() };
        }
    }
}

pub fn channel<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    let channel = Channel::new(cap);
    let arc = Arc::new(channel);

    // Sender count will be initially 0, so unwrap is fine
    arc.sender_count
        .compare_exchange(0, 1, Ordering::Release, Ordering::Relaxed)
        .unwrap();

    (
        Sender {
            channel: arc.clone(),
        },
        Receiver { channel: arc },
    )
}

#[cfg(test)]
mod test {
    use crate::channel;

    #[test]
    fn use_as_normal() {
        let (tx, rx) = channel(1);

        std::thread::spawn(move || tx.send(1));

        assert_eq!(rx.recv(), Some(1));
        assert_eq!(rx.recv(), None);
    }

    #[test]
    fn multiple_senders() {
        let (tx, rx) = channel(1);
        let tx1 = tx.clone();
        let tx2 = tx;
        std::thread::spawn(move || tx1.send(1));
        std::thread::spawn(move || tx2.send(2));

        let check = |x| x == Some(1) || x == Some(2);
        assert!(check(rx.recv()));
        assert!(check(rx.recv()));
        assert_eq!(rx.recv(), None);
    }
}
