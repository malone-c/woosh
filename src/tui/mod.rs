pub mod app;
mod art;
pub mod client;

use anyhow::Result;
use app::{App, Screen};
use client::DaemonClient;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Bar, BarChart, BarGroup, Block, Borders, List, ListItem, ListState, Paragraph,
};
use ratatui::Terminal;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::MissedTickBehavior;

use crate::daemon::eq::{BAND_FREQS, GAIN_MAX, GAIN_MIN, N_BANDS};
use crate::daemon::state::{NoisePreset, PlayState};

const BORDER_CYAN: Color = Color::Rgb(137, 179, 182);
const TITLE_FG: Color = Color::Rgb(236, 225, 215);
const TITLE_BG: Color = Color::Rgb(52, 48, 44);
const FOOTER_FG: Color = Color::Rgb(193, 167, 142);
const STATUS_FG: Color = Color::Rgb(134, 116, 98);
const PRESET_HIGHLIGHT_BG: Color = Color::Rgb(64, 58, 54);
const PRESET_HIGHLIGHT_FG: Color = Color::Rgb(236, 225, 215);
const EQ_BAR_FG: Color = Color::Rgb(163, 169, 206);
const EQ_SELECTED_FG: Color = Color::Rgb(235, 192, 109);
const PLAYING_DOT_FG: Color = Color::Rgb(133, 182, 149);
const STOPPED_DOT_FG: Color = Color::Rgb(134, 116, 98);
const INNER_BORDER_FG: Color = Color::Rgb(64, 58, 54);

/// Volume change per `[` / `]` press.
const VOLUME_STEP: f32 = 0.05;

/// Entry point: run the TUI in a single-threaded Tokio runtime.
///
/// # Errors
/// Returns an error if the terminal cannot be initialised or drawn to.
pub fn run(socket_path: PathBuf) -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run_async(socket_path))
}

async fn run_async(socket_path: PathBuf) -> Result<()> {
    let config = crate::config::load().unwrap_or_else(|e| {
        tracing::warn!("using default config: {e:#}");
        crate::config::Config::default()
    });
    let mut app = App::new(config.defaults.preset);
    let mut client = DaemonClient::new(socket_path);
    sync_from_daemon(&mut app, &mut client).await;

    // Feed crossterm events into a channel from a detached OS thread. Not
    // spawn_blocking: the runtime's shutdown would wait for this endless loop.
    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::channel::<Event>(64);
    std::thread::spawn(move || loop {
        match crossterm::event::poll(Duration::from_millis(50)) {
            Ok(true) => match crossterm::event::read() {
                Ok(event) => {
                    if ev_tx.blocking_send(event).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            },
            Ok(false) => {}
            Err(_) => break,
        }
    });

    // Terminal setup.
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut tick = tokio::time::interval(Duration::from_millis(33));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let run_result = async {
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    sync_from_daemon(&mut app, &mut client).await;
                    terminal.draw(|f| render(f, &app))?;
                }
                event = ev_rx.recv() => {
                    match event {
                        Some(Event::Key(key)) => handle_key(&mut app, &mut client, key).await,
                        Some(_) => {}
                        None => break,
                    }
                    if app.should_quit {
                        break;
                    }
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    // Always restore terminal regardless of result.
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    run_result
}

/// Copies the daemon's playback state and EQ gains into `app`, or marks it disconnected.
async fn sync_from_daemon(app: &mut App, client: &mut DaemonClient) {
    let synced =
        async { Ok::<_, anyhow::Error>((client.status().await?, client.get_eq().await?)) }.await;
    match synced {
        Ok((status, eq_gains)) => {
            app.connected = true;
            app.active_preset = status.preset;
            app.play_state = status.play_state;
            app.volume = status.volume;
            app.device_available = status.device_available;
            app.eq_gains = eq_gains;
        }
        Err(_) => app.connected = false,
    }
}

// ── Key handling ──────────────────────────────────────────────────────────────

/// Applies a key press. Request errors are ignored: the next tick's sync shows
/// the daemon's real state, or that the connection is lost.
async fn handle_key(app: &mut App, client: &mut DaemonClient, key: KeyEvent) {
    if key.kind != KeyEventKind::Press {
        return;
    }
    match (app.screen, key.code) {
        (_, KeyCode::Char('q')) => app.should_quit = true,
        (_, KeyCode::Char('Q')) => {
            let _ = client.quit_daemon().await;
            app.should_quit = true;
        }
        (Screen::Presets, KeyCode::Up | KeyCode::Char('k')) => {
            app.selected_preset = app.selected_preset.saturating_sub(1);
        }
        (Screen::Presets, KeyCode::Down | KeyCode::Char('j')) => {
            app.selected_preset = (app.selected_preset + 1).min(NoisePreset::ALL.len() - 1);
        }
        (Screen::Presets, KeyCode::Enter | KeyCode::Char(' ')) => {
            let preset = NoisePreset::ALL[app.selected_preset];
            if client.play(preset).await.is_ok() {
                app.active_preset = Some(preset);
                app.play_state = PlayState::Running;
            }
            app.screen = Screen::Equalizer;
        }
        (Screen::Presets, KeyCode::Char('e') | KeyCode::Tab) => app.screen = Screen::Equalizer,
        (Screen::Equalizer, KeyCode::Left | KeyCode::Char('h')) => {
            app.selected_eq_band = app.selected_eq_band.saturating_sub(1);
        }
        (Screen::Equalizer, KeyCode::Right | KeyCode::Char('l')) => {
            app.selected_eq_band = (app.selected_eq_band + 1).min(N_BANDS - 1);
        }
        (Screen::Equalizer, KeyCode::Up | KeyCode::Char('k')) => {
            let band = app.selected_eq_band;
            app.eq_gains[band] = (app.eq_gains[band] + 1.0).clamp(GAIN_MIN, GAIN_MAX);
            let _ = client.set_eq_band(band, app.eq_gains[band]).await;
        }
        (Screen::Equalizer, KeyCode::Down | KeyCode::Char('j')) => {
            let band = app.selected_eq_band;
            app.eq_gains[band] = (app.eq_gains[band] - 1.0).clamp(GAIN_MIN, GAIN_MAX);
            let _ = client.set_eq_band(band, app.eq_gains[band]).await;
        }
        (Screen::Equalizer, KeyCode::Char('r')) => {
            app.eq_gains = [0.0; N_BANDS];
            for band in 0..N_BANDS {
                let _ = client.set_eq_band(band, 0.0).await;
            }
        }
        (Screen::Equalizer, KeyCode::Char('s')) => {
            if app.play_state == PlayState::Running {
                if client.stop().await.is_ok() {
                    app.play_state = PlayState::Stopped;
                }
            } else if let Some(preset) = app.active_preset {
                if client.play(preset).await.is_ok() {
                    app.play_state = PlayState::Running;
                }
            }
        }
        (Screen::Equalizer, KeyCode::Char('[')) => {
            app.volume = (app.volume - VOLUME_STEP).clamp(0.0, 1.0);
            let _ = client.set_volume(app.volume).await;
        }
        (Screen::Equalizer, KeyCode::Char(']')) => {
            app.volume = (app.volume + VOLUME_STEP).clamp(0.0, 1.0);
            let _ = client.set_volume(app.volume).await;
        }
        (
            Screen::Equalizer,
            KeyCode::Tab | KeyCode::Char('p') | KeyCode::Esc | KeyCode::Backspace,
        ) => app.screen = Screen::Presets,
        _ => {}
    }
}

// ── Layout helpers ─────────────────────────────────────────────────────────────

fn centered_rect(area: Rect, max_w: u16, max_h: u16) -> Rect {
    let w = area.width.min(max_w);
    let h = area.height.min(max_h);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render_dither_margins(frame: &mut ratatui::Frame, full: Rect, outer: Rect) {
    // Top strip
    if outer.y > full.y {
        frame.render_widget(
            art::DitherBackground,
            Rect::new(full.x, full.y, full.width, outer.y - full.y),
        );
    }
    // Bottom strip
    if outer.bottom() < full.bottom() {
        frame.render_widget(
            art::DitherBackground,
            Rect::new(
                full.x,
                outer.bottom(),
                full.width,
                full.bottom() - outer.bottom(),
            ),
        );
    }
    // Left strip (rows alongside outer only)
    if outer.x > full.x {
        frame.render_widget(
            art::DitherBackground,
            Rect::new(full.x, outer.y, outer.x - full.x, outer.height),
        );
    }
    // Right strip
    if outer.right() < full.right() {
        frame.render_widget(
            art::DitherBackground,
            Rect::new(
                outer.right(),
                outer.y,
                full.right() - outer.right(),
                outer.height,
            ),
        );
    }
}

fn render(frame: &mut ratatui::Frame, app: &App) {
    let full = frame.area();
    let outer = centered_rect(full, 80, 24);
    render_dither_margins(frame, full, outer);
    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER_CYAN));
    let inner = outer_block.inner(outer);
    frame.render_widget(outer_block, outer);

    let zones = Layout::vertical([
        Constraint::Length(3),  // title bar
        Constraint::Length(16), // content
        Constraint::Length(2),  // footer
        Constraint::Length(1),  // status bar
    ])
    .split(inner);

    render_title_bar(frame, app, zones[0]);
    render_status_bar(frame, app, zones[3]);

    match app.screen {
        Screen::Presets => {
            render_presets(frame, app, zones[1]);
            render_footer_presets(frame, zones[2]);
        }
        Screen::Equalizer => {
            render_eq(frame, app, zones[1]);
            render_footer_eq(frame, zones[2]);
        }
    }
}

fn render_title_bar(frame: &mut ratatui::Frame, app: &App, area: Rect) {
    let playing = app.play_state == PlayState::Running;
    let dot_color = if playing {
        PLAYING_DOT_FG
    } else {
        STOPPED_DOT_FG
    };
    let status_dot = if playing { "●" } else { "○" };
    let preset_name = app.active_preset.map_or("—", NoisePreset::label);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // clamped to 0–100, so the cast is exact
    let vol_pct = (app.volume * 100.0).round().clamp(0.0, 100.0) as u8;
    let screen_name = match app.screen {
        Screen::Presets => "presets",
        Screen::Equalizer => "equalizer",
    };

    let title_line = Line::from(vec![
        Span::styled(
            art::LOGO_LINE,
            Style::default().fg(TITLE_FG).add_modifier(Modifier::BOLD),
        ),
        Span::styled(status_dot, Style::default().fg(dot_color)),
        Span::styled(
            format!(" {preset_name}  vol: {vol_pct}% "),
            Style::default().fg(TITLE_FG),
        ),
    ]);
    let screen_line = Line::from(Span::styled(
        format!(" {screen_name} "),
        Style::default().fg(FOOTER_FG),
    ));

    let para = Paragraph::new(vec![title_line, screen_line])
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(INNER_BORDER_FG)),
        )
        .style(Style::default().bg(TITLE_BG));
    frame.render_widget(para, area);
}

fn render_status_bar(frame: &mut ratatui::Frame, app: &App, area: Rect) {
    let text = if !app.connected {
        " daemon: disconnected  reconnecting…"
    } else if !app.device_available {
        " daemon: connected  audio device unavailable, retrying…"
    } else if app.play_state == PlayState::Running {
        " daemon: connected  playing"
    } else {
        " daemon: connected  stopped"
    };
    let para = Paragraph::new(Span::styled(text, Style::default().fg(STATUS_FG)));
    frame.render_widget(para, area);
}

fn render_footer_presets(frame: &mut ratatui::Frame, area: Rect) {
    let para = Paragraph::new(vec![
        Line::from(Span::styled(
            " ↑ ↓ / j k  navigate   Enter / Space  play + eq   e / Tab  eq",
            Style::default().fg(FOOTER_FG),
        )),
        Line::from(Span::styled(
            " q  quit   Q  quit + stop daemon",
            Style::default().fg(FOOTER_FG),
        )),
    ]);
    frame.render_widget(para, area);
}

fn render_footer_eq(frame: &mut ratatui::Frame, area: Rect) {
    let para = Paragraph::new(vec![
        Line::from(Span::styled(
            " ← →  band   ↑ ↓  ±1 dB   r  reset   s  play/stop   [ ]  vol",
            Style::default().fg(FOOTER_FG),
        )),
        Line::from(Span::styled(
            " Esc / p / Tab  presets   q  quit   Q  quit + stop daemon",
            Style::default().fg(FOOTER_FG),
        )),
    ]);
    frame.render_widget(para, area);
}

fn render_presets(frame: &mut ratatui::Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = NoisePreset::ALL
        .iter()
        .map(|&preset| {
            let prefix = if app.active_preset == Some(preset) {
                "▶ "
            } else {
                "  "
            };
            ListItem::new(format!("{prefix}{} Noise", preset.label()))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(INNER_BORDER_FG)),
        )
        .highlight_style(
            Style::default()
                .bg(PRESET_HIGHLIGHT_BG)
                .fg(PRESET_HIGHLIGHT_FG)
                .add_modifier(Modifier::BOLD),
        );

    let mut list_state = ListState::default();
    list_state.select(Some(app.selected_preset));

    frame.render_stateful_widget(list, area, &mut list_state);
}

/// Bar height for an EQ gain: `GAIN_MIN` maps to 0 and `GAIN_MAX` to the top.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0..=GAIN_MAX-GAIN_MIN, so the cast is exact
fn eq_bar_height(gain_db: f32) -> u64 {
    (gain_db.clamp(GAIN_MIN, GAIN_MAX) - GAIN_MIN).round() as u64
}

/// Gain readout: "0" when within half a dB of flat, else signed whole dB.
fn format_gain(gain_db: f32) -> String {
    if gain_db.abs() < 0.5 {
        "0".to_owned()
    } else {
        format!("{gain_db:+.0}")
    }
}

fn render_eq(frame: &mut ratatui::Frame, app: &App, area: Rect) {
    const LABELS: [&str; N_BANDS] = [
        "31", "63", "125", "250", "500", "1k", "2k", "4k", "8k", "16k",
    ];

    let band_freq = BAND_FREQS[app.selected_eq_band];
    let gain = format_gain(app.eq_gains[app.selected_eq_band]);

    let bars: Vec<Bar<'static>> = (0..N_BANDS)
        .map(|i| {
            let fg = if i == app.selected_eq_band {
                EQ_SELECTED_FG
            } else {
                EQ_BAR_FG
            };
            Bar::default()
                .value(eq_bar_height(app.eq_gains[i]))
                .text_value(format_gain(app.eq_gains[i]))
                .label(Line::from(LABELS[i]))
                .style(Style::default().fg(fg))
        })
        .collect();

    let chart = BarChart::default()
        .block(
            Block::default()
                .title(format!(" Equalizer — {band_freq:.0} Hz  {gain} dB "))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(INNER_BORDER_FG)),
        )
        .data(BarGroup::default().bars(&bars))
        .bar_width(5)
        .bar_gap(1)
        .max(eq_bar_height(GAIN_MAX));
    frame.render_widget(chart, area);
}
