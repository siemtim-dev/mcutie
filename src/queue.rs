use core::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
};

use embassy_sync::blocking_mutex::{raw::RawMutex, Mutex};
use heapless::Deque;

struct LossyQueueData<T, const N: usize> {
    receiver_waker: Option<Waker>,
    queue: Deque<T, N>,
}

pub(crate) struct ReceiveFuture<'a, M: RawMutex, T, const N: usize> {
    pipe: &'a LossyQueue<M, T, N>,
}

impl<M: RawMutex, T, const N: usize> Future for ReceiveFuture<'_, M, T, N> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.pipe.inner.lock(|cell| {
            let mut inner = cell.borrow_mut();

            if let Some(waker) = inner.receiver_waker.take() {
                waker.wake();
            }

            if let Some(item) = inner.queue.pop_front() {
                Poll::Ready(item)
            } else {
                inner.receiver_waker = Some(cx.waker().clone());
                Poll::Pending
            }
        })
    }
}

/// A FIFO queue holding a fixed number of items. Older items are dropped if the
/// queue is full when a new item is pushed.
pub(crate) struct LossyQueue<M: RawMutex, T, const N: usize> {
    inner: Mutex<M, RefCell<LossyQueueData<T, N>>>,
}

impl<M: RawMutex, T, const N: usize> LossyQueue<M, T, N> {
    pub(crate) const fn new() -> Self {
        Self {
            inner: Mutex::new(RefCell::new(LossyQueueData {
                receiver_waker: None,
                queue: Deque::new(),
            })),
        }
    }

    /// A future that waits for a new item to be available.
    pub(crate) fn pop(&self) -> ReceiveFuture<'_, M, T, N> {
        ReceiveFuture { pipe: self }
    }

    /// Pushes an item into the queue. If the queue is already full the oldest
    /// item is dropped to make space.
    pub(crate) fn push(&self, data: T) {
        self.inner.lock(|cell| {
            let mut inner = cell.borrow_mut();

            if inner.queue.is_full() {
                inner.queue.pop_front();
            }

            // As we pop above the queue cannot be full now.
            let _ = inner.queue.push_back(data);

            if let Some(waker) = inner.receiver_waker.take() {
                waker.wake();
            }
        })
    }
}
