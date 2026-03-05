use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::{LOG_AREA_HEIGHT, app::DashboardApp};
use crate::orchestrator::Phase;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownLineKind {
    HeadingLevel1Or2,
    HeadingLevel3,
    HeadingLevel4,
    Normal,
}

fn classify_markdown_line(line: &str, in_code_block: bool) -> (MarkdownLineKind, bool) {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        return (MarkdownLineKind::Normal, !in_code_block);
    }

    if in_code_block {
        return (MarkdownLineKind::Normal, true);
    }

    if trimmed.starts_with("#### ") {
        (MarkdownLineKind::HeadingLevel4, false)
    } else if trimmed.starts_with("### ") {
        (MarkdownLineKind::HeadingLevel3, false)
    } else if trimmed.starts_with("## ") || trimmed.starts_with("# ") {
        (MarkdownLineKind::HeadingLevel1Or2, false)
    } else {
        (MarkdownLineKind::Normal, false)
    }
}

fn style_conversation_line(line: &str, kind: MarkdownLineKind) -> Line<'static> {
    match kind {
        MarkdownLineKind::HeadingLevel4 => Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(Color::Yellow),
        )),
        MarkdownLineKind::HeadingLevel3 => Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        MarkdownLineKind::HeadingLevel1Or2 => Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        MarkdownLineKind::Normal => Line::from(line.to_string()),
    }
}

fn split_main_layout(area: Rect) -> Vec<Rect> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),               // Status bar
            Constraint::Min(10),                 // Content area
            Constraint::Length(LOG_AREA_HEIGHT), // Log area
        ])
        .split(area)
        .to_vec()
}

fn split_content_layout(area: Rect) -> Vec<Rect> {
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area)
        .to_vec()
}

pub fn conversation_visible_lines_for_area(area: Rect) -> u16 {
    let chunks = split_main_layout(area);
    let content_chunks = split_content_layout(chunks[1]);
    content_chunks[0].height.saturating_sub(2).max(1)
}

pub fn draw(f: &mut Frame, app: &DashboardApp) {
    if app.waiting_for_start {
        draw_start_screen(f, app);
        return;
    }

    // Main layout: top status bar, middle content, bottom logs
    let chunks = split_main_layout(f.area());

    draw_status_bar(f, app, chunks[0]);
    draw_content(f, app, chunks[1]);
    draw_logs(f, app, chunks[2]);
}

fn draw_start_screen(f: &mut Frame, app: &DashboardApp) {
    let area = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(11),   // Config form
            Constraint::Length(3), // Action hint
        ])
        .split(area);

    // Title
    let ver = env!("CARGO_PKG_VERSION");
    let title = Paragraph::new(Line::from(vec![Span::styled(
        format!(" MDTalk v{ver} - Multi-Agent Code Review"),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Interactive config form — 11 fields
    let labels = [
        "  Agent A:     ",
        "  A Timeout:   ",
        "  Agent B:     ",
        "  B Timeout:   ",
        "  Rounds:      ",
        "  Exchanges:   ",
        "  Skip Apply:  ",
        "  Auto Apply:  ",
        "  Apply Level: ",
        "  Language:    ",
        "  Branch Mode: ",
    ];
    let values: [String; 11] = [
        app.start.agent_presets[app.start.agent_a_idx].clone(),
        format!("{}s", app.start.agent_a_timeout_secs),
        app.start.agent_presets[app.start.agent_b_idx].clone(),
        format!("{}s", app.start.agent_b_timeout_secs),
        format!("{}", app.start.rounds),
        format!("{}", app.start.exchanges),
        if app.start.no_apply {
            "Yes".to_string()
        } else {
            "No".to_string()
        },
        if app.start.auto_apply {
            "Yes".to_string()
        } else {
            "No".to_string()
        },
        match app.start.apply_level {
            2 => "High+Med".to_string(),
            3 => "All".to_string(),
            _ => "High".to_string(),
        },
        if app.start.language == "en" {
            "English".to_string()
        } else {
            "中文".to_string()
        },
        if app.start.branch_mode {
            "Yes".to_string()
        } else {
            "No".to_string()
        },
    ];

    let normal_style = Style::default().fg(Color::Gray);
    let selected_bg = Style::default().bg(Color::DarkGray).fg(Color::White);
    let value_colors = [
        Color::Cyan,
        Color::White,
        Color::Magenta,
        Color::White,
        Color::White,
        Color::White,
        Color::Red,
        Color::Yellow,
        Color::Green,
        Color::Cyan,
        Color::Yellow,
    ];

    let mut info_lines = vec![Line::from("")];
    for (i, (label, value)) in labels.iter().zip(values.iter()).enumerate() {
        let is_selected = i == app.start.selected_field;
        let val_color = value_colors[i];

        if is_selected {
            info_lines.push(Line::from(vec![
                Span::styled(*label, selected_bg),
                Span::styled("◄ ", selected_bg.add_modifier(Modifier::BOLD)),
                Span::styled(
                    value,
                    selected_bg.fg(val_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ►", selected_bg.add_modifier(Modifier::BOLD)),
                // pad the rest of the line with the selected background
                Span::styled("  ", selected_bg),
            ]));
        } else {
            info_lines.push(Line::from(vec![
                Span::styled(*label, normal_style),
                Span::styled("  ", normal_style),
                Span::styled(
                    value,
                    Style::default().fg(val_color).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    }
    info_lines.push(Line::from(""));

    let info = Paragraph::new(info_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Review Config "),
    );
    f.render_widget(info, chunks[1]);

    // Action hint
    let hint = Paragraph::new(Line::from(vec![
        Span::styled("  ", Style::default().fg(Color::DarkGray)),
        Span::styled("↑↓", Style::default().fg(Color::Yellow)),
        Span::styled(" Select  ", Style::default().fg(Color::DarkGray)),
        Span::styled("←→", Style::default().fg(Color::Yellow)),
        Span::styled(" Adjust  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "Enter",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" Start  ", Style::default().fg(Color::DarkGray)),
        Span::styled("q", Style::default().fg(Color::Yellow)),
        Span::styled(" Quit", Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(hint, chunks[2]);
}

fn draw_status_bar(f: &mut Frame, app: &DashboardApp, area: Rect) {
    let state = &app.state;
    let en = state.language == "en";
    let has_error = state.error_message.is_some();

    let elapsed = state
        .session_start
        .map(|s| {
            let d = s.elapsed();
            format!(
                "{:02}:{:02}:{:02}",
                d.as_secs() / 3600,
                (d.as_secs() % 3600) / 60,
                d.as_secs() % 60
            )
        })
        .unwrap_or_else(|| "--:--:--".to_string());

    let phase_color = match state.phase {
        Phase::Init => Color::Yellow,
        Phase::AgentAReviewing => Color::Cyan,
        Phase::AgentBResponding => Color::Magenta,
        Phase::CheckConsensus => Color::Yellow,
        Phase::WaitingForApply => Color::Yellow,
        Phase::ApplyChanges => Color::Green,
        Phase::WaitingForMerge => Color::Yellow,
        Phase::Done => {
            if has_error {
                Color::Red
            } else {
                Color::Green
            }
        }
    };

    let phase_text = if en {
        match state.phase {
            Phase::Init => "Initializing".to_string(),
            Phase::AgentAReviewing => format!("Agent A ({}) reviewing", state.agent_a_name),
            Phase::AgentBResponding => format!("Agent B ({}) responding", state.agent_b_name),
            Phase::CheckConsensus => "Checking consensus".to_string(),
            Phase::WaitingForApply => "Waiting for apply".to_string(),
            Phase::ApplyChanges => format!("Agent B ({}) applying", state.agent_b_name),
            Phase::WaitingForMerge => "Waiting for merge".to_string(),
            Phase::Done => {
                if has_error {
                    "Error".to_string()
                } else {
                    "Done".to_string()
                }
            }
        }
    } else {
        match state.phase {
            Phase::Init => "初始化".to_string(),
            Phase::AgentAReviewing => format!("Agent A ({}) 审查中", state.agent_a_name),
            Phase::AgentBResponding => format!("Agent B ({}) 回应中", state.agent_b_name),
            Phase::CheckConsensus => "检测共识".to_string(),
            Phase::WaitingForApply => "等待确认修改".to_string(),
            Phase::ApplyChanges => format!("Agent B ({}) 修改代码中", state.agent_b_name),
            Phase::WaitingForMerge => "等待合并".to_string(),
            Phase::Done => {
                if has_error {
                    "异常退出".to_string()
                } else {
                    "已完成".to_string()
                }
            }
        }
    };

    let lines = vec![
        Line::from(vec![
            Span::styled(
                if en { " Status: " } else { " 状态: " },
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                phase_text,
                Style::default()
                    .fg(phase_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  │  "),
            Span::styled(
                if en { "Round: " } else { "轮次: " },
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                format!("{}/{}", state.current_round, state.max_rounds),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  │  "),
            Span::styled(
                if en { "Exchange: " } else { "讨论: " },
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                format!("{}/{}", state.current_exchange, state.max_exchanges),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        {
            let mut hint_spans = vec![
                Span::styled(
                    if en { " Elapsed: " } else { " 已用时: " },
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(elapsed, Style::default().fg(Color::White)),
                Span::raw("  │  "),
                Span::styled(
                    if en { "" } else { "按 " },
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled("q", Style::default().fg(Color::Yellow)),
                Span::styled(
                    if en { " Quit, " } else { " 退出, " },
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled("↑↓", Style::default().fg(Color::Yellow)),
                Span::styled(
                    if en { " Scroll" } else { " 滚动" },
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            if state.finished {
                hint_spans.push(Span::styled(", ", Style::default().fg(Color::DarkGray)));
                hint_spans.push(Span::styled("r", Style::default().fg(Color::Yellow)));
                hint_spans.push(Span::styled(
                    if en { " Restart" } else { " 重新开始" },
                    Style::default().fg(Color::DarkGray),
                ));
            } else if state.phase == Phase::WaitingForApply {
                hint_spans.push(Span::styled(", ", Style::default().fg(Color::DarkGray)));
                hint_spans.push(Span::styled(
                    "Enter",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ));
                hint_spans.push(Span::styled(
                    if en { " Apply" } else { " 执行修改" },
                    Style::default().fg(Color::DarkGray),
                ));
            } else if state.phase == Phase::WaitingForMerge {
                hint_spans.push(Span::styled(", ", Style::default().fg(Color::DarkGray)));
                hint_spans.push(Span::styled(
                    "Enter",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ));
                hint_spans.push(Span::styled(
                    if en { " Merge" } else { " 合并分支" },
                    Style::default().fg(Color::DarkGray),
                ));
            }
            Line::from(hint_spans)
        },
    ];

    let ver = env!("CARGO_PKG_VERSION");
    let title = if en {
        format!(" MDTalk Dashboard v{ver} ")
    } else {
        format!(" MDTalk 仪表盘 v{ver} ")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn draw_content(f: &mut Frame, app: &DashboardApp, area: Rect) {
    let state = &app.state;
    let en = state.language == "en";

    // Split content area: left = conversation preview, right = agent status
    let chunks = split_content_layout(area);

    // Left: Conversation preview
    let mut in_code_block = false;
    let mut conv_lines: Vec<Line> = Vec::new();
    for line in state
        .conversation_preview
        .lines()
        .skip(app.scroll_offset as usize)
    {
        let (kind, next_in_code_block) = classify_markdown_line(line, in_code_block);
        in_code_block = next_in_code_block;
        conv_lines.push(style_conversation_line(line, kind));
    }

    let conv_block = Block::default().borders(Borders::ALL).title(if en {
        " Conversation "
    } else {
        " 对话预览 "
    });
    let conv_paragraph = Paragraph::new(conv_lines)
        .block(conv_block)
        .wrap(Wrap { trim: false });
    f.render_widget(conv_paragraph, chunks[0]);

    // Right: Agent status + round times
    let agent_a_status = match state.phase {
        Phase::AgentAReviewing => Span::styled(
            if en { "● Reviewing" } else { "● 审查中" },
            Style::default().fg(Color::Green),
        ),
        Phase::Done => Span::styled(
            if en { "✓ Done" } else { "✓ 完成" },
            Style::default().fg(Color::Green),
        ),
        _ => Span::styled(
            if en { "○ Idle" } else { "○ 等待中" },
            Style::default().fg(Color::DarkGray),
        ),
    };
    let agent_b_status = match state.phase {
        Phase::AgentBResponding => Span::styled(
            if en {
                "● Responding"
            } else {
                "● 回应中"
            },
            Style::default().fg(Color::Green),
        ),
        Phase::WaitingForApply => Span::styled(
            if en {
                "⏸ Awaiting"
            } else {
                "⏸ 等待确认"
            },
            Style::default().fg(Color::Yellow),
        ),
        Phase::ApplyChanges => Span::styled(
            if en {
                "● Applying"
            } else {
                "● 修改代码中"
            },
            Style::default().fg(Color::Yellow),
        ),
        Phase::WaitingForMerge => Span::styled(
            if en { "⏸ Merge?" } else { "⏸ 等待合并" },
            Style::default().fg(Color::Yellow),
        ),
        Phase::Done => Span::styled(
            if en { "✓ Done" } else { "✓ 完成" },
            Style::default().fg(Color::Green),
        ),
        _ => Span::styled(
            if en { "○ Idle" } else { "○ 等待中" },
            Style::default().fg(Color::DarkGray),
        ),
    };

    let mut status_lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(" Agent A: ", Style::default().fg(Color::Gray)),
            Span::styled(&state.agent_a_name, Style::default().fg(Color::Cyan)),
            Span::raw(" "),
            agent_a_status,
        ]),
        Line::from(vec![
            Span::styled(" Agent B: ", Style::default().fg(Color::Gray)),
            Span::styled(&state.agent_b_name, Style::default().fg(Color::Magenta)),
            Span::raw(" "),
            agent_b_status,
        ]),
        Line::from(""),
        Line::from(Span::styled(
            if en {
                " Round Times:"
            } else {
                " 轮次耗时:"
            },
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        )),
    ];

    for (i, duration) in state.round_durations.iter().enumerate() {
        let secs = duration.as_secs();
        if en {
            let time_str = format!("{}m{:02}s", secs / 60, secs % 60);
            status_lines.push(Line::from(format!("  Round {}: {time_str}", i + 1)));
        } else {
            let time_str = format!("{}分{:02}秒", secs / 60, secs % 60);
            status_lines.push(Line::from(format!("  第{}轮: {time_str}", i + 1)));
        }
    }

    if state.current_round as usize > state.round_durations.len() && state.phase != Phase::Done {
        status_lines.push(Line::from(Span::styled(
            if en {
                format!("  Round {}: in progress...", state.current_round)
            } else {
                format!("  第{}轮: 进行中...", state.current_round)
            },
            Style::default().fg(Color::Yellow),
        )));
    }

    let status_block = Block::default().borders(Borders::ALL).title(if en {
        " Agent Status "
    } else {
        " Agent 状态 "
    });
    let status_paragraph = Paragraph::new(status_lines).block(status_block);
    f.render_widget(status_paragraph, chunks[1]);
}

fn draw_logs(f: &mut Frame, app: &DashboardApp, area: Rect) {
    let state = &app.state;
    let en = state.language == "en";

    let log_lines: Vec<Line> = state
        .logs
        .iter()
        .skip(app.log_scroll_offset as usize)
        .map(|log| Line::from(format!(" {log}")))
        .collect();

    let block =
        Block::default()
            .borders(Borders::ALL)
            .title(if en { " Logs " } else { " 日志 " });
    let paragraph = Paragraph::new(log_lines).block(block);
    f.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::{MarkdownLineKind, classify_markdown_line, conversation_visible_lines_for_area};

    #[test]
    fn markdown_heading_is_detected_outside_code_block() {
        let (kind, in_code_block) = classify_markdown_line("### Heading", false);
        assert_eq!(kind, MarkdownLineKind::HeadingLevel3);
        assert!(!in_code_block);
    }

    #[test]
    fn indented_markdown_heading_is_detected() {
        let (kind, in_code_block) = classify_markdown_line("   ### Heading", false);
        assert_eq!(kind, MarkdownLineKind::HeadingLevel3);
        assert!(!in_code_block);
    }

    #[test]
    fn hash_lines_inside_code_block_are_not_treated_as_headings() {
        let (_, in_code_block) = classify_markdown_line("```rust", false);
        let (kind, still_in_code_block) = classify_markdown_line("# not heading", in_code_block);
        assert_eq!(kind, MarkdownLineKind::Normal);
        assert!(still_in_code_block);
    }

    #[test]
    fn closing_fence_exits_code_block_mode() {
        let (_, in_code_block) = classify_markdown_line("```", false);
        let (_, in_code_block) = classify_markdown_line("content", in_code_block);
        let (_, in_code_block) = classify_markdown_line("```", in_code_block);
        let (kind, in_code_block) = classify_markdown_line("## heading", in_code_block);
        assert_eq!(kind, MarkdownLineKind::HeadingLevel1Or2);
        assert!(!in_code_block);
    }

    #[test]
    fn visible_conversation_lines_follow_layout() {
        let visible = conversation_visible_lines_for_area(Rect::new(0, 0, 80, 24));
        assert_eq!(visible, 12);
    }

    #[test]
    fn visible_conversation_lines_has_non_zero_floor() {
        let visible = conversation_visible_lines_for_area(Rect::new(0, 0, 80, 8));
        assert!(visible >= 1);
    }
}
