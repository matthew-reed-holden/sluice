//! ratatui view rendering for lazysluice.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::app::{AppState, ConnStatus, Screen};

pub fn draw(frame: &mut Frame, state: &AppState) {
    let size = frame.size();

    // Status bar at bottom (1 line)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(size);

    let main_area = chunks[0];
    let status_area = chunks[1];

    // Render status bar
    let status_text = match &state.conn_status {
        ConnStatus::Disconnected => Span::styled("Disconnected", Style::default().fg(Color::Red)),
        ConnStatus::Connecting => Span::styled("Connecting...", Style::default().fg(Color::Yellow)),
        ConnStatus::Connected => Span::styled("Connected", Style::default().fg(Color::Green)),
        ConnStatus::Error(e) => {
            Span::styled(format!("Error: {e}"), Style::default().fg(Color::Red))
        }
    };
    let status_bar = Paragraph::new(Line::from(vec![
        Span::raw(" ["),
        status_text,
        Span::raw("] "),
        Span::styled(
            "q:quit ?:help Tab:switch",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(status_bar, status_area);

    // Main content
    if state.show_help || state.screen == Screen::Help {
        draw_help(frame, main_area);
    } else {
        match state.screen {
            Screen::TopicList => draw_topic_list(frame, main_area, state),
            Screen::Tail => draw_tail(frame, main_area, state),
            Screen::Publish => draw_publish(frame, main_area, state),
            Screen::Help => draw_help(frame, main_area),
        }
    }
}

fn draw_topic_list(frame: &mut Frame, area: Rect, state: &AppState) {
    let items: Vec<ListItem> = state
        .topics
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let style = if i == state.topic_cursor {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(t.name.clone()).style(style)
        })
        .collect();

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Topics"));
    frame.render_widget(list, area);
}

fn draw_tail(frame: &mut Frame, area: Rect, state: &AppState) {
    let title = if state.paused {
        "Tail [PAUSED]"
    } else {
        "Tail"
    };

    let items: Vec<ListItem> = state
        .messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let ack_marker = if state.acked_ids.contains(&m.message_id) {
                "✓ "
            } else {
                "  "
            };
            let payload = render_payload(&m.payload);
            let line = format!(
                "{}[{}] seq={} ts={} {}",
                ack_marker, m.message_id, m.sequence, m.timestamp, payload
            );
            let style = if i == state.message_cursor {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(line).style(style)
        })
        .collect();

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(list, area);
}

fn draw_publish(frame: &mut Frame, area: Rect, state: &AppState) {
    let status_style = if state
        .publish_status
        .as_ref()
        .is_some_and(|s| s.starts_with("Error"))
    {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Green)
    };

    let can_send =
        !state.publish_topic.trim().is_empty() && !state.publish_payload.trim().is_empty();
    let hint = if can_send {
        "Enter to send"
    } else {
        "Enter topic and payload"
    };

    let text = vec![
        Line::from(vec![
            Span::raw("Topic: "),
            Span::styled(&state.publish_topic, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("Payload: "),
            Span::styled(&state.publish_payload, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            state.publish_status.as_deref().unwrap_or(hint),
            status_style,
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Type to edit payload, Backspace to delete",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let para = Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Publish"));
    frame.render_widget(para, area);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let text = r#"
  q        Quit
  ?        Toggle help
  Tab      Cycle views
  j / ↓    Move selection down
  k / ↑    Move selection up
  Enter    Select topic / start tail
  p        Jump to Publish view
  Space    Pause/resume tail
  a        Ack selected message
"#;
    let para = Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Help"));
    frame.render_widget(para, area);
}

/// Safe payload rendering: UTF-8 if valid, else hex preview + length.
fn render_payload(bytes: &[u8]) -> String {
    const MAX_DISPLAY: usize = 64;
    match std::str::from_utf8(bytes) {
        Ok(s) if s.len() <= MAX_DISPLAY => s.to_string(),
        Ok(s) => format!("{}… ({} bytes)", &s[..MAX_DISPLAY], bytes.len()),
        Err(_) => {
            let preview: String = bytes
                .iter()
                .take(16)
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(" ");
            format!("[binary] {} ({} bytes)", preview, bytes.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_utf8_short() {
        assert_eq!(render_payload(b"hello"), "hello");
    }

    #[test]
    fn payload_binary() {
        let bin = vec![0x00, 0xff, 0x10];
        let s = render_payload(&bin);
        assert!(s.contains("[binary]"));
        assert!(s.contains("3 bytes"));
    }

    #[test]
    fn payload_oversized_utf8() {
        let long = "x".repeat(100);
        let s = render_payload(long.as_bytes());
        assert!(s.contains("100 bytes"));
    }
}
