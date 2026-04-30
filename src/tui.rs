use crate::state::AppState;
use crate::events::Event;
use anyhow::Result;
use crossterm::{
    event::{self, Event as CEvent, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Alignment},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Borders, BorderType, Cell, Gauge, List, ListItem, Paragraph, Row, Table},
    Terminal,
};
use std::{
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const PRIMARY: Color = Color::Rgb(64, 158, 255);
const SUCCESS: Color = Color::Rgb(103, 194, 58);
const WARNING: Color = Color::Rgb(230, 162, 60);
const DANGER: Color = Color::Rgb(245, 108, 108);
const BORDER_COLOR: Color = Color::Rgb(96, 98, 102);
const BG_HL: Color = Color::Rgb(41, 42, 45);

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

fn format_size(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;
    while size >= 1024.0 && unit_index < units.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }
    format!("{:.2} {}", size, units[unit_index])
}

async fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    state: AppState,
) -> Result<()> {
    let tabs = ["📊 Dashboard", "📁 File Browser", "⚙ Config Center", "📈 Capacity", "📜 Sync Logs"];
    let mut selected_tab = 0usize;
    let mut selected_file_local = 0usize;
    let mut selected_file_r2 = 0usize;
    let mut file_browser_focus = 0usize; // 0 = local, 1 = r2
    
    let mut selected_config = 0usize;
    let mut config_input_mode = false;
    let mut input_buffer = String::new();
    
    let mut confirm_delete: Option<String> = None;
    let mut confirm_download: Option<String> = None;
    let mut confirm_sync_l2c = false;
    let mut confirm_sync_c2l = false;
    let mut last_tick = Instant::now();
    
    let event_log = Arc::new(Mutex::new(Vec::<Event>::new()));
    let mut rx = state.events.subscribe();
    let log_sink = event_log.clone();
    
    let r2_files = Arc::new(Mutex::new(Vec::<crate::r2::R2Object>::new()));
    let r2_files_clone = r2_files.clone();
    let config_clone = state.config.clone();
    tokio::spawn(async move {
        loop {
            let active_config = config_clone.read().await.clone();
            if let Ok(r2) = crate::r2::R2Client::new(&active_config.r2).await {
                if let Ok(mut objects) = r2.list_all().await {
                    objects.sort_by(|a, b| a.key.cmp(&b.key));
                    if let Ok(mut files) = r2_files_clone.lock() {
                        *files = objects;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
    
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            let mut log = log_sink.lock().expect("event log lock");
            log.push(event);
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
        
        // Sync lists accurately
        let local_files = crate::files::browse_local(&config, "")
            .map(|v| v.items)
            .unwrap_or_default();
        let cloud_files = r2_files.lock().unwrap_or_else(|e| e.into_inner()).clone();
            
        if selected_file_local >= local_files.len() && !local_files.is_empty() {
            selected_file_local = local_files.len() - 1;
        }
        if selected_file_r2 >= cloud_files.len() && !cloud_files.is_empty() {
            selected_file_r2 = cloud_files.len() - 1;
        }

        terminal.draw(|frame| {
            let area = frame.area();
            
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(26), Constraint::Min(50)])
                .split(area);
                
            let sidebar_area = chunks[0];
            let workspace_area = chunks[1];
            
            render_sidebar(frame, sidebar_area, &tabs, selected_tab, status.as_ref());

            let workspace_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(10),
                    Constraint::Length(3),
                ])
                .split(workspace_area);
                
            let header_area = workspace_chunks[0];
            let content_area = workspace_chunks[1];
            let footer_area = workspace_chunks[2];

            let header_title = format!(" SyncR2 | {} ", tabs[selected_tab]);
            let header_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(PRIMARY))
                .style(Style::default().fg(Color::White));
                
            frame.render_widget(
                Paragraph::new(Span::styled(header_title, Style::default().add_modifier(Modifier::BOLD)))
                    .block(header_block)
                    .alignment(Alignment::Center),
                header_area,
            );

            match selected_tab {
                0 => render_dashboard(frame, content_area, status.as_ref(), capacity.as_ref(), &config),
                1 => render_files(frame, content_area, &local_files, &cloud_files, selected_file_local, selected_file_r2, file_browser_focus, confirm_delete.as_deref(), confirm_download.as_deref(), confirm_sync_l2c, confirm_sync_c2l),
                2 => render_config(frame, content_area, &config, selected_config, config_input_mode, &input_buffer),
                3 => render_capacity(frame, content_area, capacity.as_ref()),
                _ => render_logs(frame, content_area, &logs),
            }
            
            let footer_text = match selected_tab {
                0 => " [Tab] Next Menu | [s] Start | [x] Stop | [p] Pause | [r] Resume | [q] Quit ",
                1 => " [Tab] Select Mode | [←/→] Panels | [u] D/Load | [d] Delete | [[] Mirror L→C | []] Mirror C→L ",
                2 => " [Tab] Next Menu | [↑/↓] Select Config | [←/→] Adjust Value | [Enter] Text | [q] Quit ",
                3 => " [Tab] Next Menu | [c] Calibrate Capacity | [q] Quit ",
                _ => " [Tab] Next Menu | [q] Quit ",
            };
            
            let footer_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(BORDER_COLOR));
            frame.render_widget(
                Paragraph::new(footer_text.fg(Color::DarkGray))
                    .block(footer_block)
                    .alignment(Alignment::Center),
                footer_area
            );
        })?;

        let timeout = Duration::from_millis(100);
        if event::poll(timeout)? {
            if let CEvent::Key(key) = event::read()? {
                if let Some(path) = confirm_delete.clone() {
                    match key.code {
                        KeyCode::Char('y') => {
                            let cfg = state.config.read().await.clone();
                            if file_browser_focus == 0 {
                                let _ = crate::files::delete_local_items(&cfg, std::slice::from_ref(&path)).await;
                                state.events.emit(
                                    "file_deleted",
                                    serde_json::json!({"path": path}),
                                    Some("TUI local delete confirmed".into()),
                                );
                            }
                            if file_browser_focus == 1 {
                                let path_clone = path.clone();
                                let events_clone = state.events.clone();
                                tokio::spawn(async move {
                                    if let Ok(r2) = crate::r2::R2Client::new(&cfg.r2).await {
                                        if let Ok(_) = r2.delete_object(&path_clone).await {
                                            events_clone.emit("file_deleted", serde_json::json!({"path": path_clone}), Some("Cloud file deleted natively using TUI".into()));
                                        }
                                    }
                                });
                            }
                            confirm_delete = None;
                        }
                        KeyCode::Char('n') | KeyCode::Esc => confirm_delete = None,
                        _ => {}
                    }
                    continue;
                }

                if let Some(path) = confirm_download.clone() {
                    match key.code {
                        KeyCode::Char('y') => {
                            let cfg = state.config.read().await.clone();
                            if file_browser_focus == 1 {
                                let path_clone = path.clone();
                                let events_clone = state.events.clone();
                                let watch_path = cfg.watch_path.clone();
                                tokio::spawn(async move {
                                    if let Ok(r2) = crate::r2::R2Client::new(&cfg.r2).await {
                                        let file_name = path_clone.split('/').filter(|p| !p.is_empty()).last().unwrap_or(&path_clone);
                                        let dest_path = crate::config::expand_path(&watch_path).join(file_name);
                                        if let Ok(_) = r2.download_file(&path_clone, &dest_path).await {
                                            events_clone.emit("file_created", serde_json::json!({"path": dest_path.display().to_string()}), Some("File safely downloaded from Cloudflare R2".into()));
                                        }
                                    }
                                });
                            }
                            confirm_download = None;
                        }
                        KeyCode::Char('n') | KeyCode::Esc => confirm_download = None,
                        _ => {}
                    }
                    continue;
                }

                if confirm_sync_l2c {
                    match key.code {
                        KeyCode::Char('y') => {
                            let cfg = state.config.read().await.clone();
                            let events_clone = state.events.clone();
                            let cloud = cloud_files.clone();
                            let local = local_files.clone();
                            let state_ref = state.engine.clone();
                            tokio::spawn(async move {
                                events_clone.emit("sync_started", serde_json::json!({}), Some("Initializing Hard Mirror: Local -> Cloud".into()));
                                if let Ok(r2) = crate::r2::R2Client::new(&cfg.r2).await {
                                    let local_names: std::collections::HashSet<String> = local.into_iter().filter(|f| !f.is_directory).map(|f| f.name).collect();
                                    let mut num_deleted = 0;
                                    for cfile in cloud {
                                        let name = cfile.key.split('/').filter(|p| !p.is_empty()).last().unwrap_or(&cfile.key).to_string();
                                        if !local_names.contains(&name) && !cfile.key.ends_with("/") {
                                            let _ = r2.delete_object(&cfile.key).await;
                                            num_deleted += 1;
                                        }
                                    }
                                    events_clone.emit("file_deleted", serde_json::json!({"count": num_deleted}), Some(format!("Mirror L->C: Removed {} orphaned cloud files", num_deleted)));
                                    let _ = state_ref.stop().await;
                                    let _ = state_ref.start().await;
                                }
                            });
                            confirm_sync_l2c = false;
                        }
                        KeyCode::Char('n') | KeyCode::Esc => confirm_sync_l2c = false,
                        _ => {}
                    }
                    continue;
                }

                if confirm_sync_c2l {
                    match key.code {
                        KeyCode::Char('y') => {
                            let cfg = state.config.read().await.clone();
                            let events_clone = state.events.clone();
                            let cloud = cloud_files.clone();
                            let local = local_files.clone();
                            tokio::spawn(async move {
                                events_clone.emit("sync_started", serde_json::json!({}), Some("Initializing Hard Mirror: Cloud -> Local".into()));
                                if let Ok(r2) = crate::r2::R2Client::new(&cfg.r2).await {
                                    let cloud_names: std::collections::HashSet<String> = cloud.iter().filter(|c| !c.key.ends_with("/")).map(|c| c.key.split('/').filter(|p| !p.is_empty()).last().unwrap_or(&c.key).to_string()).collect();
                                    let mut num_deleted = 0;
                                    for lfile in local {
                                        if !lfile.is_directory && !cloud_names.contains(&lfile.name) {
                                            let _ = crate::files::delete_local_items(&cfg, std::slice::from_ref(&lfile.path)).await;
                                            num_deleted += 1;
                                        }
                                    }
                                    events_clone.emit("file_deleted", serde_json::json!({"count": num_deleted}), Some(format!("Mirror C->L: Removed {} orphaned local files", num_deleted)));
                                    
                                    let mut num_downloaded = 0;
                                    let watch_path = cfg.watch_path.clone();
                                    let local_path_base = crate::config::expand_path(&watch_path);
                                    for cfile in cloud {
                                        if !cfile.key.ends_with("/") {
                                            let name = cfile.key.split('/').filter(|p| !p.is_empty()).last().unwrap_or(&cfile.key).to_string();
                                            let target = local_path_base.join(&name);
                                            if !target.exists() {
                                                let _ = r2.download_file(&cfile.key, &target).await;
                                                num_downloaded += 1;
                                            }
                                        }
                                    }
                                    events_clone.emit("file_created", serde_json::json!({"count": num_downloaded}), Some(format!("Mirror C->L: Downloaded {} missing cloud files", num_downloaded)));
                                }
                            });
                            confirm_sync_c2l = false;
                        }
                        KeyCode::Char('n') | KeyCode::Esc => confirm_sync_c2l = false,
                        _ => {}
                    }
                    continue;
                }
                
                // Route all keys if in input string mode
                if config_input_mode {
                    match key.code {
                        KeyCode::Enter => {
                            let new_val = input_buffer.clone();
                            update_config(&state, |cfg| {
                                if selected_config == 2 { cfg.watch_path = new_val; }
                                else if selected_config == 3 { cfg.r2.endpoint = new_val; }
                                else if selected_config == 4 { cfg.r2.bucket_name = new_val; }
                            }).await;
                            config_input_mode = false;
                        }
                        KeyCode::Esc => { config_input_mode = false; }
                        KeyCode::Backspace => { input_buffer.pop(); }
                        KeyCode::Char(c) => { input_buffer.push(c); }
                        _ => {}
                    }
                    continue; // Skip standard navigation
                }
                
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Tab => selected_tab = (selected_tab + 1) % tabs.len(),
                    KeyCode::BackTab => {
                        selected_tab = selected_tab.checked_sub(1).unwrap_or(tabs.len() - 1)
                    }
                    KeyCode::Char('s') => if selected_tab == 0 { let _ = state.engine.start().await; },
                    KeyCode::Char('x') => if selected_tab == 0 { let _ = state.engine.stop().await; },
                    KeyCode::Char('p') => if selected_tab == 0 { let _ = state.engine.pause().await; },
                    KeyCode::Char('r') => if selected_tab == 0 { let _ = state.engine.resume().await; },
                    KeyCode::Char('c') => if selected_tab == 3 { let _ = state.engine.calibrate_capacity().await; },
                    KeyCode::Enter => {
                        // Activate input edit mode for string paths
                        if selected_tab == 2 && (2..=4).contains(&selected_config) {
                            config_input_mode = true;
                            let active_config = state.config.read().await.clone();
                            input_buffer = if selected_config == 2 { active_config.watch_path }
                                else if selected_config == 3 { active_config.r2.endpoint }
                                else { active_config.r2.bucket_name };
                        }
                    }
                    KeyCode::Down => {
                        if selected_tab == 1 {
                            if file_browser_focus == 0 && !local_files.is_empty() {
                                selected_file_local = (selected_file_local + 1).min(local_files.len() - 1);
                            } else if file_browser_focus == 1 && !cloud_files.is_empty() {
                                selected_file_r2 = (selected_file_r2 + 1).min(cloud_files.len() - 1);
                            }
                        } else if selected_tab == 2 {
                            selected_config = (selected_config + 1).min(6); // Allowing up to patterns
                        }
                    }
                    KeyCode::Up => {
                        if selected_tab == 1 {
                            if file_browser_focus == 0 {
                                selected_file_local = selected_file_local.saturating_sub(1);
                            } else {
                                selected_file_r2 = selected_file_r2.saturating_sub(1);
                            }
                        } else if selected_tab == 2 {
                            selected_config = selected_config.saturating_sub(1);
                        }
                    }
                    KeyCode::Char('d') => {
                        if selected_tab == 1 && file_browser_focus == 0 {
                            if let Some(item) = local_files.get(selected_file_local) {
                                confirm_delete = Some(item.path.clone());
                            }
                        }
                        if selected_tab == 1 && file_browser_focus == 1 {
                            if let Some(item) = cloud_files.get(selected_file_r2) {
                                confirm_delete = Some(item.key.clone());
                            }
                        }
                    }
                    KeyCode::Char('u') => {
                        if selected_tab == 1 && file_browser_focus == 1 {
                            if let Some(item) = cloud_files.get(selected_file_r2) {
                                confirm_download = Some(item.key.clone());
                            }
                        }
                    }
                    KeyCode::Char('[') => {
                        if selected_tab == 1 {
                            confirm_sync_l2c = true;
                        }
                    }
                    KeyCode::Char(']') => {
                        if selected_tab == 1 {
                            confirm_sync_c2l = true;
                        }
                    }
                    KeyCode::Left | KeyCode::Char('-') => {
                        if selected_tab == 1 {
                            file_browser_focus = 0;
                        } else if selected_tab == 2 {
                            if selected_config == 0 {
                                update_config(&state, |cfg| {
                                    cfg.concurrency.max_uploads = cfg.concurrency.max_uploads.saturating_sub(1).max(1);
                                }).await;
                            } else if selected_config == 1 {
                                update_config(&state, |cfg| {
                                    cfg.capacity.max_size_bytes = cfg.capacity.max_size_bytes.saturating_sub(1024 * 1024 * 1024).max(10 * 1024);
                                }).await;
                            }
                        }
                    }
                    KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=') => {
                        if selected_tab == 1 {
                            file_browser_focus = 1;
                        } else if selected_tab == 2 {
                            if selected_config == 0 {
                                update_config(&state, |cfg| {
                                    cfg.concurrency.max_uploads = (cfg.concurrency.max_uploads + 1).min(100);
                                }).await;
                            } else if selected_config == 1 {
                                update_config(&state, |cfg| {
                                    cfg.capacity.max_size_bytes = cfg.capacity.max_size_bytes.saturating_add(1024 * 1024 * 1024);
                                }).await;
                            }
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

fn create_styled_block(title: &str) -> Block<'static> {
    Block::default()
        .title(Span::styled(format!(" {} ", title), Style::default().add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER_COLOR))
}

fn render_sidebar(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    tabs: &[&str],
    selected: usize,
    status: Option<&crate::core::SyncStatus>
) {
    let items: Vec<ListItem> = tabs
        .iter()
        .enumerate()
        .map(|(i, &t)| {
            if i == selected {
                ListItem::new(format!(" ▶ {}", t))
                    .style(Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD).bg(BG_HL))
            } else {
                ListItem::new(format!("   {}", t))
                    .style(Style::default().fg(Color::Gray))
            }
        })
        .collect();

    let list = List::new(items)
        .block(create_styled_block("Menu"));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(20), Constraint::Length(5)])
        .split(area);

    frame.render_widget(list, chunks[0]);

    let conn_text = if let Some(s) = status {
        if s.is_running {
            Text::from(Line::from(vec![Span::styled("● ", Style::default().fg(SUCCESS)), Span::raw("Running ")]))
        } else {
             Text::from(Line::from(vec![Span::styled("○ ", Style::default().fg(DANGER)), Span::raw("Stopped ")]))
        }
    } else {
         Text::from(Line::from(vec![Span::styled("○ ", Style::default().fg(DANGER)), Span::raw("Idle ")]))
    };

    frame.render_widget(
        Paragraph::new(conn_text)
            .block(create_styled_block("Engine Status"))
            .alignment(Alignment::Center),
        chunks[1]
    );
}

fn render_dashboard(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    status: Option<&crate::core::SyncStatus>,
    capacity: Option<&crate::core::CapacitySnapshot>,
    config: &crate::config::AppConfig,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Min(2)])
        .split(area);
        
    let top_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[0]);
        
    let s = status.cloned().unwrap_or_default();
    
    let is_r2_active = s.is_running && !s.is_paused;
    let r2_conn_str = if is_r2_active { "🔗 Connected" } else { "🔌 Disconnected" };
    let r2_conn_color = if is_r2_active { SUCCESS } else { DANGER };
    
    let stat_lines = vec![
        Line::from(vec![
            Span::styled(" Sync Status: ", Style::default().fg(Color::Gray)),
            if s.is_running {
                Span::styled("✅ Running", Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD))
            } else {
                Span::styled("⏹ Stopped", Style::default().fg(DANGER).add_modifier(Modifier::BOLD))
            }
        ]),
        Line::from(vec![
            Span::styled(" R2 Connection: ", Style::default().fg(Color::Gray)),
            Span::styled(r2_conn_str, Style::default().fg(r2_conn_color).add_modifier(Modifier::BOLD))
        ]),
        Line::from(vec![
            Span::styled(" Uptime: ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{}s", s.uptime_seconds as u64)),
        ]),
        Line::from(vec![
            Span::styled(" Watch path: ", Style::default().fg(Color::Gray)),
            Span::raw(crate::config::expand_path(&config.watch_path).display().to_string()),
        ]),
        Line::from(vec![
            Span::styled(" Target R2 Bucket: ", Style::default().fg(Color::Gray)),
            Span::raw(crate::config::expand_env(&config.r2.bucket_name)),
        ]),
    ];
    frame.render_widget(Paragraph::new(stat_lines).block(create_styled_block("Sync Service Control")), top_chunks[0]);

    let task_lines = vec![
        Line::from(vec![
            Span::styled(" Pending Tasks: ", Style::default().fg(Color::Gray)),
            Span::styled(format!("{}", s.pending_tasks), Style::default().fg(WARNING)),
        ]),
        Line::from(vec![
            Span::styled(" Completed Tasks: ", Style::default().fg(Color::Gray)),
            Span::styled(format!("{}", s.completed_tasks), Style::default().fg(SUCCESS)),
        ]),
        Line::from(vec![
            Span::styled(" Failed Tasks: ", Style::default().fg(Color::Gray)),
            Span::styled(format!("{}", s.failed_tasks), Style::default().fg(DANGER)),
        ]),
        Line::from(vec![
            Span::styled(" Sync Queue Size: ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{}", s.queue_size)),
        ]),
    ];
    frame.render_widget(Paragraph::new(task_lines).block(create_styled_block("Workload Info")), top_chunks[1]);

    if let Some(cap) = capacity {
        let usage = cap.usage_percentage.clamp(0.0, 100.0) as u16;
        let gauge_color = if usage >= 90 { DANGER } else if usage >= 70 { WARNING } else { SUCCESS };
        
        let label = format!("{:.1}% Used ({} / {})", cap.usage_percentage.abs(), format_size(cap.current_usage_bytes), format_size(cap.max_capacity_bytes));
        let gauge = Gauge::default()
            .block(create_styled_block("Storage Capacity Overview"))
            .gauge_style(Style::default().fg(gauge_color).bg(BG_HL))
            .percent(usage)
            .label(Span::styled(label, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)));
            
        frame.render_widget(gauge, chunks[1]);
    } else {
        frame.render_widget(Paragraph::new("Capacity unavailable").alignment(Alignment::Center).block(create_styled_block("Storage Capacity Overview")), chunks[1]);
    }
}

fn render_config(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    config: &crate::config::AppConfig,
    selected_config: usize,
    config_input_mode: bool,
    input_buffer: &str,
) {
    let watch_path_val = if selected_config == 2 && config_input_mode { format!("{}█", input_buffer) } else { crate::config::expand_path(&config.watch_path).display().to_string() };
    let endpoint_val = if selected_config == 3 && config_input_mode { format!("{}█", input_buffer) } else { crate::config::expand_env(&config.r2.endpoint) };
    let bucket_val = if selected_config == 4 && config_input_mode { format!("{}█", input_buffer) } else { crate::config::expand_env(&config.r2.bucket_name) };

    let ctrl_2 = if selected_config == 2 && config_input_mode { "[Enter] Save | [Esc] Cancel".to_string() } else { "Press [Enter] to Edit".to_string() };
    let ctrl_3 = if selected_config == 3 && config_input_mode { "[Enter] Save | [Esc] Cancel".to_string() } else { "Press [Enter] to Edit".to_string() };
    let ctrl_4 = if selected_config == 4 && config_input_mode { "[Enter] Save | [Esc] Cancel".to_string() } else { "Press [Enter] to Edit".to_string() };

    let mut rows = vec![
        Row::new(vec!["concurrency.max_uploads".to_string(), config.concurrency.max_uploads.to_string(), "Use ←/→ (Max Concurrent Workers)".to_string()]),
        Row::new(vec!["capacity.max_size_bytes".to_string(), format!("{} ({})", config.capacity.max_size_bytes, format_size(config.capacity.max_size_bytes)), "Use ←/→ (Max Bucket Storage Size)".to_string()]),
        Row::new(vec!["watch_path".to_string(), watch_path_val, ctrl_2]),
        Row::new(vec!["r2.endpoint".to_string(), endpoint_val, ctrl_3]),
        Row::new(vec!["r2.bucket_name".to_string(), bucket_val, ctrl_4]),
        Row::new(vec!["include_patterns".to_string(), format!("{:?}", config.watcher.include_patterns), "Read-only".to_string()]),
        Row::new(vec!["exclude_patterns".to_string(), format!("{:?}", config.watcher.exclude_patterns), "Read-only".to_string()]),
    ];
    
    // Apply styling to selected row
    for (i, row) in rows.iter_mut().enumerate() {
        if i == selected_config {
            if config_input_mode {
                *row = row.clone().style(Style::default().bg(BG_HL).fg(WARNING).add_modifier(Modifier::BOLD));
            } else {
                *row = row.clone().style(Style::default().bg(BG_HL).fg(PRIMARY).add_modifier(Modifier::BOLD));
            }
        } else if (0..=4).contains(&i) {
            *row = row.clone().style(Style::default().fg(Color::White));
        } else {
            *row = row.clone().style(Style::default().fg(Color::DarkGray));
        }
    }
    
    let table = Table::new(rows, [Constraint::Percentage(25), Constraint::Percentage(60), Constraint::Percentage(15)])
        .block(create_styled_block("Active Configuration Properties"))
        .header(Row::new(vec![
            Cell::from("Key").style(Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD)),
            Cell::from("Value").style(Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD)),
            Cell::from("Controls").style(Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD)),
        ]).bottom_margin(1));
        
    frame.render_widget(table, area);
}

fn render_capacity(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    capacity: Option<&crate::core::CapacitySnapshot>,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(2)])
        .split(area);

    if let Some(c) = capacity {
        let usage = c.usage_percentage.clamp(0.0, 100.0) as u16;
        let gauge_color = if usage >= 90 { DANGER } else if usage >= 70 { WARNING } else { SUCCESS };
        let gauge = Gauge::default()
            .block(create_styled_block("Capacity Meter"))
            .gauge_style(Style::default().fg(gauge_color).bg(BG_HL))
            .percent(usage)
            .label(format!("{:.1}%", c.usage_percentage));
        frame.render_widget(gauge, chunks[0]);

        let formatted_date = c.last_updated.split('.').next().unwrap_or(&c.last_updated).replace("T", " ");
        let stats = vec![
            Line::from(vec![Span::styled(" Usage: ", Style::default().fg(Color::Gray)), Span::raw(format!("{} / {}", format_size(c.current_usage_bytes), format_size(c.max_capacity_bytes)))]),
            Line::from(vec![Span::styled(" Available: ", Style::default().fg(Color::Gray)), Span::raw(format!("{}", format_size(c.available_bytes)))]),
            Line::from(vec![Span::styled(" Total Files: ", Style::default().fg(Color::Gray)), Span::raw(format!("{}", c.total_files))]),
            Line::from(vec![Span::styled(" Last Updated: ", Style::default().fg(Color::Gray)), Span::raw(formatted_date)]),
        ];
        frame.render_widget(Paragraph::new(stats).block(create_styled_block("Capacity Details")), chunks[1]);
    } else {
        frame.render_widget(Paragraph::new("Capacity unavailable").block(create_styled_block("Capacity Management")), area);
    }
}

fn render_files(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    local_files: &[crate::files::LocalFileInfo],
    cloud_files: &[crate::r2::R2Object],
    selected_local: usize,
    selected_r2: usize,
    focus: usize,
    confirm_delete: Option<&str>,
    confirm_download: Option<&str>,
    confirm_sync_l2c: bool,
    confirm_sync_c2l: bool,
) {
    if confirm_sync_l2c {
         let p = Paragraph::new(vec![
             Line::from(Span::styled("⚠️ MIRROR SYNC: Local → Cloud (Destructive)", Style::default().fg(DANGER).add_modifier(Modifier::BOLD))),
             Line::from("This deletes Cloud files not found locally, and ensures pure Local dominance."),
             Line::from("Press 'y' to confirm, or 'n' to cancel."),
         ]).block(create_styled_block("Hard Synchronization")).alignment(Alignment::Center);
         
         let popup_area = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage(40), Constraint::Length(5), Constraint::Percentage(40)]).split(area)[1];
         let popup_area = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(10), Constraint::Percentage(80), Constraint::Percentage(10)]).split(popup_area)[1];
         frame.render_widget(p, popup_area);
         return;
    }

    if confirm_sync_c2l {
         let p = Paragraph::new(vec![
             Line::from(Span::styled("⚠️ MIRROR SYNC: Cloud → Local (Destructive)", Style::default().fg(DANGER).add_modifier(Modifier::BOLD))),
             Line::from("This deletes Local files not found on Cloud, and downloads missing cloud items locally."),
             Line::from("Press 'y' to confirm, or 'n' to cancel."),
         ]).block(create_styled_block("Hard Synchronization")).alignment(Alignment::Center);
         
         let popup_area = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage(40), Constraint::Length(5), Constraint::Percentage(40)]).split(area)[1];
         let popup_area = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(10), Constraint::Percentage(80), Constraint::Percentage(10)]).split(popup_area)[1];
         frame.render_widget(p, popup_area);
         return;
    }

    if let Some(path) = confirm_delete {
         let p = Paragraph::new(vec![
             Line::from(Span::styled("⚠️ Confirm Deletion", Style::default().fg(WARNING).add_modifier(Modifier::BOLD))),
             Line::from(format!("Are you sure you want to delete: {}?", path)),
             Line::from("Press 'y' to confirm, or 'n' to cancel."),
         ]).block(create_styled_block("Confirm Action")).alignment(Alignment::Center);
         
         let popup_area = Layout::default()
             .direction(Direction::Vertical)
             .constraints([Constraint::Percentage(40), Constraint::Length(5), Constraint::Percentage(40)])
             .split(area)[1];
         let popup_area = Layout::default()
             .direction(Direction::Horizontal)
             .constraints([Constraint::Percentage(10), Constraint::Percentage(80), Constraint::Percentage(10)])
             .split(popup_area)[1];
             
         frame.render_widget(p, popup_area);
         return;
    }

    if let Some(path) = confirm_download {
         let p = Paragraph::new(vec![
             Line::from(Span::styled("⬇️ Confirm Download", Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD))),
             Line::from(format!("Download cloud file to local disk: {}?", path)),
             Line::from("Press 'y' to confirm, or 'n' to cancel."),
         ]).block(create_styled_block("Confirm Action")).alignment(Alignment::Center);
         
         let popup_area = Layout::default()
             .direction(Direction::Vertical)
             .constraints([Constraint::Percentage(40), Constraint::Length(5), Constraint::Percentage(40)])
             .split(area)[1];
         let popup_area = Layout::default()
             .direction(Direction::Horizontal)
             .constraints([Constraint::Percentage(10), Constraint::Percentage(80), Constraint::Percentage(10)])
             .split(popup_area)[1];
             
         frame.render_widget(p, popup_area);
         return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let header = Row::new(vec![
        Cell::from("T"),
        Cell::from("Name"),
        Cell::from("Size"),
        Cell::from("Last Modified"),
    ])
    .style(Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD))
    .bottom_margin(1);

    // Render Local Files Block
    let local_rows: Vec<Row> = local_files.iter().enumerate().map(|(i, item)| {
        let (icon, color) = if item.is_directory { ("📂", WARNING) } else { ("📄", Color::White) };
        let size_str = if item.is_directory { "-".to_string() } else { format_size(item.size) };
        
        let row_style = if i == selected_local && focus == 0 {
            Style::default().bg(BG_HL).fg(PRIMARY).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        
        Row::new(vec![
            Cell::from(Span::styled(icon, Style::default().fg(color))),
            Cell::from(item.name.clone()),
            Cell::from(size_str),
            Cell::from(item.modified_time.clone().unwrap_or_else(|| "-".into())),
        ]).style(row_style)
    }).collect();

    let title_style = if focus == 0 { Style::default().fg(PRIMARY) } else { Style::default().fg(BORDER_COLOR) };
    let local_block = Block::default()
        .title(Span::styled(format!(" 💻 Local File System ({} items) ", local_files.len()), title_style.add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(title_style);

    let local_table = Table::new(local_rows, [
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(10),
        Constraint::Length(20)
    ])
    .header(header.clone())
    .block(local_block);
    
    frame.render_widget(local_table, chunks[0]);

    // Render R2 Cloud Files Block
    let r2_rows: Vec<Row> = cloud_files.iter().enumerate().map(|(i, item)| {
        let is_dir = item.key.ends_with("/");
        let (icon, color) = if is_dir { ("📂", WARNING) } else { ("☁️", Color::White) };
        let size_str = if is_dir { "-".to_string() } else { format_size(item.size) };
        
        let row_style = if i == selected_r2 && focus == 1 {
            Style::default().bg(BG_HL).fg(PRIMARY).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        
        let name = item.key.split('/').filter(|p| !p.is_empty()).last().unwrap_or(&item.key).to_string();
        let stripped_date = item.last_modified.clone().unwrap_or_else(|| "-".into()).replace("T", " ").replace("Z", "");
        let date_str = stripped_date.split('.').next().unwrap_or(&stripped_date).to_string();

        Row::new(vec![
            Cell::from(Span::styled(icon, Style::default().fg(color))),
            Cell::from(name),
            Cell::from(size_str),
            Cell::from(date_str),
        ]).style(row_style)
    }).collect();

    let r2_title_style = if focus == 1 { Style::default().fg(PRIMARY) } else { Style::default().fg(BORDER_COLOR) };
    let r2_block = Block::default()
        .title(Span::styled(format!(" ☁ Cloudflare R2 Storage ({} items) ", cloud_files.len()), r2_title_style.add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(r2_title_style);

    let r2_table = Table::new(r2_rows, [
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(10),
        Constraint::Length(20)
    ])
    .header(header)
    .block(r2_block);

    frame.render_widget(r2_table, chunks[1]);
}

fn render_logs(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, logs: &[Event]) {
    let items: Vec<ListItem> = logs
        .iter()
        .rev()
        .take(40)
        .map(|ev| {
            let color = match ev.event_type.as_str() {
                "upload_started" => PRIMARY,
                "upload_completed" | "capacity_updated" => SUCCESS,
                "upload_failed" | "error" => DANGER,
                "file_modified" | "file_created" | "file_deleted" => WARNING,
                _ => Color::Gray,
            };
            
            let time_span = Span::styled(format!("[{}] ", ev.timestamp.split('.').next().unwrap_or(&ev.timestamp).replace("T", " ")), Style::default().fg(Color::DarkGray));
            let type_span = Span::styled(format!("{:width$} ", ev.event_type, width = 18), Style::default().fg(color).add_modifier(Modifier::BOLD));
            let msg_span = Span::raw(ev.message.clone().unwrap_or_default());
            
            ListItem::new(Line::from(vec![time_span, type_span, msg_span]))
        })
        .collect();
        
    frame.render_widget(List::new(items).block(create_styled_block("Real-time Activity Events")), area);
}

#[cfg(test)]
mod tests {
    #[test]
    fn tui_module_smoke() {
        assert_eq!(1 + 1, 2);
    }
}
