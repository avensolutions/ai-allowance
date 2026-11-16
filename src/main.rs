//! AIAllowance: AI subscription usage and reset times in the Windows tray.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod notifications;
#[cfg(windows)]
mod platform;
mod providers;
mod refresh;
mod state;
#[cfg(windows)]
mod tray;

#[cfg(not(windows))]
fn main() {
    eprintln!("AIAllowance currently targets Windows 11 x64 only.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--set-copilot-token") => {
            platform::attach_parent_console();
            set_copilot_token();
            return;
        }
        Some("--version") => {
            platform::attach_parent_console();
            print_line(&format!("aiallowance {}", env!("CARGO_PKG_VERSION")));
            return;
        }
        Some(other) => {
            platform::attach_parent_console();
            eprint_line(&format!("unknown argument: {other}"));
            eprint_line("usage: aiallowance [--set-copilot-token | --version]");
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
fn set_copilot_token() {
    use std::io::BufRead;
    eprint_line(
        "Paste a fine-grained GitHub PAT with the \"Plan\" read permission, then press Enter.",
    );
    eprint_line("The token is stored in Windows Credential Manager, never in files or logs.");
    let mut token = String::new();
    if std::io::stdin().lock().read_line(&mut token).is_err() {
        eprint_line("could not read from stdin");
        std::process::exit(1);
    }
    let token = token.trim();
    if token.is_empty() {
        eprint_line("no token provided; nothing stored");
        std::process::exit(1);
    }
    match providers::copilot::store_token(token) {
        Ok(()) => eprint_line("token stored"),
        Err(e) => {
            eprint_line(&e);
            std::process::exit(1);
        }
    }
}

#[cfg(windows)]
mod app {
    use std::time::{Duration, Instant};

    use chrono::Utc;
    use tao::event::{Event, StartCause};
    use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
    use tray_icon::menu::MenuEvent;
    use tray_icon::TrayIconEvent;

    use crate::config::{self, Config};
    use crate::notifications::{Alert, AlertEngine};
    use crate::platform;
    use crate::providers;
    use crate::refresh::RefreshCoordinator;
    use crate::state::{AppState, FetchError, FetchOutcome, ProviderKind, HOVER_MAX_AGE_SECS};
    use crate::tray::{
        TrayUi, MENU_ID_AUTOSTART, MENU_ID_OPEN_PREFIX, MENU_ID_QUIT, MENU_ID_REFRESH,
        MENU_ID_SETTINGS,
    };

    /// Tooltip countdowns update locally on this cadence without any network.
    const TICK: Duration = Duration::from_secs(30);

    pub enum UserEvent {
        Tray(TrayIconEvent),
        Menu(MenuEvent),
        Fetched {
            kind: ProviderKind,
            result: Result<FetchOutcome, FetchError>,
        },
    }

    struct App {
        config: Config,
        state: AppState,
        coordinator: RefreshCoordinator,
        alerts: AlertEngine,
        autostart: bool,
        proxy: EventLoopProxy<UserEvent>,
        runtime: tokio::runtime::Runtime,
        last_poll: Option<Instant>,
    }

    impl App {
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

        fn spawn_fetch(&mut self, kind: ProviderKind) {
            self.state.mark_loading(kind);
            let proxy = self.proxy.clone();
            let cfg = self.config.providers.clone();
            self.runtime.spawn(async move {
                let result = match kind {
                    ProviderKind::Claude => providers::claude::fetch(&cfg.claude).await,
                    ProviderKind::Codex => providers::codex::fetch(&cfg.codex).await,
                    ProviderKind::Copilot => providers::copilot::fetch(&cfg.copilot).await,
                };
                let _ = proxy.send_event(UserEvent::Fetched { kind, result });
            });
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
                "AIAllowance",
                &format!("{label}: {threshold}% or less remaining"),
            ),
            Alert::Reset { label } => platform::show_toast(
                "AIAllowance",
                &format!("{label}: allowance reset confirmed"),
            ),
        }
    }

    pub fn run() {
        let (config, config_warning) = config::load_or_create();
        if let Some(w) = &config_warning {
            crate::eprint_line(&format!("AIAllowance: {w}"));
        }

        let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
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

        let mut app = App {
            config,
            state: AppState::default(),
            coordinator: RefreshCoordinator::default(),
            alerts: AlertEngine::default(),
            autostart: platform::autostart_enabled(),
            proxy,
            runtime,
            last_poll: None,
        };

        // Explicit states for disabled providers so the menu explains them.
        if !app.config.providers.claude.enabled {
            app.state
                .set_not_configured(ProviderKind::Claude, "disabled in config.toml");
        }
        if !app.config.providers.codex.enabled {
            app.state
                .set_not_configured(ProviderKind::Codex, "disabled in config.toml");
        }
        if !app.config.providers.copilot.enabled {
            app.state
                .set_not_configured(ProviderKind::Copilot, "disabled in config.toml");
        }

        let mut tray: Option<TrayUi> = None;

        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::WaitUntil(Instant::now() + TICK);
            match event {
                Event::NewEvents(StartCause::Init) => {
                    match TrayUi::new(&app.state, &app.config, app.autostart) {
                        Ok(t) => tray = Some(t),
                        Err(e) => {
                            crate::eprint_line(&format!("AIAllowance: {e}"));
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
                    if let Some(t) = &tray {
                        t.update(&app.state, &app.config, app.autostart);
                    }
                }
                Event::UserEvent(UserEvent::Tray(tray_event)) => {
                    if matches!(
                        tray_event,
                        TrayIconEvent::Enter { .. } | TrayIconEvent::Move { .. }
                    ) && app.refresh_stale(HOVER_MAX_AGE_SECS)
                    {
                        if let Some(t) = &tray {
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
                            if let Some(path) = config::config_path() {
                                platform::shell_open(&path.display().to_string());
                            }
                        }
                        MENU_ID_AUTOSTART => {
                            let target = !app.autostart;
                            match platform::set_autostart(target) {
                                Ok(()) => app.autostart = target,
                                Err(e) => crate::eprint_line(&format!("AIAllowance: {e}")),
                            }
                        }
                        MENU_ID_QUIT => {
                            *control_flow = ControlFlow::Exit;
                        }
                        other => {
                            if let Some(key) = other.strip_prefix(MENU_ID_OPEN_PREFIX) {
                                for kind in ProviderKind::ALL {
                                    if kind.key() == key {
                                        platform::shell_open(kind.usage_url());
                                    }
                                }
                            }
                        }
                    }
                    if let Some(t) = &tray {
                        t.update(&app.state, &app.config, app.autostart);
                    }
                }
                Event::UserEvent(UserEvent::Fetched { kind, result }) => {
                    app.handle_fetched(kind, result);
                    if let Some(t) = &tray {
                        t.update(&app.state, &app.config, app.autostart);
                    }
                }
                Event::LoopDestroyed => {
                    // Remove the tray icon on shutdown.
                    tray.take();
                }
                _ => {}
            }
        });
    }
}
