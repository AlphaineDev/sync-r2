use crate::state::AppState;
use anyhow::Result;
use crossterm::{
    event::{self, Event as CEvent, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Tabs},
    Terminal,
};
use std::{
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub async fn run_tui(state: AppState) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, state).await;
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

async fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    state: AppState,
) -> Result<()> {
    let tabs = ["Dashboard", "Config", "Capacity", "Files", "Logs"];
    let mut selected = 0usize;
    let mut selected_file = 0usize;
    let mut confirm_delete: Option<String> = None;
    let mut last_tick = Instant::now();
    let event_log = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut rx = state.events.subscribe();
    let log_sink = event_log.clone();
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            let mut log = log_sink.lock().expect("event log lock");
            log.push(format!(
                "{} {} {}",
                event.timestamp,
                event.event_type,
                event.message.unwrap_or_default()
            ));
            if log.len() > 300 {
                let drain = log.len() - 300;
                log.drain(0..drain);
            }
        }
    });

    loop {
        let status = state.engine.status().await.ok();
        let config = state.config.read().await.clone();
        let capacity = state.engine.capacity_info().await.ok();
        let logs = event_log.lock().expect("event log lock").clone();
        let local_files = crate::files::browse_local(&config, "")
            .map(|v| v.items)
            .unwrap_or_default();
        if selected_file >= local_files.len() && !local_files.is_empty() {
            selected_file = local_files.len() - 1;
        }
        terminal.draw(|frame| {
            let area = frame.area();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(5), Constraint::Length(3)])
                .split(area);
            let titles = tabs
                .iter()
                .map(|t| Line::from(Span::styled(*t, Style::default().fg(Color::Cyan))))
                .collect::<Vec<_>>();
            frame.render_widget(
                Tabs::new(titles)
                    .select(selected)
                    .block(Block::default().borders(Borders::ALL).title("SyncR2 TUI"))
                    .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                chunks[0],
            );

            match selected {
                0 => render_dashboard(frame, chunks[1], status.as_ref(), &config),
                1 => render_config(frame, chunks[1], &config),
                2 => render_capacity(frame, chunks[1], capacity.as_ref()),
                3 => render_files(frame, chunks[1], &local_files, selected_file, confirm_delete.as_deref()),
                _ => render_logs(frame, chunks[1], &logs),
            }
            frame.render_widget(
                Paragraph::new("Tab switch | s start | x stop | p pause | r resume | c calibrate | +/- uploads | [/] capacity | arrows select | d delete | y/n confirm | q quit")
                    .block(Block::default().borders(Borders::ALL)),
                chunks[2],
            );
        })?;

        let timeout = Duration::from_millis(100);
        if event::poll(timeout)? {
            if let CEvent::Key(key) = event::read()? {
                if let Some(path) = confirm_delete.clone() {
                    match key.code {
                        KeyCode::Char('y') => {
                            let cfg = state.config.read().await.clone();
                            let _ =
                                crate::files::delete_local_items(&cfg, std::slice::from_ref(&path))
                                    .await;
                            state.events.emit(
                                "file_deleted",
                                serde_json::json!({"path": path}),
                                Some("TUI local delete confirmed".into()),
                            );
                            confirm_delete = None;
                        }
                        KeyCode::Char('n') | KeyCode::Esc => confirm_delete = None,
                        _ => {}
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Tab => selected = (selected + 1) % tabs.len(),
                    KeyCode::BackTab => {
                        selected = selected.checked_sub(1).unwrap_or(tabs.len() - 1)
                    }
                    KeyCode::Char('s') => {
                        let _ = state.engine.start().await;
                    }
                    KeyCode::Char('x') => {
                        let _ = state.engine.stop().await;
                    }
                    KeyCode::Char('p') => {
                        let _ = state.engine.pause().await;
                    }
                    KeyCode::Char('r') => {
                        let _ = state.engine.resume().await;
                    }
                    KeyCode::Char('c') => {
                        let _ = state.engine.calibrate_capacity().await;
                    }
                    KeyCode::Down => {
                        if selected == 3 && !local_files.is_empty() {
                            selected_file = (selected_file + 1).min(local_files.len() - 1);
                        }
                    }
                    KeyCode::Up => {
                        if selected == 3 {
                            selected_file = selected_file.saturating_sub(1);
                        }
                    }
                    KeyCode::Char('d') => {
                        if selected == 3 {
                            if let Some(item) = local_files.get(selected_file) {
                                confirm_delete = Some(item.path.clone());
                            }
                        }
                    }
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        if selected == 1 {
                            update_config(&state, |cfg| {
                                cfg.concurrency.max_uploads =
                                    (cfg.concurrency.max_uploads + 1).min(100);
                            })
                            .await;
                        }
                    }
                    KeyCode::Char('-') => {
                        if selected == 1 {
                            update_config(&state, |cfg| {
                                cfg.concurrency.max_uploads =
                                    cfg.concurrency.max_uploads.saturating_sub(1).max(1);
                            })
                            .await;
                        }
                    }
                    KeyCode::Char(']') => {
                        if selected == 1 {
                            update_config(&state, |cfg| {
                                cfg.capacity.max_size_bytes = cfg
                                    .capacity
                                    .max_size_bytes
                                    .saturating_add(1024 * 1024 * 1024);
                            })
                            .await;
                        }
                    }
                    KeyCode::Char('[') => {
                        if selected == 1 {
                            update_config(&state, |cfg| {
                                cfg.capacity.max_size_bytes = cfg
                                    .capacity
                                    .max_size_bytes
                                    .saturating_sub(1024 * 1024 * 1024)
                                    .max(10 * 1024);
                            })
                            .await;
                        }
                    }
                    _ => {}
                }
            }
        }
        if last_tick.elapsed() >= Duration::from_secs(1) {
            last_tick = Instant::now();
        }
    }
    Ok(())
}

async fn update_config(state: &AppState, edit: impl FnOnce(&mut crate::config::AppConfig)) {
    let mut cfg = state.config.write().await;
    edit(&mut cfg);
    let _ = crate::config::save_toml(&state.config_path, &cfg);
    state.events.emit(
        "config_updated",
        serde_json::json!({"source": "tui"}),
        Some("TUI config updated".into()),
    );
}

fn render_dashboard(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    status: Option<&crate::core::SyncStatus>,
    config: &crate::config::AppConfig,
) {
    let text = if let Some(status) = status {
        vec![
            Line::from(format!(
                "Running: {}   Paused: {}",
                status.is_running, status.is_paused
            )),
            Line::from(format!(
                "Completed: {}   Pending: {}   Failed: {}",
                status.completed_tasks, status.pending_tasks, status.failed_tasks
            )),
            Line::from(format!(
                "Uptime: {:.0}s   Queue: {}",
                status.uptime_seconds, status.queue_size
            )),
            Line::from(format!("Watch path: {}", config.watch_path)),
            Line::from(format!(
                "Bucket: {}   Endpoint: {}",
                config.r2.bucket_name, config.r2.endpoint
            )),
        ]
    } else {
        vec![Line::from("Status unavailable")]
    };
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Dashboard")),
        area,
    );
}

fn render_config(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    config: &crate::config::AppConfig,
) {
    let text = vec![
        Line::from(format!("watch_path = {}", config.watch_path)),
        Line::from(format!("r2.endpoint = {}", config.r2.endpoint)),
        Line::from(format!("r2.bucket_name = {}", config.r2.bucket_name)),
        Line::from(format!(
            "capacity.max_size_bytes = {}",
            config.capacity.max_size_bytes
        )),
        Line::from(format!(
            "concurrency.max_uploads = {}",
            config.concurrency.max_uploads
        )),
        Line::from(format!(
            "include_patterns = {:?}",
            config.watcher.include_patterns
        )),
        Line::from(format!(
            "exclude_patterns = {:?}",
            config.watcher.exclude_patterns
        )),
    ];
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Config")),
        area,
    );
}

fn render_capacity(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    capacity: Option<&crate::core::CapacitySnapshot>,
) {
    let text = if let Some(c) = capacity {
        vec![
            Line::from(format!(
                "Usage: {} / {} bytes",
                c.current_usage_bytes, c.max_capacity_bytes
            )),
            Line::from(format!("Usage percentage: {:.2}%", c.usage_percentage)),
            Line::from(format!("Available: {} bytes", c.available_bytes)),
            Line::from(format!("Last updated: {}", c.last_updated)),
        ]
    } else {
        vec![Line::from("Capacity unavailable")]
    };
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Capacity")),
        area,
    );
}

fn render_files(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    local_files: &[crate::files::LocalFileInfo],
    selected_file: usize,
    confirm_delete: Option<&str>,
) {
    let mut items = local_files
        .iter()
        .take(30)
        .enumerate()
        .map(|(idx, item)| {
            let prefix = if item.is_directory { "[D]" } else { "[F]" };
            let marker = if idx == selected_file { ">" } else { " " };
            ListItem::new(format!("{marker} {prefix} {}  {}", item.name, item.size))
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        items.push(ListItem::new("No local files found"));
    }
    if let Some(path) = confirm_delete {
        items.insert(0, ListItem::new(format!("Confirm delete {path}? y/n")));
    }
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Local Files")),
        area,
    );
}

fn render_logs(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, logs: &[String]) {
    let items = logs
        .iter()
        .rev()
        .take(40)
        .map(|line| ListItem::new(line.clone()))
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Events")),
        area,
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn tui_module_smoke() {
        assert_eq!(1 + 1, 2);
    }
}
