use std::thread::sleep;
use tokio::sync::mpsc;
use common::{TCMessage, EmulationEvent};

pub struct EventScheduler {
    event_sender: mpsc::Sender<TCMessage>,
}

impl EventScheduler {
    pub fn schedule(event_sender: mpsc::Sender<TCMessage>, events: Vec<EmulationEvent>) {
        for event in events.into_iter() {
            let sender = event_sender.clone();
            tokio::spawn(async move {
                sleep(event.time);
                sender.send(TCMessage::Event(event)).await.unwrap()
            });
        }
    }
}