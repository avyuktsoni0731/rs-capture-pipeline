use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError, TrySendError};

/// Timestamped pipeline message payload.
#[derive(Debug, Clone)]
pub struct Timed<T> {
    pub timestamp_us: u64,
    pub data: T,
}

impl<T> Timed<T> {
    pub fn new(timestamp_us: u64, data: T) -> Self {
        Self { timestamp_us, data }
    }
}

/// Small bounded queue pair for cross-thread pipeline stages.
#[derive(Clone)]
pub struct StageTx<T> {
    inner: Sender<Timed<T>>,
}

pub struct StageRx<T> {
    inner: Receiver<Timed<T>>,
}

pub fn stage_channel<T>(capacity: usize) -> (StageTx<T>, StageRx<T>) {
    let (tx, rx) = bounded(capacity);
    (StageTx { inner: tx }, StageRx { inner: rx })
}

impl<T> StageTx<T> {
    pub fn send(&self, item: Timed<T>) -> Result<(), crossbeam_channel::SendError<Timed<T>>> {
        self.inner.send(item)
    }

    pub fn try_send(&self, item: Timed<T>) -> Result<(), TrySendError<Timed<T>>> {
        self.inner.try_send(item)
    }
}

impl<T> StageRx<T> {
    pub fn recv(&self) -> Result<Timed<T>, crossbeam_channel::RecvError> {
        self.inner.recv()
    }

    pub fn try_recv(&self) -> Result<Timed<T>, TryRecvError> {
        self.inner.try_recv()
    }
}

