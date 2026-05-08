use super::{BatteryBackend, BatteryEvent};
use std::sync::Mutex;
use tokio::sync::broadcast;

pub struct MockBackend {
    tx: broadcast::Sender<BatteryEvent>,
    queue: Mutex<Vec<BatteryEvent>>,
}

impl MockBackend {
    pub fn new(events_in_pop_order: Vec<BatteryEvent>) -> Self {
        let (tx, _rx) = broadcast::channel(64);
        Self {
            tx,
            queue: Mutex::new(events_in_pop_order),
        }
    }

    pub async fn run_to_completion(&self) {
        loop {
            let next = self.queue.lock().unwrap().pop();
            match next {
                Some(ev) => {
                    let _ = self.tx.send(ev);
                }
                None => break,
            }
        }
    }
}

impl BatteryBackend for MockBackend {
    fn subscribe(&self) -> broadcast::Receiver<BatteryEvent> {
        self.tx.subscribe()
    }
}
