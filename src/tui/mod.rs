pub mod app;
pub mod client;

use anyhow::Result;
use app::{App, Screen, NUM_BARS};
use client::{subscribe_samples, DaemonClient};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Bar, BarChart, BarGroup, Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Terminal;
use spectrum_analyzer::windows::hann_window;
use spectrum_analyzer::{samples_fft_to_spectrum, FrequencyLimit};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::MissedTickBehavior;

use crate::daemon::eq::{BAND_FREQS, GAIN_MAX, GAIN_MIN, N_BANDS};
use crate::daemon::state::{NoisePreset, PlayState};

const FFT_SIZE: usize = 2048;
const SAMPLE_WINDOW_CAP: usize = 4_096;

/// Entry point: run the TUI in a single-threaded Tokio runtime.
///
/// # Errors
/// Returns an error if the terminal cannot be initialised or the daemon is unreachable.
pub fn run(socket_path: PathBuf) -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run_async(socket_path))
}

async fn run_async(socket_path: PathBuf) -> Result<()> {
    let config = crate::config::load().unwrap_or_default();
    let mut app = App::new(config.audio.sample_rate, config.defaults.volume);

    let client = DaemonClient::connect(socket_path.clone()).await?;

    // Sync EQ state from daemon (best-effort; ignore errors).
    if let Ok(gains) = client.get_eq().await {
        app.eq_gains = gains;
    }

    let (samples_tx, mut samples_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(32);
    let _sub_handle = subscribe_samples(socket_path, samples_tx).await?;

    // Spawn a blocking thread that feeds crossterm events into a channel.
    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::channel::<Event>(64);
    tokio::task::spawn_blocking(move || loop {
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
                    terminal.draw(|f| render(f, &app))?;
                }
                event = ev_rx.recv() => {
                    match event {
                        Some(Event::Key(key)) => {
                            handle_key(&mut app, &client, key).await?;
                        }
                        Some(_) => {}
                        None => break,
                    }
                    if app.should_quit {
                        break;
                    }
                }
                samples = samples_rx.recv() => {
                    if let Some(s) = samples {
                        update_spectrum(&mut app, &s);
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

// ── Key handling ──────────────────────────────────────────────────────────────

async fn handle_key(app: &mut App, client: &DaemonClient, key: KeyEvent) -> Result<()> {
    if key.kind != KeyEventKind::Press {
        return Ok(());
    }
    match app.screen {
        Screen::Presets => match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if app.selected_preset > 0 {
                    app.selected_preset -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if app.selected_preset + 1 < app.preset_list.len() {
                    app.selected_preset += 1;
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let preset = app.preset_list[app.selected_preset];
                client.send_command(&format!("PLAY {preset}")).await?;
                app.active_preset = Some(preset);
                app.play_state = PlayState::Running;
                app.screen = Screen::Visualizer;
            }
            KeyCode::Char('q' | 'Q') => {
                app.should_quit = true;
            }
            _ => {}
        },
        Screen::Visualizer => match key.code {
            KeyCode::Left => {
                app.volume = (app.volume - 0.05).clamp(0.0, 1.0);
                client
                    .send_fire_and_forget(&format!("SET_VOLUME {:.2}", app.volume))
                    .await?;
            }
            KeyCode::Right => {
                app.volume = (app.volume + 0.05).clamp(0.0, 1.0);
                client
                    .send_fire_and_forget(&format!("SET_VOLUME {:.2}", app.volume))
                    .await?;
            }
            KeyCode::Tab | KeyCode::Char('p') => {
                app.screen = Screen::Presets;
            }
            KeyCode::Char('e') => {
                app.screen = Screen::Equalizer;
            }
            KeyCode::Char('s') => {
                client.send_command("STOP").await?;
                app.play_state = PlayState::Stopped;
            }
            KeyCode::Char('q' | 'Q') => {
                app.should_quit = true;
            }
            _ => {}
        },
        Screen::Equalizer => match key.code {
            KeyCode::Left => {
                if app.selected_eq_band > 0 {
                    app.selected_eq_band -= 1;
                }
            }
            KeyCode::Right => {
                if app.selected_eq_band + 1 < N_BANDS {
                    app.selected_eq_band += 1;
                }
            }
            KeyCode::Up => {
                let band = app.selected_eq_band;
                app.eq_gains[band] = (app.eq_gains[band] + 1.0).clamp(GAIN_MIN, GAIN_MAX);
                client.set_eq_band(band, app.eq_gains[band]).await?;
            }
            KeyCode::Down => {
                let band = app.selected_eq_band;
                app.eq_gains[band] = (app.eq_gains[band] - 1.0).clamp(GAIN_MIN, GAIN_MAX);
                client.set_eq_band(band, app.eq_gains[band]).await?;
            }
            KeyCode::Char('r') => {
                app.eq_gains = [0.0f32; N_BANDS];
                for band in 0..N_BANDS {
                    client.set_eq_band(band, 0.0).await?;
                }
            }
            KeyCode::Esc | KeyCode::Backspace => {
                app.screen = Screen::Visualizer;
            }
            KeyCode::Char('q' | 'Q') => {
                app.should_quit = true;
            }
            _ => {}
        },
    }
    Ok(())
}

// ── Spectrum ──────────────────────────────────────────────────────────────────

fn update_spectrum(app: &mut App, new_samples: &[f32]) {
    app.sample_window.extend_from_slice(new_samples);
    // Keep only the last SAMPLE_WINDOW_CAP samples.
    let len = app.sample_window.len();
    if len > SAMPLE_WINDOW_CAP {
        app.sample_window.drain(..len - SAMPLE_WINDOW_CAP);
    }
    app.bar_heights = samples_to_bars(&app.sample_window, app.sample_rate);
}

fn samples_to_bars(samples: &[f32], sample_rate: u32) -> [u64; NUM_BARS] {
    // Prepare FFT input: take last FFT_SIZE samples, zero-pad at the front if needed.
    let mut fft_input = vec![0.0f32; FFT_SIZE];
    let n = samples.len().min(FFT_SIZE);
    if n > 0 {
        fft_input[FFT_SIZE - n..].copy_from_slice(&samples[samples.len() - n..]);
    }

    let windowed = hann_window(&fft_input);
    let Ok(spectrum) = samples_fft_to_spectrum(
        &windowed,
        sample_rate,
        FrequencyLimit::Range(20.0, 20_000.0),
        None,
    ) else {
        return [0; NUM_BARS];
    };

    let data = spectrum.data();
    let mut bars = [0u64; NUM_BARS];

    for (i, bar) in bars.iter_mut().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let f_lo = 20.0_f32 * 1000.0_f32.powf(i as f32 / NUM_BARS as f32);
        #[allow(clippy::cast_precision_loss)]
        let f_hi = 20.0_f32 * 1000.0_f32.powf((i + 1) as f32 / NUM_BARS as f32);

        let peak = data
            .iter()
            .filter(|(f, _)| {
                let hz = f.val();
                hz >= f_lo && hz < f_hi
            })
            .map(|(_, amp)| amp.val())
            .fold(0.0_f32, f32::max);

        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        {
            *bar = (peak.clamp(0.0, 1.0) * 100.0) as u64;
        }
    }

    bars
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(frame: &mut ratatui::Frame, app: &App) {
    match app.screen {
        Screen::Presets => render_presets(frame, app),
        Screen::Visualizer => render_visualizer(frame, app),
        Screen::Equalizer => render_eq(frame, app),
    }
}

fn render_presets(frame: &mut ratatui::Frame, app: &App) {
    let items: Vec<ListItem> = app
        .preset_list
        .iter()
        .map(|preset| {
            let prefix = if app.active_preset == Some(*preset) {
                "▶ "
            } else {
                "  "
            };
            let name = match preset {
                NoisePreset::White => "White Noise",
                NoisePreset::Pink => "Pink Noise",
                NoisePreset::Brown => "Brown Noise",
            };
            ListItem::new(format!("{prefix}{name}"))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Woosh — Select Preset ")
                .borders(Borders::ALL),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut list_state = ListState::default();
    list_state.select(Some(app.selected_preset));

    frame.render_stateful_widget(list, frame.area(), &mut list_state);
}

fn render_visualizer(frame: &mut ratatui::Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(frame.area());

    // Header.
    let status_dot = if app.play_state == PlayState::Running {
        "●"
    } else {
        "○"
    };
    let preset_name = app.active_preset.map_or("none", |p| match p {
        NoisePreset::White => "white",
        NoisePreset::Pink => "pink",
        NoisePreset::Brown => "brown",
    });
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let vol_pct = (app.volume * 100.0).clamp(0.0, 100.0) as u8;
    let header_text = format!(" Woosh  {status_dot} {preset_name}  vol: {vol_pct}% ");
    let header = Paragraph::new(header_text).block(Block::default().borders(Borders::ALL));
    frame.render_widget(header, chunks[0]);

    // Spectrum bar chart.
    let bar_data: Vec<(&str, u64)> = app.bar_heights.iter().map(|&h| ("", h)).collect();
    let chart = BarChart::default()
        .block(Block::default().title(" Spectrum ").borders(Borders::ALL))
        .data(bar_data.as_slice())
        .bar_width(3)
        .bar_gap(1)
        .max(100);
    frame.render_widget(chart, chunks[1]);

    // Footer hints.
    let footer = Paragraph::new(" ← → vol   p presets   e eq   s stop   q quit ");
    frame.render_widget(footer, chunks[2]);
}

fn render_eq(frame: &mut ratatui::Frame, app: &App) {
    const LABELS: [&str; N_BANDS] = [
        "31", "63", "125", "250", "500", "1k", "2k", "4k", "8k", "16k",
    ];

    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(frame.area());

    // Header: selected band frequency and current gain.
    let band_freq = BAND_FREQS[app.selected_eq_band];
    let gain = app.eq_gains[app.selected_eq_band];
    let gain_str = if gain.abs() < 0.5 {
        "0 dB".to_owned()
    } else {
        format!("{gain:+.0} dB")
    };
    let header_text = format!(" EQ — Band: {band_freq:.0} Hz   Gain: {gain_str} ");
    let header = Paragraph::new(header_text).block(Block::default().borders(Borders::ALL));
    frame.render_widget(header, chunks[0]);

    // Bar chart: 10 bands, each bar maps ±12 dB → 0–24.
    let bars: Vec<Bar<'static>> = (0..N_BANDS)
        .map(|i| {
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let value = (app.eq_gains[i] + 12.0).clamp(0.0, 24.0) as u64;
            let bar = Bar::default()
                .value(value)
                .label(Line::from(LABELS[i]));
            if i == app.selected_eq_band {
                bar.style(Style::default().fg(Color::Yellow))
            } else {
                bar
            }
        })
        .collect();

    let chart = BarChart::default()
        .block(Block::default().title(" Equalizer ").borders(Borders::ALL))
        .data(BarGroup::default().bars(&bars))
        .bar_width(5)
        .bar_gap(1)
        .max(24);
    frame.render_widget(chart, chunks[1]);

    // Readout: all gains with selected band bracketed.
    let readout: String = (0..N_BANDS)
        .map(|i| {
            let g = app.eq_gains[i];
            let s = if g.abs() < 0.5 {
                "0".to_owned()
            } else if g > 0.0 {
                #[allow(clippy::cast_possible_truncation)]
                let v = g.round() as i32;
                format!("+{v}")
            } else {
                #[allow(clippy::cast_possible_truncation)]
                let v = g.round() as i32;
                format!("{v}")
            };
            if i == app.selected_eq_band {
                format!("[{s}]")
            } else {
                s
            }
        })
        .collect::<Vec<_>>()
        .join("  ");
    let readout_widget =
        Paragraph::new(format!(" {readout} ")).block(Block::default().borders(Borders::ALL));
    frame.render_widget(readout_widget, chunks[2]);

    // Footer hints.
    let footer =
        Paragraph::new(" ← → band   ↑ ↓ ±1 dB   r reset   Esc back   q quit ");
    frame.render_widget(footer, chunks[3]);
}
