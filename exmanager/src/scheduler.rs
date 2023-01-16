use std::thread::sleep;
use tokio::sync::mpsc;
use common::{EmulMessage, EmulationEvent};

pub fn schedule(event_sender: mpsc::Sender<EmulMessage>, events: Vec<EmulationEvent>) {
    for event in events.into_iter() {
        let sender = event_sender.clone();
        tokio::spawn(async move {
            sleep(event.time);
            sender.send(EmulMessage::Event(event)).await.unwrap()
        });
    }
}