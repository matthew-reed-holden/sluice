//! ratatui view rendering for lazysluice.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::app::{AppState, Screen};

pub fn draw(frame: &mut Frame, state: &AppState) {
    let size = frame.size();

    // Status bar at bottom (1 line)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(size);

    let main_area = chunks[0];
    let status_area = chunks[1];

    // Render enhanced status bar
    draw_status_bar(frame, status_area, state);

    // Main content
    if state.show_help || state.screen == Screen::Help {
        draw_help(frame, main_area);
    } else {
        match state.screen {
            Screen::TopicList => draw_topic_list(frame, main_area, state),
            Screen::Tail => draw_tail(frame, main_area, state),
            Screen::Publish => draw_publish(frame, main_area, state),
            Screen::Help => draw_help(frame, main_area),
            Screen::CreateTopic => draw_create_topic(frame, main_area, state),
            Screen::MessageDetail => draw_message_detail(frame, main_area, state),
            Screen::Metrics => draw_metrics(frame, main_area, state),
            Screen::ConsumerGroupInput => draw_consumer_group_input(frame, main_area, state),
        }
    }
}

fn draw_status_bar(frame: &mut Frame, area: Rect, state: &AppState) {
    use crate::app::ConnStatus;

    let status_text = match &state.conn_status {
        ConnStatus::Disconnected => Span::styled("Disconnected", Style::default().fg(Color::Red)),
        ConnStatus::Connecting => Span::styled("Connecting...", Style::default().fg(Color::Yellow)),
        ConnStatus::Connected => Span::styled("Connected", Style::default().fg(Color::Green)),
        ConnStatus::Error(e) => {
            Span::styled(format!("Error: {e}"), Style::default().fg(Color::Red))
        }
    };

    // Add browse mode indicator
    let browse_indicator = if state.browse_mode {
        Span::styled(" [BROWSE]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    } else {
        Span::raw("")
    };

    // Add subscription info if available
    let subscription_info = if let Some(ref topic) = state.current_topic {
        format!(" Sub: {}", topic)
    } else {
        String::new()
    };

    let status_bar = Paragraph::new(Line::from(vec![
        Span::raw(" ["),
        status_text,
        Span::raw("]"),
        Span::styled(subscription_info, Style::default().fg(Color::Cyan)),
        browse_indicator,
        Span::raw(" "),
        Span::styled(
            "q:quit ?:help b:browse Tab:switch",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(status_bar, area);
}

fn draw_topic_list(frame: &mut Frame, area: Rect, state: &AppState) {
    let items: Vec<ListItem> = state
        .topics
        .iter()
        .enumerate()
        .map(|(i, t)| {
            // Show indicator for visited topics
            let visited_marker = if state.visited_topics.contains(&t.name) {
                "• "
            } else {
                "  "
            };

            let is_current = state
                .current_topic
                .as_ref()
                .is_some_and(|ct| ct == &t.name);
            let current_marker = if is_current { "▶ " } else { "" };

            // Get message count from stats if available
            let stats_info = if let Some(stats) = state.topic_stats.get(&t.name) {
                format!(" ({} msgs)", stats.total_messages)
            } else {
                String::new()
            };

            let display = format!("{}{}{}{}", visited_marker, current_marker, t.name, stats_info);

            let style = if i == state.topic_cursor {
                Style::default().add_modifier(Modifier::REVERSED)
            } else if is_current {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            };
            ListItem::new(display).style(style)
        })
        .collect();

    let title = if state.browse_mode {
        "Topics [BROWSE MODE]"
    } else {
        "Topics"
    };

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(list, area);
}

fn draw_tail(frame: &mut Frame, area: Rect, state: &AppState) {
    use sluice_client::InitialPosition;

    let position_indicator = match state.initial_position {
        InitialPosition::Earliest => "EARLIEST",
        InitialPosition::Latest => "LATEST",
        InitialPosition::Offset => "OFFSET",
    };

    let title = if state.search_active {
        format!("Tail [SEARCH: {}]", state.search_query)
    } else if !state.search_query.is_empty() && !state.filtered_messages.is_empty() {
        format!(
            "Tail [{}] [FILTERED: {} of {}]",
            position_indicator,
            state.filtered_messages.len(),
            state.messages.len()
        )
    } else if state.paused {
        format!("Tail [PAUSED | {}]", position_indicator)
    } else {
        format!("Tail [{}]", position_indicator)
    };

    let items: Vec<ListItem> = state
        .messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let is_acked = state.acked_ids.contains(&m.message_id);
            let ack_marker = if is_acked { "✓ " } else { "  " };
            let payload = render_payload(&m.payload);

            // Show attribute count if any exist
            let attr_indicator = if m.attributes.is_empty() {
                String::new()
            } else {
                format!(" [+{} attrs]", m.attributes.len())
            };

            let line = format!(
                "{}[{}] seq={} ts={} {}{}",
                ack_marker, m.message_id, m.sequence, m.timestamp, payload, attr_indicator
            );

            // Apply color: green for acked, selection highlight if cursor, or default
            let style = if i == state.message_cursor {
                if is_acked {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::REVERSED)
                } else {
                    Style::default().add_modifier(Modifier::REVERSED)
                }
            } else if is_acked {
                Style::default().fg(Color::Green)
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
    use crate::app::PublishInputField;

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

    // Highlight active field
    let topic_style = if state.publish_active_field == PublishInputField::Topic {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };

    let payload_style = if state.publish_active_field == PublishInputField::Payload {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };

    let text = vec![
        Line::from(vec![
            Span::raw("Topic: "),
            Span::styled(&state.publish_topic, topic_style),
            if state.publish_active_field == PublishInputField::Topic {
                Span::styled(" ◄", Style::default().fg(Color::Green))
            } else {
                Span::raw("")
            },
        ]),
        Line::from(vec![
            Span::raw("Payload: "),
            Span::styled(&state.publish_payload, payload_style),
            if state.publish_active_field == PublishInputField::Payload {
                Span::styled(" ◄", Style::default().fg(Color::Green))
            } else {
                Span::raw("")
            },
        ]),
        Line::from(""),
        Line::from(Span::styled(
            state.publish_status.as_deref().unwrap_or(hint),
            status_style,
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Tab to switch fields, Enter to send, Esc to cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let para = Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Publish"));
    frame.render_widget(para, area);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let text = r#"
  GLOBAL
  q          Quit
  ?          Toggle help
  b          Toggle browse mode (non-destructive viewing)
  Tab        Cycle views / Switch fields in publish

  NAVIGATION
  j / ↓      Move selection down
  k / ↑      Move selection up
  PgDn       Page down (10 items)
  PgUp       Page up (10 items)
  Home       Jump to top
  End        Jump to bottom

  TOPIC LIST
  Enter      Select topic / start tail
  p          Jump to Publish view
  c          Create new topic
  g          Set consumer group
  m          View metrics dashboard

  TAIL VIEW
  Space      Pause/resume tail
  a          Ack selected message (ignored in browse mode)
  e          Subscribe from Earliest (history)
  l          Subscribe from Latest (new only)
  i          Inspect message details
  /          Search/filter messages
  m          View metrics dashboard

  PUBLISH VIEW
  Tab        Switch between Topic/Payload fields
  Enter      Send message
  Esc        Return to topic list
"#;
    let para = Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Help"));
    frame.render_widget(para, area);
}

fn draw_create_topic(frame: &mut Frame, area: Rect, state: &AppState) {
    let status_style = if state
        .create_topic_status
        .as_ref()
        .is_some_and(|s| s.starts_with("Error"))
    {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Green)
    };

    let can_create = state.can_create_topic();
    let hint = if can_create {
        "Enter to create"
    } else {
        "Enter topic name (alphanumeric, -, _, .)"
    };

    let text = vec![
        Line::from(vec![
            Span::raw("Topic Name: "),
            Span::styled(
                &state.create_topic_name,
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ◄", Style::default().fg(Color::Green)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            state.create_topic_status.as_deref().unwrap_or(hint),
            status_style,
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Type to enter name, Enter to create, Esc to cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let para = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("Create Topic"));
    frame.render_widget(para, area);
}

fn draw_message_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    let Some(ref msg) = state.detail_message else {
        let text = vec![Line::from("No message selected")];
        let para = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("Message Detail"));
        frame.render_widget(para, area);
        return;
    };

    let is_acked = state.acked_ids.contains(&msg.message_id);
    let ack_status = if is_acked {
        Span::styled("ACKNOWLEDGED", Style::default().fg(Color::Green))
    } else {
        Span::styled("PENDING", Style::default().fg(Color::Yellow))
    };

    // Format attributes
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Message ID: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&msg.message_id),
        ]),
        Line::from(vec![
            Span::styled("Sequence: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{}", msg.sequence)),
        ]),
        Line::from(vec![
            Span::styled("Timestamp: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{}", msg.timestamp)),
        ]),
        Line::from(vec![
            Span::styled("Status: ", Style::default().add_modifier(Modifier::BOLD)),
            ack_status,
        ]),
        Line::from(""),
    ];

    // Add attributes section if any exist
    if !msg.attributes.is_empty() {
        lines.push(Line::from(Span::styled(
            "Attributes:",
            Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan),
        )));
        for (key, value) in &msg.attributes {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(key, Style::default().fg(Color::Yellow)),
                Span::raw(": "),
                Span::raw(value),
            ]));
        }
        lines.push(Line::from(""));
    } else {
        lines.push(Line::from(Span::styled(
            "Attributes: (none)",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
    }

    // Add payload
    lines.push(Line::from(Span::styled(
        format!("Payload ({} bytes):", msg.payload.len()),
        Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan),
    )));

    let payload_str = render_payload(&msg.payload);
    // Split payload into multiple lines if needed
    for line in payload_str.lines() {
        lines.push(Line::from(Span::raw(format!("  {}", line))));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press Esc to close",
        Style::default().fg(Color::DarkGray),
    )));

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Message Detail"))
        .wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(para, area);
}

fn draw_metrics(frame: &mut Frame, area: Rect, state: &AppState) {
    let uptime = state.uptime_string();
    let pub_rate = state.publish_rate();
    let con_rate = state.consume_rate();

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Metrics Dashboard",
            Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Connection Uptime: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(uptime),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "── Publish Stats ──",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(vec![
            Span::styled("  Total Published: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", state.total_published)),
        ]),
        Line::from(vec![
            Span::styled("  Last Minute: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", state.publish_timestamps.len())),
        ]),
        Line::from(vec![
            Span::styled("  Rate: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{:.2} msgs/sec", pub_rate)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "── Subscribe Stats ──",
            Style::default().fg(Color::Green),
        )),
        Line::from(vec![
            Span::styled("  Total Consumed: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", state.total_consumed)),
        ]),
        Line::from(vec![
            Span::styled("  Last Minute: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", state.consume_timestamps.len())),
        ]),
        Line::from(vec![
            Span::styled("  Rate: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{:.2} msgs/sec", con_rate)),
        ]),
        Line::from(""),
    ];

    // Add subscription info if active
    let mut lines = text;
    if let Some(ref topic) = state.current_topic {
        lines.push(Line::from(Span::styled(
            "── Active Subscription ──",
            Style::default().fg(Color::Magenta),
        )));
        lines.push(Line::from(vec![
            Span::styled("  Topic: ", Style::default().fg(Color::DarkGray)),
            Span::styled(topic, Style::default().fg(Color::Cyan)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Consumer Group: ", Style::default().fg(Color::DarkGray)),
            Span::raw(
                state
                    .consumer_group
                    .as_deref()
                    .unwrap_or("default"),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Messages Buffered: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", state.messages.len())),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Acked: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", state.acked_ids.len())),
        ]));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        "Press Esc to return",
        Style::default().fg(Color::DarkGray),
    )));

    let para = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Metrics"));
    frame.render_widget(para, area);
}

fn draw_consumer_group_input(frame: &mut Frame, area: Rect, state: &AppState) {
    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw("Consumer Group: "),
            Span::styled(
                &state.consumer_group_input,
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ◄", Style::default().fg(Color::Green)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Leave empty for \"default\" group",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Enter to confirm, Esc to cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let para = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("Consumer Group"));
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
