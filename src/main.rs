//! AI Allowance: AI subscription usage and reset times in the Windows tray.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
#[cfg(windows)]
mod dialogs;
#[cfg(windows)]
mod file_picker;
mod notifications;
#[cfg(windows)]
mod platform;
mod providers;
mod refresh;
mod state;
#[cfg(windows)]
mod tooltip;
#[cfg(windows)]
mod tray;

#[cfg(not(windows))]
fn main() {
    eprintln!("AI Allowance currently targets Windows 11 x64 only.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            platform::attach_parent_console();
            print_line(&format!("ai-allowance {}", env!("CARGO_PKG_VERSION")));
            return;
        }
        Some(other) => {
            platform::attach_parent_console();
            eprint_line(&format!("unknown argument: {other}"));
            eprint_line("usage: ai-allowance [--version]");
            std::process::exit(2);
        }
        None => {}
    }

    if !platform::acquire_single_instance() {
        // Another instance already owns the tray icon.
        return;
    }
    app::run();
}

#[cfg(windows)]
fn print_line(s: &str) {
    // Stdout may be an already-closed pipe when a GUI-subsystem binary is
    // invoked from a shell that does not wait for it; fall back to the
    // attached console and never panic.
    use std::io::Write;
    if writeln!(std::io::stdout(), "{s}").is_err() {
        if let Ok(mut con) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            let _ = writeln!(con, "{s}");
        }
    }
}

#[cfg(windows)]
fn eprint_line(s: &str) {
    use std::io::Write;
    if writeln!(std::io::stderr(), "{s}").is_err() {
        if let Ok(mut con) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            let _ = writeln!(con, "{s}");
        }
    }
}

#[cfg(windows)]
mod app {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use chrono::Utc;
    use tao::event::{Event, StartCause};
    use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
    use tao::platform::windows::EventLoopBuilderExtWindows;
    use tray_icon::menu::MenuEvent;
    use tray_icon::TrayIconEvent;

    use crate::config::{self, Config};
    use crate::dialogs::{self, Dialogs};
    use crate::notifications::{Alert, AlertEngine};
    use crate::platform;
    use crate::providers;
    use crate::refresh::RefreshCoordinator;
    use crate::state::{AppState, FetchError, FetchOutcome, ProviderKind, HOVER_MAX_AGE_SECS};
    use crate::tray::{
        TrayUi, MENU_ID_ABOUT, MENU_ID_AUTOSTART, MENU_ID_OPEN_PREFIX, MENU_ID_QUIT,
        MENU_ID_REFRESH, MENU_ID_SETTINGS,
    };

    /// Tooltip countdowns update locally on this cadence without any network.
    const TICK: Duration = Duration::from_secs(30);
    /// How long Quit waits for in-flight fetches to stop. Dropping the runtime
    /// instead would wait without limit for a helper pipe read, which tokio
    /// runs on a blocking thread on Windows, and keep the app (and its
    /// single-instance lock, which blocks upgrades) alive.
    const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

    pub enum UserEvent {
        Tray(TrayIconEvent),
        Menu(MenuEvent),
        Fetched {
            kind: ProviderKind,
            generation: u64,
            result: Result<FetchOutcome, FetchError>,
        },
        SettingsSaved {
            config: Box<Config>,
        },
        /// The usage window's Refresh button or F5.
        RefreshRequested,
    }

    struct App {
        config: Config,
        state: AppState,
        coordinator: RefreshCoordinator,
        alerts: AlertEngine,
        autostart: Option<bool>,
        proxy: EventLoopProxy<UserEvent>,
        runtime: tokio::runtime::Handle,
        last_poll: Option<Instant>,
        generations: HashMap<ProviderKind, u64>,
        fetches: HashMap<ProviderKind, tokio::task::JoinHandle<()>>,
    }

    impl App {
        fn sync_autostart(&mut self) {
            match platform::autostart_enabled() {
                Ok(enabled) => self.autostart = Some(enabled),
                Err(e) => {
                    if self.autostart.is_some() {
                        dialogs::show_error(&e);
                    }
                    self.autostart = None;
                }
            }
        }

        fn enabled_providers(&self) -> Vec<ProviderKind> {
            let mut v = Vec::new();
            if self.config.providers.claude.enabled {
                v.push(ProviderKind::Claude);
            }
            if self.config.providers.codex.enabled {
                v.push(ProviderKind::Codex);
            }
            if self.config.providers.copilot.enabled {
                v.push(ProviderKind::Copilot);
            }
            v
        }

        fn refreshing(&self) -> Vec<ProviderKind> {
            ProviderKind::ALL
                .into_iter()
                .filter(|kind| self.coordinator.is_in_flight(*kind))
                .collect()
        }

        fn spawn_fetch(&mut self, kind: ProviderKind) {
            self.state.mark_loading(kind);
            let proxy = self.proxy.clone();
            let cfg = self.config.providers.clone();
            let generation = self.generations.get(&kind).copied().unwrap_or(0);
            let previous = self.fetches.remove(&kind);
            let handle = self.runtime.spawn(async move {
                if let Some(previous) = previous {
                    if let Err(e) = previous.await {
                        if !e.is_cancelled() {
                            crate::eprint_line(&format!(
                                "AI Allowance: previous provider task failed: {e}"
                            ));
                        }
                    }
                }
                let result = match kind {
                    ProviderKind::Claude => providers::claude::fetch(&cfg.claude).await,
                    ProviderKind::Codex => providers::codex::fetch(&cfg.codex).await,
                    ProviderKind::Copilot => providers::copilot::fetch(&cfg.copilot).await,
                };
                let _ = proxy.send_event(UserEvent::Fetched {
                    kind,
                    generation,
                    result,
                });
            });
            self.fetches.insert(kind, handle);
        }

        fn apply_settings(&mut self, config: Config) {
            let changed = [
                (
                    ProviderKind::Claude,
                    self.config.providers.claude != config.providers.claude,
                ),
                (
                    ProviderKind::Codex,
                    self.config.providers.codex != config.providers.codex,
                ),
                (
                    ProviderKind::Copilot,
                    self.config.providers.copilot != config.providers.copilot,
                ),
            ];
            self.config = config;
            self.last_poll = None;
            for (kind, changed) in changed {
                if !changed {
                    continue;
                }
                if let Some(handle) = self.fetches.get(&kind) {
                    handle.abort();
                }
                *self.generations.entry(kind).or_default() += 1;
                self.coordinator.reset(kind);
                self.alerts
                    .reset_windows_with_prefix(&format!("{}:", kind.key()));
                self.state.set_not_configured(kind, "disabled in Settings");
                if self.enabled_providers().contains(&kind) && self.coordinator.force_begin(kind) {
                    self.spawn_fetch(kind);
                }
            }
        }

        fn initialize_disabled(&mut self) {
            let enabled = self.enabled_providers();
            for kind in ProviderKind::ALL {
                if !enabled.contains(&kind) {
                    self.state.set_not_configured(kind, "disabled in Settings");
                }
            }
        }

        /// Hover-triggered refresh of providers whose data is stale.
        fn refresh_stale(&mut self, max_age_secs: i64) -> bool {
            let now = Utc::now();
            let mut any = false;
            for kind in self.enabled_providers() {
                let last = self.state.snapshot(kind).and_then(|s| s.collected_at);
                if self.coordinator.try_begin(kind, last, now, max_age_secs) {
                    self.spawn_fetch(kind);
                    any = true;
                }
            }
            any
        }

        /// User-requested refresh ignoring age and backoff.
        fn refresh_forced(&mut self) -> bool {
            let mut any = false;
            for kind in self.enabled_providers() {
                if self.coordinator.force_begin(kind) {
                    self.spawn_fetch(kind);
                    any = true;
                }
            }
            any
        }

        fn handle_fetched(&mut self, kind: ProviderKind, result: Result<FetchOutcome, FetchError>) {
            let now = Utc::now();
            match result {
                Ok(outcome) => {
                    self.coordinator.complete(kind, true, None, now);
                    let windows = outcome.windows.clone();
                    let account_changed = self.state.apply_success(kind, outcome, now);
                    if account_changed {
                        self.alerts
                            .reset_windows_with_prefix(&format!("{}:", kind.key()));
                    }
                    if self.config.notifications.enabled {
                        for w in &windows {
                            for alert in self.alerts.observe_fresh(w) {
                                deliver(&alert);
                            }
                        }
                    }
                }
                Err(err) => {
                    let retry_after = match &err {
                        FetchError::RateLimited {
                            retry_after_secs, ..
                        } => *retry_after_secs,
                        _ => None,
                    };
                    self.coordinator.complete(kind, false, retry_after, now);
                    self.state.apply_failure(kind, &err);
                }
            }
        }

        fn maybe_poll(&mut self) {
            if !self.config.general.poll_enabled {
                return;
            }
            let interval = Duration::from_secs(self.config.general.poll_interval_secs.max(60));
            let due = self.last_poll.is_none_or(|t| t.elapsed() >= interval);
            if due {
                self.last_poll = Some(Instant::now());
                self.refresh_stale(self.config.general.poll_interval_secs.max(60) as i64 - 5);
            }
        }
    }

    fn deliver(alert: &Alert) {
        match alert {
            Alert::Low { label, threshold } => platform::show_toast(
                "AI Allowance",
                &format!("{label}: {threshold}% or less remaining"),
            ),
            Alert::Reset { label } => platform::show_toast(
                "AI Allowance",
                &format!("{label}: allowance reset confirmed"),
            ),
        }
    }

    pub fn run() {
        if let Err(e) = dialogs::initialize() {
            dialogs::show_error(&e);
            return;
        }
        let (config, config_warning) = config::load_or_create();
        if let Some(w) = &config_warning {
            crate::eprint_line(&format!("AI Allowance: {w}"));
            dialogs::show_error(w);
        }

        let event_loop = EventLoopBuilder::<UserEvent>::with_user_event()
            .with_msg_hook(Dialogs::process_message)
            .build();
        let proxy = event_loop.create_proxy();

        let tray_proxy = proxy.clone();
        TrayIconEvent::set_event_handler(Some(move |event| {
            let _ = tray_proxy.send_event(UserEvent::Tray(event));
        }));
        let menu_proxy = proxy.clone();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = menu_proxy.send_event(UserEvent::Menu(event));
        }));

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");

        let autostart = match platform::autostart_enabled() {
            Ok(enabled) => Some(enabled),
            Err(e) => {
                dialogs::show_error(&e);
                None
            }
        };
        let mut app = App {
            config,
            state: AppState::default(),
            coordinator: RefreshCoordinator::default(),
            alerts: AlertEngine::default(),
            autostart,
            proxy,
            runtime: runtime.handle().clone(),
            last_poll: None,
            generations: HashMap::new(),
            fetches: HashMap::new(),
        };
        let mut runtime = Some(runtime);

        app.initialize_disabled();
        if app.autostart == Some(true) {
            if let Err(e) = platform::set_autostart(true) {
                dialogs::show_error(&e);
            }
            app.sync_autostart();
        }

        let mut tray: Option<TrayUi> = None;
        let dialogs = Dialogs::default();

        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::WaitUntil(Instant::now() + TICK);
            match event {
                Event::NewEvents(StartCause::Init) => {
                    match TrayUi::new(&app.state, &app.config, app.autostart) {
                        Ok(t) => tray = Some(t),
                        Err(e) => {
                            crate::eprint_line(&format!("AI Allowance: {e}"));
                            *control_flow = ControlFlow::Exit;
                            return;
                        }
                    }
                    // Initial startup fetch; there is no cached data yet.
                    app.refresh_stale(HOVER_MAX_AGE_SECS);
                }
                Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                    // Local countdown updates need no network requests.
                    app.maybe_poll();
                    app.sync_autostart();
                    if let Some(t) = &mut tray {
                        t.update(&app.state, &app.config, app.autostart);
                    }
                    dialogs.update_usage(&app.state, &app.refreshing());
                }
                Event::UserEvent(UserEvent::Tray(tray_event)) => {
                    if let Some(t) = &mut tray {
                        t.handle_event(&tray_event);
                    }
                    if matches!(
                        tray_event,
                        TrayIconEvent::DoubleClick {
                            button: tray_icon::MouseButton::Left,
                            ..
                        }
                    ) {
                        app.refresh_stale(HOVER_MAX_AGE_SECS);
                        if let Err(e) =
                            dialogs.usage(&app.state, &app.refreshing(), app.proxy.clone())
                        {
                            dialogs::show_error(&e);
                        }
                    }
                    if matches!(
                        tray_event,
                        TrayIconEvent::Enter { .. } | TrayIconEvent::Move { .. }
                    ) && app.refresh_stale(HOVER_MAX_AGE_SECS)
                    {
                        if let Some(t) = &mut tray {
                            t.update(&app.state, &app.config, app.autostart);
                        }
                    }
                }
                Event::UserEvent(UserEvent::Menu(menu_event)) => {
                    let id = menu_event.id().0.as_str();
                    match id {
                        MENU_ID_REFRESH => {
                            app.refresh_forced();
                        }
                        MENU_ID_SETTINGS => {
                            if let Err(e) =
                                dialogs.settings(&app.config, &app.state, app.proxy.clone())
                            {
                                dialogs::show_error(&e);
                            }
                        }
                        MENU_ID_AUTOSTART => {
                            let result = platform::autostart_enabled()
                                .and_then(|enabled| platform::set_autostart(!enabled));
                            if let Err(e) = result {
                                dialogs::show_error(&e);
                            }
                            app.sync_autostart();
                        }
                        MENU_ID_ABOUT => {
                            if let Err(e) = dialogs.about(app.proxy.clone()) {
                                dialogs::show_error(&e);
                            }
                        }
                        MENU_ID_QUIT => {
                            *control_flow = ControlFlow::Exit;
                        }
                        other => {
                            if let Some(key) = other.strip_prefix(MENU_ID_OPEN_PREFIX) {
                                for kind in ProviderKind::ALL {
                                    if kind.key() == key {
                                        if let Err(e) =
                                            platform::open_usage(kind, &app.config.browser)
                                        {
                                            dialogs::show_error(&e);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(t) = &mut tray {
                        t.update(&app.state, &app.config, app.autostart);
                    }
                    dialogs.update_usage(&app.state, &app.refreshing());
                }
                Event::UserEvent(UserEvent::Fetched {
                    kind,
                    generation,
                    result,
                }) => {
                    if generation != app.generations.get(&kind).copied().unwrap_or(0) {
                        return;
                    }
                    app.fetches.remove(&kind);
                    app.handle_fetched(kind, result);
                    if let Some(t) = &mut tray {
                        t.update(&app.state, &app.config, app.autostart);
                    }
                    dialogs.update_usage(&app.state, &app.refreshing());
                }
                Event::UserEvent(UserEvent::SettingsSaved { config }) => {
                    app.apply_settings(*config);
                    if let Some(t) = &mut tray {
                        t.update(&app.state, &app.config, app.autostart);
                    }
                    dialogs.update_usage(&app.state, &app.refreshing());
                }
                Event::UserEvent(UserEvent::RefreshRequested) => {
                    app.refresh_forced();
                    if let Some(t) = &mut tray {
                        t.update(&app.state, &app.config, app.autostart);
                    }
                    dialogs.update_usage(&app.state, &app.refreshing());
                }
                Event::LoopDestroyed => {
                    dialogs.close_all();
                    // Remove the tray icon on shutdown.
                    tray.take();
                    // Cancelling the fetches drops their helper processes,
                    // which are spawned with kill_on_drop.
                    if let Some(runtime) = runtime.take() {
                        runtime.shutdown_timeout(SHUTDOWN_TIMEOUT);
                    }
                }
                _ => {}
            }
        });
    }
}
