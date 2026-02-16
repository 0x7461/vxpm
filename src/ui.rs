use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
    Frame,
};

use crate::app::{App, PanelMode, View};
use crate::build::BuildJobStatus;
use crate::dep_graph::TreeNode;
use crate::package::Status;

// Catppuccin Macchiato palette
const GREEN: Color = Color::Rgb(166, 218, 149);
const RED: Color = Color::Rgb(237, 135, 150);
const YELLOW: Color = Color::Rgb(238, 212, 159);
const PEACH: Color = Color::Rgb(245, 169, 127);
const TEAL: Color = Color::Rgb(139, 213, 202);
const TEXT: Color = Color::Rgb(202, 211, 245);
const SURFACE0: Color = Color::Rgb(54, 58, 79);
const OVERLAY0: Color = Color::Rgb(110, 115, 141);
const BASE: Color = Color::Rgb(36, 39, 58);

fn status_color(status: &Status) -> Color {
    match status {
        Status::Ok => GREEN,
        Status::NotInstalled => YELLOW,
        Status::NeedsBuild => YELLOW,
        Status::BuildReady => PEACH,
        Status::UpdateAvailable => PEACH,
        Status::BuildFailed => RED,
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let panel_height = match app.panel {
        PanelMode::None => 0,
        PanelMode::Detail => 8,
        PanelMode::BuildLog => 12,
    };

    let chunks = if panel_height > 0 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),           // header
                Constraint::Min(8),              // main
                Constraint::Length(panel_height), // panel
                Constraint::Length(1),            // status bar
            ])
            .split(f.area())
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Min(8),   // main
                Constraint::Length(1), // status bar
            ])
            .split(f.area())
    };

    draw_header(f, chunks[0]);

    match app.view {
        View::List => draw_package_list(f, app, chunks[1]),
        View::Tree => draw_tree_view(f, app, chunks[1]),
    }

    if panel_height > 0 {
        match app.panel {
            PanelMode::Detail => draw_detail(f, app, chunks[2]),
            PanelMode::BuildLog => draw_build_log(f, app, chunks[2]),
            PanelMode::None => {}
        }
        draw_status_bar(f, app, chunks[3]);
    } else {
        draw_status_bar(f, app, chunks[chunks.len() - 1]);
    }
}

fn draw_header(f: &mut Frame, area: Rect) {
    let header = Paragraph::new(Line::from(vec![
        Span::styled(" VPM ", Style::default().fg(BASE).bg(TEAL).add_modifier(Modifier::BOLD)),
        Span::styled(" Void Package Manager", Style::default().fg(TEXT)),
    ]));
    f.render_widget(header, area);
}

fn draw_package_list(f: &mut Frame, app: &App, area: Rect) {
    let header_cells = ["Package", "Template", "Installed", "Latest", "Status"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(TEAL).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = app
        .packages
        .iter()
        .enumerate()
        .map(|(i, ps)| {
            let installed_display = ps
                .installed
                .as_ref()
                .map(|v| {
                    v.rfind('-')
                        .map(|idx| v[idx + 1..].to_string())
                        .unwrap_or_else(|| v.clone())
                })
                .unwrap_or_else(|| "-".to_string());

            let latest_display = ps.latest.as_deref().unwrap_or("-").to_string();

            let style = if i == app.selected {
                Style::default().bg(SURFACE0).fg(TEXT)
            } else {
                Style::default().fg(TEXT)
            };

            let status_style = if i == app.selected {
                Style::default()
                    .bg(SURFACE0)
                    .fg(status_color(&ps.status))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(status_color(&ps.status))
                    .add_modifier(Modifier::BOLD)
            };

            Row::new(vec![
                Cell::from(ps.package.name.clone()).style(style),
                Cell::from(ps.package.version.clone()).style(style),
                Cell::from(installed_display).style(style),
                Cell::from(latest_display).style(style),
                Cell::from(ps.status.label()).style(status_style),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(24),
            Constraint::Length(14),
            Constraint::Length(14),
            Constraint::Length(14),
            Constraint::Min(16),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(SURFACE0))
            .title_style(Style::default().fg(TEAL)),
    );

    f.render_widget(table, area);
}

fn draw_tree_view(f: &mut Frame, app: &App, area: Rect) {
    let selected = match app.selected_package() {
        Some(p) => p,
        None => return,
    };

    let name = &selected.package.name;
    let tree = app.dep_graph.reverse_dep_tree(name);

    let mut lines = vec![Line::from(vec![
        Span::styled(
            name.clone(),
            Style::default()
                .fg(TEAL)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" (v{})", selected.package.version),
            Style::default().fg(OVERLAY0),
        ),
    ])];

    render_tree_lines(&tree, "", true, &mut lines);

    if tree.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no reverse dependencies)",
            Style::default().fg(OVERLAY0),
        )));
    }

    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(SURFACE0))
            .title(Span::styled(
                " Dependency Tree ",
                Style::default().fg(TEAL),
            )),
    );
    f.render_widget(para, area);
}

fn render_tree_lines(nodes: &[TreeNode], prefix: &str, is_root: bool, lines: &mut Vec<Line<'static>>) {
    for (i, node) in nodes.iter().enumerate() {
        let is_last = i == nodes.len() - 1;
        let connector = if is_root {
            if is_last { "└── " } else { "├── " }
        } else if is_last {
            "└── "
        } else {
            "├── "
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{}{}", prefix, connector), Style::default().fg(OVERLAY0)),
            Span::styled(node.name.clone(), Style::default().fg(TEXT)),
        ]));

        let child_prefix = format!(
            "{}{}",
            prefix,
            if is_last { "    " } else { "│   " }
        );
        render_tree_lines(&node.children, &child_prefix, false, lines);
    }
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let selected = match app.selected_package() {
        Some(p) => p,
        None => return,
    };

    let pkg = &selected.package;
    let desc = selected.description();

    let rev_deps = app
        .dep_graph
        .reverse
        .get(&pkg.name)
        .map(|d| {
            let mut v: Vec<&String> = d.iter().collect();
            v.sort();
            v.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    let fwd_deps = app
        .dep_graph
        .forward
        .get(&pkg.name)
        .map(|d| {
            let mut v: Vec<&String> = d.iter().collect();
            v.sort();
            v.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    let lines = vec![
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(&pkg.short_desc, Style::default().fg(TEXT)),
        ]),
        Line::from(vec![
            Span::styled("  Homepage: ", Style::default().fg(OVERLAY0)),
            Span::styled(&pkg.homepage, Style::default().fg(TEAL)),
        ]),
        Line::from(vec![
            Span::styled("  Status: ", Style::default().fg(OVERLAY0)),
            Span::styled(desc, Style::default().fg(status_color(&selected.status))),
        ]),
        Line::from(vec![
            Span::styled("  Depends on: ", Style::default().fg(OVERLAY0)),
            Span::styled(
                if fwd_deps.is_empty() { "(none)".to_string() } else { fwd_deps },
                Style::default().fg(TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Depended by: ", Style::default().fg(OVERLAY0)),
            Span::styled(
                if rev_deps.is_empty() { "(none)".to_string() } else { rev_deps },
                Style::default().fg(TEXT),
            ),
        ]),
    ];

    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(SURFACE0))
                .title(Span::styled(
                    format!(" {} ", pkg.name),
                    Style::default().fg(TEAL).add_modifier(Modifier::BOLD),
                )),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

fn draw_build_log(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Queue status line: [OK] pkg  [BUILD] pkg  [WAIT] pkg
    let mut queue_spans: Vec<Span<'static>> = vec![Span::styled("  ", Style::default())];
    for job in &app.build_queue.jobs {
        let (badge, color) = match job.status {
            BuildJobStatus::Success => ("[OK]", GREEN),
            BuildJobStatus::Building => ("[BUILD]", TEAL),
            BuildJobStatus::Failed => ("[FAIL]", RED),
            BuildJobStatus::Pending => ("[WAIT]", OVERLAY0),
        };
        queue_spans.push(Span::styled(
            format!("{} ", badge),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        queue_spans.push(Span::styled(
            format!("{}  ", job.name),
            Style::default().fg(TEXT),
        ));
    }
    lines.push(Line::from(queue_spans));

    // Scrolling build output — show last N lines that fit
    let available = area.height.saturating_sub(3) as usize; // borders + queue line
    let output = &app.build_queue.current_output;
    let start = output.len().saturating_sub(available);
    for line_text in &output[start..] {
        let color = if line_text.starts_with("ERR:") { RED } else { TEXT };
        lines.push(Line::from(Span::styled(
            format!("  {}", line_text),
            Style::default().fg(color),
        )));
    }

    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(SURFACE0))
            .title(Span::styled(
                " Build Log ",
                Style::default().fg(TEAL).add_modifier(Modifier::BOLD),
            )),
    );
    f.render_widget(para, area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let counts = app.status_counts();

    let mut spans = vec![
        Span::styled(" ", Style::default()),
    ];

    if counts.ok > 0 {
        spans.push(Span::styled(format!("{} ok", counts.ok), Style::default().fg(GREEN)));
        spans.push(Span::styled(" │ ", Style::default().fg(OVERLAY0)));
    }
    if counts.update_avail > 0 {
        spans.push(Span::styled(format!("{} updates", counts.update_avail), Style::default().fg(PEACH)));
        spans.push(Span::styled(" │ ", Style::default().fg(OVERLAY0)));
    }
    if counts.needs_build > 0 {
        spans.push(Span::styled(format!("{} need build", counts.needs_build), Style::default().fg(YELLOW)));
        spans.push(Span::styled(" │ ", Style::default().fg(OVERLAY0)));
    }
    if counts.build_ready > 0 {
        spans.push(Span::styled(format!("{} ready", counts.build_ready), Style::default().fg(PEACH)));
        spans.push(Span::styled(" │ ", Style::default().fg(OVERLAY0)));
    }
    if counts.not_installed > 0 {
        spans.push(Span::styled(format!("{} not installed", counts.not_installed), Style::default().fg(YELLOW)));
        spans.push(Span::styled(" │ ", Style::default().fg(OVERLAY0)));
    }
    if counts.build_failed > 0 {
        spans.push(Span::styled(format!("{} failed", counts.build_failed), Style::default().fg(RED)));
        spans.push(Span::styled(" │ ", Style::default().fg(OVERLAY0)));
    }

    // Remove trailing separator
    if spans.len() > 1 {
        spans.pop();
    }

    // Add status message on the right
    if let Some(ref msg) = app.status_msg {
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled(msg.clone(), Style::default().fg(OVERLAY0)));
    }

    // Keybind help
    spans.push(Span::styled("  ", Style::default()));
    spans.push(Span::styled(
        "j/k:nav  Enter:detail  t:tree  u:upstream  b:build  B:build+deps  q:quit",
        Style::default().fg(OVERLAY0),
    ));

    let bar = Paragraph::new(Line::from(spans));
    f.render_widget(bar, area);
}
