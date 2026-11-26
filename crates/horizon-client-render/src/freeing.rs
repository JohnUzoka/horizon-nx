//! One long-lived thread that drops what the frame's threads hand it: freed GPU buffers and
//! unloaded chunk columns, whose drops took the frame 15-20 ms. A thread per batch cost each
//! batch a thread's creation and its stack (2 MiB mapped on the Switch, and an exited one closed
//! from the frame loop later), about one a second while playing.
use std::sync::LazyLock;
use std::sync::mpsc::{self, Sender};

type Garbage = Box<dyn Send>;

static FREEING: LazyLock<Option<Sender<Garbage>>> = LazyLock::new(|| {
    let (sender, garbage) = mpsc::channel::<Garbage>();
    std::thread::Builder::new()
        .name("Freeing".into())
        .spawn(move || garbage.into_iter().for_each(drop))
        .ok()?;
    Some(sender)
});

/// Drops `value` on the freeing thread; should the thread not start, it drops here.
pub fn drop_later<T: Send + 'static>(value: T) {
    if let Some(sender) = &*FREEING {
        // A send fails only once the thread is gone; the value then drops here.
        let _ = sender.send(Box::new(value));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// What is handed over is dropped, on another thread.
    #[test]
    fn values_are_dropped_on_the_freeing_thread() {
        struct Marker(Arc<std::sync::Mutex<Option<std::thread::ThreadId>>>);
        impl Drop for Marker {
            fn drop(&mut self) {
                *self.0.lock().unwrap() = Some(std::thread::current().id());
            }
        }
        let seen = Arc::new(std::sync::Mutex::new(None));
        super::drop_later(Marker(seen.clone()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while seen.lock().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        let dropper = seen.lock().unwrap().expect("dropped within 5 s");
        assert_ne!(dropper, std::thread::current().id());
    }
}
