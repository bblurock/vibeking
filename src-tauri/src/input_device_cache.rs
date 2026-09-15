//! Keep slow microphone discovery off the UI thread, with at most one scan.
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct InputDeviceCache {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    devices: Vec<String>,
    scanning: bool,
}

impl InputDeviceCache {
    pub fn snapshot(&self) -> Vec<String> {
        self.state.lock().unwrap().devices.clone()
    }

    pub fn refresh(
        self: &Arc<Self>,
        discover: impl FnOnce() -> Vec<String> + Send + 'static,
        changed: impl FnOnce(Vec<String>) + Send + 'static,
    ) -> std::io::Result<bool> {
        {
            let mut state = self.state.lock().unwrap();
            if state.scanning {
                return Ok(false);
            }
            state.scanning = true;
        }
        let cache = Arc::clone(self);
        let result = std::thread::Builder::new()
            .name("vibeking-device-discovery".into())
            .spawn(move || {
                // Driver calls can block for minutes. Never hold the snapshot
                // lock while discovering, or make UI readers wait for this job.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(discover));
                let update = {
                    let mut state = cache.state.lock().unwrap();
                    state.scanning = false;
                    match result {
                        Ok(devices) if devices != state.devices => {
                            state.devices = devices.clone();
                            Some(devices)
                        }
                        _ => None,
                    }
                };
                if let Some(devices) = update {
                    changed(devices);
                }
            });
        if let Err(error) = result {
            self.state.lock().unwrap().scanning = false;
            return Err(error);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn stalled_discovery_does_not_block_ui_or_start_duplicate_scans() {
        let cache = Arc::new(InputDeviceCache::default());
        cache.state.lock().unwrap().devices = vec!["Known microphone".into()];
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (returned_tx, returned_rx) = mpsc::channel();
        let (changed_tx, changed_rx) = mpsc::channel();
        let caller_cache = cache.clone();
        let caller = std::thread::spawn(move || {
            assert!(caller_cache
                .refresh(
                    move || {
                        entered_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        vec!["New microphone".into()]
                    },
                    move |devices| {
                        changed_tx.send(devices).unwrap();
                    }
                )
                .unwrap());
            // Mimic another startup/focus/settings event while the driver hangs.
            assert!(!caller_cache
                .refresh(|| panic!("duplicate scan"), |_| {})
                .unwrap());
            returned_tx.send(caller_cache.snapshot()).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let snapshot = returned_rx.recv_timeout(Duration::from_secs(1));
        // Always release the fake driver before asserting, even on failure.
        release_tx.send(()).unwrap();
        caller.join().unwrap();
        assert_eq!(snapshot.unwrap(), vec!["Known microphone"]);
        assert_eq!(
            changed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            vec!["New microphone"]
        );
        assert_eq!(cache.snapshot(), vec!["New microphone"]);
    }

    #[test]
    fn unchanged_devices_do_not_trigger_refresh_event_loops() {
        let cache = Arc::new(InputDeviceCache::default());
        let (changed_tx, changed_rx) = mpsc::channel();
        cache
            .refresh(Vec::new, move |devices| {
                changed_tx.send(devices).unwrap();
            })
            .unwrap();
        // Sender is dropped once the unchanged scan completes.
        assert!(matches!(
            changed_rx.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }
}
