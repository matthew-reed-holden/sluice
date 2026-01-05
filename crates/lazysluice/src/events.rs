//! Input + tick event plumbing for lazysluice TUI.

use std::time::Duration;

use crossterm::event::{Event as CtEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use tokio::sync::mpsc;

/// Events consumed by the controller.
#[derive(Debug)]
pub enum Event {
    Key(KeyEvent),
    Tick,
    Quit,
}

/// Spawns a background task that emits terminal + tick events.
pub fn spawn_event_reader(tick_rate: Duration) -> mpsc::UnboundedReceiver<Event> {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        let mut reader = EventStream::new();
        let mut tick_interval = tokio::time::interval(tick_rate);

        loop {
            tokio::select! {
                _ = tick_interval.tick() => {
                    if tx.send(Event::Tick).is_err() {
                        break;
                    }
                }
                maybe_event = reader.next() => {
                    match maybe_event {
                        Some(Ok(CtEvent::Key(key))) => {
                            // Ctrl-C → quit
                            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                                let _ = tx.send(Event::Quit);
                                break;
                            }
                            if tx.send(Event::Key(key)).is_err() {
                                break;
                            }
                        }
                        Some(Ok(_)) => {
                            // Ignore resize/mouse/etc for MVP.
                        }
                        Some(Err(_)) => break,
                        None => break,
                    }
                }
            }
        }
    });

    rx
}
