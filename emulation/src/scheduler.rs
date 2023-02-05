use std::thread::sleep;
use tokio::sync::mpsc;
use common::{EmulMessage, EmulationEvent};

pub fn schedule(event_sender: mpsc::Sender<EmulMessage>, events: Vec<EmulationEvent>) {
    for event in events.into_iter() {
        let sender = event_sender.clone();
        tokio::spawn(async move {
            sleep(event.time);
            // Do not care if we can or cannot push the event. The case we cannot only appears
            // when the emulation is done, so no worry.
            let _ = sender.send(EmulMessage::Event(event)).await;
        });
    }
}