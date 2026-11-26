#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui;
use poe_craft::{
    analytic_estimates, base_influences, bases_for_class, simulate, simulate_build,
    supported_classes, AugmentMode, BaseSpec, BuildOptions, BuildPlan, Costs, Goal, OnFail,
    PlanFile, PlanStep, Pool, Requirement, Simulator, Slot, StartRarity, StepAction, StepEstimate,
};
use poe_data::{load_cache, PobData};
use poe_input::{AppConfig, Calibration, Shortcut, Win32Game, Win32GameIo, TARGET_PANEL};
use poe_item::{parse_item, Resolver};
use poe_session::{
    CraftSession, ProgressKind, SessionConfig, SessionLogger, SessionProgress, SessionReport,
};
use poe_timing::DelaySettings;

const DATA_LOCK: &str = "data/pob-lock.json";
const DATA_DIR: &str = "data/pob";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

fn updates_installable() -> bool {
    !cfg!(debug_assertions) || std::env::var_os("POE_CRAFTER_ALLOW_DEBUG_UPDATE").is_some()
}

fn main() -> eframe::Result {
    if let Some(code) = poe_update::apply_from_args(std::env::args_os()) {
        std::process::exit(code);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 720.0])
            .with_min_inner_size([860.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "PoE Crafter",
        options,
        Box::new(|cc| Ok(Box::new(PoEApp::new(cc)))),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Doctor,
    Calibrate,
    Inspect,
    Build,
    Plan,
    Run,
    Settings,
}

enum WorkerMessage {
    Doctor(Result<String, String>),
    DataProgress(poe_data::DataProgress),
    DataReady(Result<String, String>),
    Calibrated(Result<Calibration, String>),
    Inspected(Result<String, String>),
    PlanValidated(Result<String, String>),
    Simulated(Result<String, String>),
    BuildSimulated(Result<poe_craft::BuildSimulationReport, String>),
    Preview(Result<String, String>),
    Progress(SessionProgress),
    Session(Box<Result<SessionReport, String>>),
    StartRequested,
    UpdateStatus(String),
    UpdateProgress(poe_update::UpdateProgress),
    UpdateFinished(Result<String, String>),
}

struct PoEApp {
    tab: Tab,
    cache_path: String,
    calibration_path: String,
    delays_path: String,
    plan_path: String,
    log_dir: String,
    config_path: String,
    config: AppConfig,
    item_path: String,
    item_text: String,
    target: String,
    calibration: Option<Calibration>,
    doctor_output: String,
    inspect_output: String,
    plan_output: String,
    run_output: String,
    events: Vec<String>,
    status: String,
    busy: bool,
    running: bool,
    update_busy: bool,
    update_output: String,
    cancel: Option<Arc<AtomicBool>>,
    run_worker: Option<thread::JoinHandle<()>>,
    start_cancel: Option<Arc<AtomicBool>>,
    start_worker: Option<thread::JoinHandle<()>>,
    ctx: egui::Context,
    sender: Sender<WorkerMessage>,
    receiver: Receiver<WorkerMessage>,
    max_steps: u32,
    max_seconds: u64,
    max_cost: String,
    runs: u64,
    seed: u64,
    dry_run: bool,
    once: bool,
    verify_currency: bool,
    hold_currency: bool,
    builder: Option<BuilderState>,
}

impl PoEApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (sender, receiver) = channel();
        let mut app = Self {
            tab: Tab::Doctor,
            cache_path: "target/poe-data.msgpack".to_string(),
            calibration_path: Calibration::default_path().display().to_string(),
            delays_path: "delays.toml".to_string(),
            plan_path: "plans/es-boots-alt.toml".to_string(),
            log_dir: "logs".to_string(),
            config_path: "config.toml".to_string(),
            config: AppConfig::default(),
            item_path: String::new(),
            item_text: String::new(),
            target: "item".to_string(),
            calibration: None,
            doctor_output: String::new(),
            inspect_output: String::new(),
            plan_output: String::new(),
            run_output: String::new(),
            events: Vec::new(),
            status: "Ready".to_string(),
            busy: false,
            running: false,
            update_busy: false,
            update_output: String::new(),
            cancel: None,
            run_worker: None,
            start_cancel: None,
            start_worker: None,
            ctx: cc.egui_ctx.clone(),
            sender,
            receiver,
            max_steps: 10_000,
            max_seconds: 3_600,
            max_cost: String::new(),
            runs: 1_000,
            seed: 42,
            dry_run: true,
            once: false,
            verify_currency: true,
            hold_currency: true,
            builder: None,
        };
        app.reload_calibration();
        app.reload_config();
        app.restart_start_monitor();
        if app.config.update.check_on_startup {
            app.start_update_check();
        } else if !poe_data::cache_is_current(&app.cache()) {
            app.start_data_setup();
        }
        app
    }

    fn cache(&self) -> PathBuf {
        PathBuf::from(&self.cache_path)
    }

    fn plan_path(&self) -> PathBuf {
        PathBuf::from(&self.plan_path)
    }

    fn calibration_path(&self) -> PathBuf {
        PathBuf::from(&self.calibration_path)
    }

    fn delays_path(&self) -> PathBuf {
        PathBuf::from(&self.delays_path)
    }

    fn log_dir(&self) -> PathBuf {
        PathBuf::from(&self.log_dir)
    }

    fn config_path(&self) -> PathBuf {
        PathBuf::from(&self.config_path)
    }

    fn reload_config(&mut self) {
        let path = self.config_path();
        match AppConfig::load_or_default(&path) {
            Ok(config) => {
                self.config = config;
                self.status = format!("Loaded hotkey config {}", path.display());
            }
            Err(error) => {
                self.status = format!("Hotkey config error: {error}");
            }
        }
    }

    fn save_config(&mut self) {
        let path = self.config_path();
        match self.config.save(&path) {
            Ok(()) => {
                self.status = format!("Saved hotkey config {}", path.display());
                self.restart_start_monitor();
            }
            Err(error) => {
                self.status = format!("Hotkey config error: {error}");
            }
        }
    }

    fn restart_start_monitor(&mut self) {
        if let Some(cancel) = self.start_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(worker) = self.start_worker.take() {
            let _ = worker.join();
        }
        let shortcut: Shortcut = match self.config.hotkeys.parse() {
            Ok(hotkeys) => hotkeys.start,
            Err(error) => {
                self.status = format!("Start hotkey disabled: {error}");
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        self.start_cancel = Some(cancel);
        self.start_worker = Some(thread::spawn(move || {
            let mut was_down = false;
            while !worker_cancel.load(Ordering::Relaxed) {
                let down = shortcut.is_down();
                if down && !was_down {
                    let _ = sender.send(WorkerMessage::StartRequested);
                    ctx.request_repaint();
                }
                was_down = down;
                thread::sleep(Duration::from_millis(50));
            }
        }));
    }

    fn reload_calibration(&mut self) {
        let path = self.calibration_path();
        match Calibration::load(&path) {
            Ok(calibration) => {
                self.calibration = Some(calibration);
                self.status = format!("Loaded {}", path.display());
            }
            Err(error) => {
                self.calibration = None;
                self.status = format!("Calibration not loaded: {error}");
            }
        }
    }

    fn request_repaint(&self, ctx: &egui::Context) {
        if self.running || self.busy || self.update_busy {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn handle_messages(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                WorkerMessage::Doctor(result) => {
                    self.busy = false;
                    self.doctor_output = result.unwrap_or_else(|error| error);
                }
                WorkerMessage::DataProgress(progress) => {
                    self.status = format!(
                        "Data {}/{}: {}{}",
                        progress.completed,
                        progress.total,
                        progress.path,
                        if progress.downloaded { "" } else { " (reused)" }
                    );
                }
                WorkerMessage::DataReady(result) => {
                    self.busy = false;
                    match result {
                        Ok(message) => {
                            self.status = message.clone();
                            self.doctor_output = message;
                        }
                        Err(error) => {
                            self.status = format!("Data setup failed: {error}");
                            self.doctor_output = error;
                        }
                    }
                }
                WorkerMessage::Calibrated(result) => {
                    self.busy = false;
                    match result {
                        Ok(calibration) => {
                            self.status = format!(
                                "Saved calibration to {}",
                                self.calibration_path().display()
                            );
                            self.calibration = Some(calibration);
                        }
                        Err(error) => {
                            self.status = format!("Calibration failed: {error}");
                        }
                    }
                }
                WorkerMessage::Inspected(result) => {
                    self.busy = false;
                    self.inspect_output = result.unwrap_or_else(|error| error);
                }
                WorkerMessage::PlanValidated(result) => {
                    self.busy = false;
                    self.plan_output = result.unwrap_or_else(|error| error);
                }
                WorkerMessage::Simulated(result) => {
                    self.busy = false;
                    self.plan_output = result.unwrap_or_else(|error| error);
                }
                WorkerMessage::BuildSimulated(result) => {
                    self.busy = false;
                    if let Some(builder) = self.builder.as_mut() {
                        builder.apply_simulation(result);
                    }
                }
                WorkerMessage::Preview(result) => {
                    self.busy = false;
                    self.running = false;
                    self.run_output = result.unwrap_or_else(|error| error);
                    self.join_worker();
                }
                WorkerMessage::Progress(progress) => {
                    self.events.push(format_progress(&progress));
                    if self.events.len() > 200 {
                        self.events.remove(0);
                    }
                }
                WorkerMessage::StartRequested => {
                    if self.running || self.busy || self.update_busy {
                        self.status =
                            "Start hotkey ignored while another action is running.".to_string();
                    } else {
                        self.start_session();
                    }
                }
                WorkerMessage::UpdateStatus(message) => {
                    self.status = message.clone();
                    self.update_output = message;
                }
                WorkerMessage::UpdateProgress(progress) => {
                    let percent = progress.total.map(|total| {
                        if total == 0 {
                            100.0
                        } else {
                            progress.downloaded as f64 / total as f64 * 100.0
                        }
                    });
                    let message = match percent {
                        Some(percent) => format!("Downloading update... {percent:.0}%"),
                        None => format!("Downloading update... {} KiB", progress.downloaded / 1024),
                    };
                    self.status = message.clone();
                    self.update_output = message;
                }
                WorkerMessage::UpdateFinished(result) => {
                    self.update_busy = false;
                    match result {
                        Ok(message) => {
                            self.status = message.clone();
                            self.update_output = message;
                        }
                        Err(error) => {
                            self.status = format!("Update check failed: {error}");
                            self.update_output = format!("Update check failed: {error}");
                        }
                    }
                    if !poe_data::cache_is_current(&self.cache()) && !self.busy {
                        self.start_data_setup();
                    }
                }
                WorkerMessage::Session(result) => {
                    self.running = false;
                    self.busy = false;
                    match *result {
                        Ok(report) => {
                            self.run_output = format_report(&report);
                            self.events
                                .push(format!("Session ended: {:?}", report.stop_reason));
                        }
                        Err(error) => {
                            self.run_output = error.clone();
                            self.events.push(format!("Session stopped: {error}"));
                        }
                    }
                    self.join_worker();
                }
            }
        }
    }

    fn join_worker(&mut self) {
        if let Some(handle) = self.run_worker.take() {
            let _ = handle.join();
        }
    }

    fn start_update_check(&mut self) {
        if self.update_busy || self.running || self.busy {
            return;
        }
        self.update_busy = true;
        self.status = format!("Checking for updates (current {CURRENT_VERSION})...");
        self.update_output = self.status.clone();
        let sender = self.sender.clone();
        thread::spawn(move || {
            let release = match poe_update::check_for_update(CURRENT_VERSION) {
                Ok(release) => release,
                Err(error) => {
                    let _ = sender.send(WorkerMessage::UpdateFinished(Err(error.to_string())));
                    return;
                }
            };
            let Some(release) = release else {
                let _ = sender.send(WorkerMessage::UpdateFinished(Ok(format!(
                    "Version {CURRENT_VERSION} is up to date."
                ))));
                return;
            };
            if !updates_installable() {
                let _ = sender.send(WorkerMessage::UpdateFinished(Ok(format!(
                    "Version {} is available. Debug builds do not auto-install updates.",
                    release.version
                ))));
                return;
            }
            let _ = sender.send(WorkerMessage::UpdateStatus(format!(
                "Update {} available; downloading...",
                release.version
            )));
            let staging = poe_update::staging_dir(&release.version);
            let progress_sender = sender.clone();
            let mut progress = |progress: poe_update::UpdateProgress| {
                let _ = progress_sender.send(WorkerMessage::UpdateProgress(progress));
            };
            let staged =
                match poe_update::download_and_stage(&release, &staging, Some(&mut progress)) {
                    Ok(staged) => staged,
                    Err(error) => {
                        let _ = sender.send(WorkerMessage::UpdateFinished(Err(error.to_string())));
                        return;
                    }
                };
            let target = match std::env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf))
            {
                Some(target) => target,
                None => {
                    let _ = sender.send(WorkerMessage::UpdateFinished(Err(
                        "cannot locate the current executable directory".to_string(),
                    )));
                    return;
                }
            };
            let _ = sender.send(WorkerMessage::UpdateStatus(
                "Update downloaded; restarting...".to_string(),
            ));
            thread::sleep(Duration::from_millis(300));
            let working_dir = std::env::current_dir().ok();
            if let Err(error) =
                poe_update::apply_and_restart(&staged, &target, working_dir.as_deref())
            {
                let _ = sender.send(WorkerMessage::UpdateFinished(Err(error.to_string())));
                return;
            }
            std::process::exit(0);
        });
    }

    fn start_doctor(&mut self) {
        self.busy = true;
        self.status = "Checking game...".to_string();
        let sender = self.sender.clone();
        let cache = self.cache();
        let calibration = self.calibration_path();
        let config = self.config_path();
        thread::spawn(move || {
            let result = doctor_report(&cache, &calibration, &config);
            let _ = sender.send(WorkerMessage::Doctor(result));
        });
    }

    fn start_data_setup(&mut self) {
        if poe_data::cache_is_current(&self.cache()) {
            self.status = "Data cache is already present.".to_string();
            return;
        }
        self.busy = true;
        self.status = "Data cache missing or outdated; fetching pinned data...".to_string();
        let sender = self.sender.clone();
        let progress_sender = sender.clone();
        let cache = self.cache();
        thread::spawn(move || {
            let mut progress = |progress: poe_data::DataProgress| {
                let _ = progress_sender.send(WorkerMessage::DataProgress(progress));
            };
            let result = poe_data::ensure_cache(
                Path::new(DATA_LOCK),
                Path::new(DATA_DIR),
                &cache,
                Some(&mut progress),
            )
            .map(|outcome| {
                format!(
                    "Data cache ready: {} files ({} downloaded, {} reused).",
                    outcome.files, outcome.downloaded, outcome.skipped
                )
            })
            .map_err(|error| error.to_string());
            let _ = sender.send(WorkerMessage::DataReady(result));
        });
    }

    fn start_calibration(&mut self) {
        self.busy = true;
        self.status = format!(
            "Capturing {}: focus the game and press {}",
            self.target, self.config.hotkeys.calibrate
        );
        let sender = self.sender.clone();
        let path = self.calibration_path();
        let config_path = self.config_path();
        let target = self.target.clone();
        thread::spawn(move || {
            let result = capture_calibration(&path, &target, &config_path);
            let _ = sender.send(WorkerMessage::Calibrated(result));
        });
    }

    fn start_inspect(&mut self) {
        let text = if self.item_text.trim().is_empty() {
            let path = PathBuf::from(&self.item_path);
            match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) => {
                    self.inspect_output = format!("Cannot read {}: {error}", path.display());
                    return;
                }
            }
        } else {
            self.item_text.clone()
        };
        self.busy = true;
        self.inspect_output = "Resolving...".to_string();
        let sender = self.sender.clone();
        let cache = self.cache();
        thread::spawn(move || {
            let result = inspect_text(&cache, &text);
            let _ = sender.send(WorkerMessage::Inspected(result));
        });
    }

    fn start_validate(&mut self) {
        self.busy = true;
        self.plan_output = "Validating...".to_string();
        let sender = self.sender.clone();
        let cache = self.cache();
        let plan = self.plan_path();
        thread::spawn(move || {
            let result = validate_plan(&cache, &plan);
            let _ = sender.send(WorkerMessage::PlanValidated(result));
        });
    }

    fn start_simulation(&mut self) {
        let runs = self.runs.max(1);
        let seed = self.seed;
        self.busy = true;
        self.plan_output = format!("Simulating {runs} runs...");
        let sender = self.sender.clone();
        let cache = self.cache();
        let plan = self.plan_path();
        thread::spawn(move || {
            let result = run_simulation(&cache, &plan, runs, seed);
            let _ = sender.send(WorkerMessage::Simulated(result));
        });
    }

    fn start_session(&mut self) {
        if self.running || self.update_busy {
            if self.update_busy {
                self.status = "Wait for the update check to finish.".to_string();
            }
            return;
        }
        let cache = self.cache();
        let plan = self.plan_path();
        let calibration = self.calibration_path();
        let delays = self.delays_path();
        let log_dir = self.log_dir();
        let config = self.config_path();
        let dry_run = self.dry_run;
        let once = self.once;
        let max_steps = if once || dry_run {
            1
        } else {
            self.max_steps.max(1)
        };
        let max_seconds = self.max_seconds.max(1);
        let max_cost = match self.max_cost.trim() {
            "" => None,
            value => match value.parse::<f64>() {
                Ok(value) if value > 0.0 => Some(value),
                _ => {
                    self.status = "Max cost must be a positive number or empty".to_string();
                    return;
                }
            },
        };
        let verify_currency = self.verify_currency;
        let hold_currency = self.hold_currency;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.running = true;
        self.busy = true;
        self.events.clear();
        self.run_output = if dry_run {
            "Reading the current item...".to_string()
        } else {
            "Starting real session...".to_string()
        };
        self.status = if dry_run {
            "Dry run".to_string()
        } else {
            format!("Real session running. {} stops.", self.config.hotkeys.stop)
        };
        let sender = self.sender.clone();
        let handle = thread::spawn(move || {
            let result = run_session(SessionRequest {
                cache,
                plan,
                calibration,
                delays,
                log_dir,
                config,
                dry_run,
                max_steps,
                max_seconds,
                max_cost,
                verify_currency,
                hold_currency,
                cancel,
                sender: sender.clone(),
            });
            match result {
                SessionResult::Preview(result) => {
                    let _ = sender.send(WorkerMessage::Preview(result));
                }
                SessionResult::Ran(result) => {
                    let _ = sender.send(WorkerMessage::Session(result));
                }
            }
        });
        self.run_worker = Some(handle);
    }

    fn stop_session(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.status = "Stop requested. Releasing held currency...".to_string();
    }
}

impl eframe::App for PoEApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_messages();

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("PoE Crafter");
                ui.separator();
                ui.label(&self.status);
                if self.running || self.busy || self.update_busy {
                    ui.spinner();
                }
            });
            ui.add_space(4.0);
        });

        egui::SidePanel::left("nav")
            .default_width(150.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.selectable_value(&mut self.tab, Tab::Doctor, "Doctor");
                ui.selectable_value(&mut self.tab, Tab::Calibrate, "Calibrate");
                ui.selectable_value(&mut self.tab, Tab::Inspect, "Inspect");
                ui.selectable_value(&mut self.tab, Tab::Build, "Build");
                ui.selectable_value(&mut self.tab, Tab::Plan, "Plan");
                ui.selectable_value(&mut self.tab, Tab::Run, "Run");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
                ui.separator();
                ui.small(format!("Emergency stop: {}", self.config.hotkeys.stop));
                if let Some(calibration) = &self.calibration {
                    ui.separator();
                    ui.small(if calibration.panel.is_some() {
                        "Panel guard: on"
                    } else {
                        "Panel guard: off"
                    });
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Doctor => self.doctor_tab(ui),
            Tab::Calibrate => self.calibrate_tab(ui),
            Tab::Inspect => self.inspect_tab(ui),
            Tab::Build => self.build_tab(ui),
            Tab::Plan => self.plan_tab(ui),
            Tab::Run => self.run_tab(ui),
            Tab::Settings => self.settings_tab(ui),
        });

        self.request_repaint(ctx);
    }
}

impl PoEApp {
    fn doctor_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Doctor");
        ui.label("Checks the game window, data cache, calibration, and clipboard.");
        ui.horizontal(|ui| {
            if ui.button("Refresh").clicked() {
                self.start_doctor();
            }
            if ui
                .add_enabled(
                    !self.busy && !self.update_busy && !poe_data::cache_is_current(&self.cache()),
                    egui::Button::new("Download data"),
                )
                .clicked()
            {
                self.start_data_setup();
            }
            if ui
                .add_enabled(
                    !self.update_busy && !self.busy && !self.running,
                    egui::Button::new("Check for updates"),
                )
                .clicked()
            {
                self.start_update_check();
            }
            if ui.button("Reload calibration").clicked() {
                self.reload_calibration();
            }
        });
        ui.separator();
        path_row(
            ui,
            "Data cache",
            &mut self.cache_path,
            PathKind::File,
            "target",
        );
        path_row(
            ui,
            "Calibration",
            &mut self.calibration_path,
            PathKind::File,
            ".",
        );
        ui.separator();
        ui.label(format!(
            "Hotkeys: start {} | stop {} | calibrate {} | copy {}",
            self.config.hotkeys.start,
            self.config.hotkeys.stop,
            self.config.hotkeys.calibrate,
            self.config.hotkeys.copy
        ));
        ui.separator();
        if let Some(calibration) = &self.calibration {
            ui.label(calibration_summary(calibration));
        } else {
            ui.label("No calibration loaded.");
        }
        ui.separator();
        output(ui, &self.doctor_output);
    }

    fn calibrate_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Calibration");
        ui.label(format!(
            "Focus Path of Exile, hover the requested slot, then press {}. \
             A panel guard is an optional pixel probe on a stable part of the open stash/inventory.",
            self.config.hotkeys.calibrate
        ));
        path_row(
            ui,
            "Calibration",
            &mut self.calibration_path,
            PathKind::File,
            ".",
        );
        ui.horizontal(|ui| {
            egui::ComboBox::from_label("Target")
                .selected_text(&self.target)
                .show_ui(ui, |ui| {
                    for target in Calibration::capturable_targets() {
                        ui.selectable_value(&mut self.target, target.to_string(), *target);
                    }
                });
            if ui
                .add_enabled(
                    !self.busy && !self.update_busy,
                    egui::Button::new("Capture"),
                )
                .clicked()
            {
                self.start_calibration();
            }
            if ui.button("Reload").clicked() {
                self.reload_calibration();
            }
        });
        if self.busy {
            ui.label("Waiting for F8...");
        }
        ui.separator();
        if let Some(calibration) = &self.calibration {
            ui.label(calibration_summary(calibration));
        }
    }

    fn inspect_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Item Inspector");
        ui.label("Load a captured item text file or paste item text.");
        path_row(
            ui,
            "Item file",
            &mut self.item_path,
            PathKind::File,
            "samples/items",
        );
        ui.horizontal(|ui| {
            if ui.button("Load file").clicked() {
                if let Ok(text) = std::fs::read_to_string(PathBuf::from(&self.item_path)) {
                    self.item_text = text;
                } else {
                    self.status = format!("Cannot read {}", self.item_path);
                }
            }
            if ui.button("Inspect").clicked() {
                self.start_inspect();
            }
        });
        ui.separator();
        ui.add(
            egui::TextEdit::multiline(&mut self.item_text)
                .desired_rows(10)
                .desired_width(f32::INFINITY),
        );
        ui.separator();
        output(ui, &self.inspect_output);
    }

    fn build_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Build Plan");
        ui.label(
            "Author a step-based schema 2 plan. Validate as you edit, simulate probabilities, \
             then save and use the Plan/Run tabs.",
        );
        if self.builder.is_none() {
            if !poe_data::cache_is_current(&self.cache()) {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "The data cache is missing or outdated. Download it before building a plan.",
                );
                if ui.button("Download data").clicked() {
                    self.start_data_setup();
                }
                return;
            }
            match load_ui_data(&self.cache()) {
                Ok(data) => {
                    self.builder = Some(BuilderState::new(Arc::new(data)));
                }
                Err(error) => {
                    ui.colored_label(
                        egui::Color32::RED,
                        format!("Cannot load data cache: {error}"),
                    );
                    return;
                }
            }
        }
        path_row(ui, "Plan", &mut self.plan_path, PathKind::File, "plans");
        let plan_path = self.plan_path.clone();
        let mut load_plan = false;
        ui.horizontal(|ui| {
            if ui.button("Load saved schema 2 plan").clicked() {
                load_plan = true;
            }
            if ui.button("Reload data cache").clicked() {
                self.builder = None;
            }
        });
        if load_plan {
            match PlanFile::load(Path::new(&plan_path)) {
                Ok(PlanFile::Steps(plan)) => {
                    if let Some(builder) = self.builder.as_mut() {
                        builder.load_plan(plan);
                    }
                }
                Ok(PlanFile::Legacy(_)) => {
                    self.status =
                        "Legacy schema 1 plans cannot be edited in the builder.".to_string();
                }
                Err(error) => {
                    self.status = format!("Cannot load plan: {error}");
                }
            }
        }
        let mut simulate_request = None;
        if let Some(builder) = self.builder.as_mut() {
            builder.ui(ui, &plan_path);
            if builder.simulate_requested {
                builder.simulate_requested = false;
                simulate_request = Some((builder.data.clone(), builder.plan.clone()));
            }
        }
        if let Some((data, plan)) = simulate_request {
            self.busy = true;
            self.status = "Simulating build plan...".to_string();
            let sender = self.sender.clone();
            let ctx = self.ctx.clone();
            thread::spawn(move || {
                let result = simulate_build(&data, plan, 1_000, 42, None, None)
                    .map_err(|error| error.to_string());
                let _ = sender.send(WorkerMessage::BuildSimulated(result));
                ctx.request_repaint();
            });
        }
    }

    fn plan_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Plan");
        path_row(
            ui,
            "Data cache",
            &mut self.cache_path,
            PathKind::File,
            "target",
        );
        path_row(
            ui,
            "Plan file",
            &mut self.plan_path,
            PathKind::File,
            "plans",
        );
        ui.horizontal(|ui| {
            if ui.button("Validate").clicked() {
                self.start_validate();
            }
            ui.label("Runs");
            ui.add(egui::DragValue::new(&mut self.runs).range(1..=1_000_000));
            ui.label("Seed");
            ui.add(egui::DragValue::new(&mut self.seed));
            if ui.button("Simulate").clicked() {
                self.start_simulation();
            }
        });
        ui.separator();
        output(ui, &self.plan_output);
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.label("Global keyboard shortcuts. Use names such as F9, F12, Ctrl+C, or Shift+F8.");
        path_row(
            ui,
            "Config file",
            &mut self.config_path,
            PathKind::File,
            ".",
        );
        hotkey_row(ui, "Start", &mut self.config.hotkeys.start);
        hotkey_row(ui, "Stop", &mut self.config.hotkeys.stop);
        hotkey_row(ui, "Calibrate", &mut self.config.hotkeys.calibrate);
        hotkey_row(ui, "Copy item/currency", &mut self.config.hotkeys.copy);
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                self.save_config();
            }
            if ui.button("Reload").clicked() {
                self.reload_config();
                self.restart_start_monitor();
            }
            if ui.button("Defaults").clicked() {
                self.config.hotkeys = Default::default();
                self.status = "Default hotkeys restored (not saved).".to_string();
                self.restart_start_monitor();
            }
        });
        ui.separator();
        match self.config.hotkeys.parse() {
            Ok(_) => {
                ui.label(format!(
                    "Start: {} | Stop: {} | Calibrate: {} | Copy: {}",
                    self.config.hotkeys.start,
                    self.config.hotkeys.stop,
                    self.config.hotkeys.calibrate,
                    self.config.hotkeys.copy
                ));
            }
            Err(error) => {
                ui.colored_label(egui::Color32::RED, error.to_string());
            }
        }
        ui.label(
            "Start works while the UI is in the background. Keep the game focused when using stop, \
             calibrate, and copy keys so the game receives them.",
        );
        ui.separator();
        ui.label(format!("Application version: {CURRENT_VERSION}"));
        ui.checkbox(
            &mut self.config.update.check_on_startup,
            "Check for updates on startup",
        );
        if ui
            .add_enabled(
                !self.update_busy && !self.busy && !self.running,
                egui::Button::new("Check for updates"),
            )
            .clicked()
        {
            self.start_update_check();
        }
        if !self.update_output.is_empty() {
            ui.separator();
            output(ui, &self.update_output);
        }
    }

    fn run_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Craft");
        ui.label(format!(
            "Dry run reads the item and recommends an action. A real run sends mouse clicks; \
             keep the stash/inventory open and use {} to stop.",
            self.config.hotkeys.stop
        ));
        path_row(
            ui,
            "Data cache",
            &mut self.cache_path,
            PathKind::File,
            "target",
        );
        path_row(ui, "Plan", &mut self.plan_path, PathKind::File, "plans");
        path_row(
            ui,
            "Calibration",
            &mut self.calibration_path,
            PathKind::File,
            ".",
        );
        path_row(ui, "Delays", &mut self.delays_path, PathKind::File, ".");
        path_row(
            ui,
            "Log directory",
            &mut self.log_dir,
            PathKind::Directory,
            "logs",
        );
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.dry_run, "Dry run");
            ui.checkbox(&mut self.once, "One step");
            ui.checkbox(&mut self.verify_currency, "Verify currency");
            ui.checkbox(&mut self.hold_currency, "Hold currency");
        });
        ui.horizontal(|ui| {
            ui.label("Max steps");
            ui.add_enabled(
                !self.once && !self.dry_run,
                egui::DragValue::new(&mut self.max_steps).range(1..=1_000_000),
            );
            ui.label("Max seconds");
            ui.add(egui::DragValue::new(&mut self.max_seconds).range(1..=86_400));
            ui.label("Max cost");
            ui.add(
                egui::TextEdit::singleline(&mut self.max_cost)
                    .hint_text("optional")
                    .desired_width(90.0),
            );
        });
        ui.horizontal(|ui| {
            let label = if self.dry_run { "Preview" } else { "Start run" };
            if ui
                .add_enabled(!self.running && !self.update_busy, egui::Button::new(label))
                .clicked()
            {
                self.start_session();
            }
            if ui
                .add_enabled(self.running, egui::Button::new("Stop"))
                .clicked()
            {
                self.stop_session();
            }
            if self.running {
                ui.spinner();
            }
        });
        if let Some(calibration) = &self.calibration {
            if calibration.panel.is_none() {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Panel guard is off. Calibrate `panel` on the open stash/inventory for a pre-click guard.",
                );
            }
        }
        ui.separator();
        ui.columns(2, |columns| {
            columns[0].heading("Live events");
            egui::ScrollArea::vertical()
                .id_salt("events")
                .max_height(280.0)
                .show(&mut columns[0], |ui| {
                    for event in self.events.iter().rev() {
                        ui.monospace(event);
                    }
                });
            columns[1].heading("Result");
            output(&mut columns[1], &self.run_output);
        });
    }
}

impl Drop for PoEApp {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(cancel) = &self.start_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(handle) = self.start_worker.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.run_worker.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum PathKind {
    File,
    Directory,
}

fn hotkey_row(ui: &mut egui::Ui, label: &str, value: &mut String) {
    egui::Grid::new(label)
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(label);
            ui.add(
                egui::TextEdit::singleline(value)
                    .desired_width(160.0)
                    .font(egui::TextStyle::Monospace),
            );
            ui.end_row();
        });
}

fn path_row(ui: &mut egui::Ui, label: &str, value: &mut String, kind: PathKind, default_dir: &str) {
    egui::Grid::new(label)
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(label);
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 78.0).max(120.0);
                ui.add(
                    egui::TextEdit::singleline(value)
                        .desired_width(width)
                        .font(egui::TextStyle::Monospace),
                );
                if ui.button("Browse").clicked() {
                    if let Some(selected) = pick_path(value, kind, default_dir) {
                        *value = selected.display().to_string();
                    }
                }
            });
            ui.end_row();
        });
}

fn pick_path(current: &str, kind: PathKind, default_dir: &str) -> Option<PathBuf> {
    let current = Path::new(current);
    let mut dialog = rfd::FileDialog::new();
    let default = if default_dir == "." {
        std::env::current_dir().ok()
    } else {
        Some(PathBuf::from(default_dir))
    };
    let start = default.filter(|directory| directory.is_dir()).or_else(|| {
        current
            .parent()
            .filter(|directory| !directory.as_os_str().is_empty())
            .map(Path::to_path_buf)
    });
    if let Some(directory) = start {
        dialog = dialog.set_directory(directory);
    }
    match kind {
        PathKind::Directory => dialog.pick_folder(),
        PathKind::File => {
            if let Some(name) = current.file_name().and_then(|name| name.to_str()) {
                let extension = current
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or_default();
                if !extension.is_empty() {
                    dialog = dialog.add_filter(extension, &[extension]);
                }
                dialog = dialog.set_file_name(name);
            }
            dialog.pick_file()
        }
    }
}

fn output(ui: &mut egui::Ui, text: &str) {
    egui::ScrollArea::both()
        .id_salt("output")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.monospace(text);
        });
}

fn calibration_summary(calibration: &Calibration) -> String {
    let mut out = String::new();
    out.push_str(&format!("Game title: {}\n", calibration.game_title));
    for target in Calibration::all_targets() {
        match calibration.point(target) {
            Some(point) => {
                out.push_str(&format!("{target:<10} ({:.4}, {:.4})\n", point.x, point.y))
            }
            None => out.push_str(&format!("{target:<10} missing\n")),
        }
    }
    match calibration.panel {
        Some(panel) => out.push_str(&format!(
            "{TARGET_PANEL:<10} ({:.4}, {:.4}) {:?} +/- {}\n",
            panel.point.x, panel.point.y, panel.color, panel.tolerance
        )),
        None => out.push_str(&format!("{TARGET_PANEL:<10} missing (optional)\n")),
    }
    out
}

fn format_progress(progress: &SessionProgress) -> String {
    match progress.kind {
        ProgressKind::Started => format!(
            "started at {:.1}s; goal {}",
            progress.elapsed_seconds,
            evaluation_summary(progress.evaluation.as_ref())
        ),
        ProgressKind::Applied => format!(
            "step {} | action {} | {} | {:.1}s | {}",
            progress.step,
            progress.currency_actions,
            progress
                .currency
                .map(|currency| currency.label().to_string())
                .unwrap_or_else(|| "?".to_string()),
            progress.elapsed_seconds,
            evaluation_summary(progress.evaluation.as_ref())
        ),
    }
}

fn evaluation_summary(evaluation: Option<&poe_craft::GoalEvaluation>) -> String {
    match evaluation {
        Some(evaluation) => format!(
            "score {}, {}/{} required, {}",
            evaluation.score,
            evaluation.required_matched,
            evaluation.required_total,
            if evaluation.satisfied {
                "satisfied"
            } else {
                "not satisfied"
            }
        ),
        None => "-".to_string(),
    }
}

fn doctor_report(cache: &Path, calibration: &Path, config_path: &Path) -> Result<String, String> {
    let mut out = String::new();
    match Win32Game::find("Path of Exile") {
        Ok(game) => match game.window_info() {
            Ok(info) => out.push_str(&format!(
                "Window: {}\nClient: {}x{} at {},{} (foreground: {})\n",
                info.title,
                info.client_width,
                info.client_height,
                info.origin_x,
                info.origin_y,
                info.foreground
            )),
            Err(error) => out.push_str(&format!("Window: {error}\n")),
        },
        Err(error) => out.push_str(&format!("Game: {error}\n")),
    }
    match load_cache(cache) {
        Ok(data) => out.push_str(&format!(
            "Data: {} mods, {} bases, {} trade stats, {} essences\n",
            data.mods.len(),
            data.bases.len(),
            data.trade_stats.len(),
            data.essences.len()
        )),
        Err(error) => out.push_str(&format!("Data: {error}\n")),
    }
    if calibration.exists() {
        match Calibration::load(calibration) {
            Ok(calibration) => {
                out.push_str(&format!(
                    "Calibration: {}\n",
                    calibration_summary(&calibration)
                ));
                let missing = calibration.missing(Calibration::all_targets());
                if !missing.is_empty() {
                    out.push_str(&format!("Missing: {}\n", missing.join(", ")));
                }
            }
            Err(error) => out.push_str(&format!("Calibration error: {error}\n")),
        }
    } else {
        out.push_str(&format!(
            "Calibration: missing ({})\n",
            calibration.display()
        ));
    }
    match AppConfig::load_or_default(config_path) {
        Ok(config) => {
            out.push_str(&format!(
                "Hotkeys: start {}, stop {}, calibrate {}, copy {}\n",
                config.hotkeys.start,
                config.hotkeys.stop,
                config.hotkeys.calibrate,
                config.hotkeys.copy
            ));
            out.push_str(&format!("Emergency stop: {}\n", config.hotkeys.stop));
        }
        Err(error) => out.push_str(&format!("Hotkey config: {error}\n")),
    }
    Ok(out)
}

fn capture_calibration(
    path: &Path,
    target: &str,
    config_path: &Path,
) -> Result<Calibration, String> {
    let mut calibration = if path.exists() {
        Calibration::load(path).map_err(|error| error.to_string())?
    } else {
        Calibration::empty()
    };
    let config = AppConfig::load_or_default(config_path).map_err(|error| error.to_string())?;
    let hotkeys = config.hotkeys.parse().map_err(|error| error.to_string())?;
    let mut game = Win32Game::find(&calibration.game_title).map_err(|error| error.to_string())?;
    game.apply_hotkeys(hotkeys);
    let info = game.window_info().map_err(|error| error.to_string())?;
    calibration.set_capture_size(info.client_width, info.client_height);
    game.focus().map_err(|error| error.to_string())?;
    let point = game
        .capture_point(Duration::from_secs(20))
        .map_err(|error| error.to_string())?;
    if target == TARGET_PANEL {
        let color = game
            .sample_color(point)
            .map_err(|error| error.to_string())?;
        calibration
            .set_panel(point, color)
            .map_err(|error| error.to_string())?;
    } else {
        calibration
            .set_target(target, point)
            .map_err(|error| error.to_string())?;
    }
    calibration.save(path).map_err(|error| error.to_string())?;
    Ok(calibration)
}

fn inspect_text(cache: &Path, text: &str) -> Result<String, String> {
    if text.trim().is_empty() {
        return Err("No item text provided.".to_string());
    }
    let data = load_ui_data(cache)?;
    let parsed = parse_item(text);
    let resolver = Resolver::new(&data);
    let resolved = resolver.resolve_item(&parsed);
    let mut out = String::new();
    out.push_str(&format!(
        "{} | {:?} | ilvl {}\n",
        parsed.item_class.as_deref().unwrap_or("?"),
        parsed.rarity.unwrap_or(poe_item::Rarity::Other),
        parsed
            .item_level
            .map(|level| level.to_string())
            .unwrap_or_else(|| "?".to_string())
    ));
    if let Some(base) = &parsed.base_type {
        out.push_str(&format!("Base: {base}\n"));
    }
    out.push_str(&format!("Tags: {}\n\n", resolved.base_tags.join(", ")));
    for group in &resolved.groups {
        out.push_str(&format!("[{:>2}] {:?}\n", group.index, group.kind));
        for line in &group.lines {
            out.push_str(&format!("     {}\n", line.text));
        }
        match &group.matched {
            Some(matched) => out.push_str(&format!(
                "     -> {} | {:?} | tier {:?} | group {}\n",
                matched.mod_id, matched.mod_type, matched.tier, matched.group
            )),
            None => out.push_str(&format!(
                "     -> unresolved: {}\n",
                group.note.as_deref().unwrap_or("no match")
            )),
        }
    }
    Ok(out)
}

struct BuilderState {
    data: Arc<PobData>,
    plan: BuildPlan,
    class: String,
    base_name: String,
    base_query: String,
    mod_query: String,
    mod_slot: Slot,
    mod_weighted: bool,
    active_step: usize,
    pool: Option<Pool>,
    estimates: Vec<Option<StepEstimate>>,
    output: String,
    base_key: String,
    simulate_requested: bool,
    simulating: bool,
}

impl BuilderState {
    fn new(data: Arc<PobData>) -> Self {
        let classes = supported_classes(&data);
        let class = classes
            .first()
            .cloned()
            .unwrap_or_else(|| "Ring".to_string());
        let bases = bases_for_class(&data, &class);
        let base_name = bases
            .first()
            .map(|base| base.name.clone())
            .unwrap_or_default();
        let plan = BuildPlan {
            schema: 2,
            name: "New plan".to_string(),
            start: StartRarity::Normal,
            base: BaseSpec {
                item_class: Some(class.clone()),
                base_names: vec![base_name.clone()],
                tags: vec![],
                item_level: 86,
                unrestricted: false,
                influences: vec![],
            },
            options: BuildOptions::default(),
            steps: vec![PlanStep {
                name: "Step 1".to_string(),
                action: StepAction::Alt,
                goal: Some(default_goal()),
                on_fail: OnFail::Restart,
                max_attempts: Some(50_000),
                augment: AugmentMode::IfNeeded,
            }],
            costs: Costs::default(),
        };
        let base_key = base_key(&plan);
        let mut state = Self {
            data,
            plan,
            class,
            base_name,
            base_query: String::new(),
            mod_query: String::new(),
            mod_slot: Slot::Prefix,
            mod_weighted: false,
            active_step: 0,
            pool: None,
            estimates: Vec::new(),
            output: "Set a base, add steps, and add mods to each step.".to_string(),
            base_key,
            simulate_requested: false,
            simulating: false,
        };
        state.rebuild_pool();
        state
    }

    fn load_plan(&mut self, plan: BuildPlan) {
        self.class = plan
            .base
            .item_class
            .clone()
            .unwrap_or_else(|| self.class.clone());
        self.base_name = plan
            .base
            .base_names
            .first()
            .cloned()
            .unwrap_or_else(|| self.base_name.clone());
        self.base_query.clear();
        self.active_step = 0;
        self.plan = plan;
        self.base_key = base_key(&self.plan);
        self.output = "Loaded schema 2 plan.".to_string();
        self.rebuild_pool();
    }

    fn rebuild_pool(&mut self) {
        match self.plan.pool(&self.data) {
            Ok(pool) => {
                self.estimates = analytic_estimates(&self.plan, &pool);
                self.pool = Some(pool);
            }
            Err(error) => {
                self.pool = None;
                self.estimates.clear();
                self.output = format!("Pool error: {error}");
            }
        }
    }

    fn sync_base(&mut self) {
        self.plan.base.item_class = Some(self.class.clone());
        self.plan.base.base_names = if self.base_name.is_empty() {
            vec![]
        } else {
            vec![self.base_name.clone()]
        };
        let supported = bases_for_class(&self.data, &self.class)
            .into_iter()
            .find(|base| base.name == self.base_name)
            .map(base_influences)
            .unwrap_or_default();
        self.plan
            .base
            .influences
            .retain(|influence| supported.contains(influence));
        let key = base_key(&self.plan);
        if key != self.base_key {
            self.base_key = key;
            self.rebuild_pool();
        }
    }

    fn validate(&mut self) {
        self.sync_base();
        match self.plan.validate(&self.data) {
            Ok(summary) => {
                self.rebuild_pool();
                self.output = format!(
                    "Valid: {} | pool {} prefixes / {} suffixes | {} steps",
                    summary.base, summary.prefixes, summary.suffixes, summary.steps
                );
            }
            Err(error) => {
                self.output = format!("Invalid plan: {error}");
            }
        }
    }
    fn apply_simulation(&mut self, result: Result<poe_craft::BuildSimulationReport, String>) {
        self.simulating = false;
        match result {
            Ok(report) => {
                let previous = self.estimates.clone();
                let mut warnings = String::new();
                for (index, step) in report.steps.iter().enumerate() {
                    if let (Some(mc), Some(Some(analytic))) = (step.p_success, previous.get(index))
                    {
                        if let Some(analytic) = analytic.p_success {
                            if (mc - analytic).abs() > 0.02 {
                                warnings.push_str(&format!(
                                    "\nWarning: step {} analytic {:.4}% differs from simulation {:.4}%.",
                                    index + 1,
                                    analytic * 100.0,
                                    mc * 100.0
                                ));
                            }
                        }
                    }
                }
                self.estimates = report.steps.iter().cloned().map(Some).collect();
                self.output = format!(
                    "Simulation: success {:.2}% | mean attempts {:.2} | mean cost {:.3}\n{}{}",
                    report.success_rate * 100.0,
                    report.mean_attempts,
                    report.mean_cost,
                    report
                        .steps
                        .iter()
                        .map(|step| {
                            format!(
                                "  {}: p={} expected {} confidence {}",
                                step.step_index + 1,
                                step.p_success
                                    .map(|value| format!("{:.4}%", value * 100.0))
                                    .unwrap_or_else(|| "-".to_string()),
                                step.expected_attempts
                                    .map(|value| format!("{value:.2}"))
                                    .unwrap_or_else(|| "-".to_string()),
                                step.confidence_attempts
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "-".to_string())
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                    warnings
                );
            }
            Err(error) => self.output = format!("Simulation failed: {error}"),
        }
    }

    fn save(&mut self, plan_path: &str) {
        self.validate();
        if self.output.starts_with("Invalid") || self.output.starts_with("Pool error") {
            return;
        }
        let text = match self.plan.to_toml() {
            Ok(text) => text,
            Err(error) => {
                self.output = format!("Cannot serialize plan: {error}");
                return;
            }
        };
        let path = Path::new(plan_path);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    self.output = format!("Cannot create {}: {error}", parent.display());
                    return;
                }
            }
        }
        let temp = path.with_extension("toml.tmp");
        if let Err(error) = std::fs::write(&temp, text) {
            self.output = format!("Cannot write {}: {error}", temp.display());
            return;
        }
        let backup = path.with_extension("toml.bak");
        let had_old = path.exists();
        if had_old {
            let _ = std::fs::remove_file(&backup);
            if let Err(error) = std::fs::rename(path, &backup) {
                let _ = std::fs::remove_file(&temp);
                self.output = format!("Cannot back up {}: {error}", path.display());
                return;
            }
        }
        if let Err(error) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            if had_old {
                let _ = std::fs::rename(&backup, path);
            }
            self.output = format!("Cannot finalize {}: {error}", path.display());
            return;
        }
        if had_old {
            let _ = std::fs::remove_file(&backup);
        }
        self.output = format!("Saved {}\n{}", path.display(), self.output);
    }

    fn ui(&mut self, ui: &mut egui::Ui, plan_path: &str) {
        ui.heading("Base");
        let classes = supported_classes(&self.data);
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("builder_class")
                .selected_text(&self.class)
                .show_ui(ui, |ui| {
                    for class in &classes {
                        if ui
                            .selectable_value(&mut self.class, class.clone(), class)
                            .clicked()
                        {
                            self.base_query.clear();
                        }
                    }
                });
            ui.label("Base");
            ui.text_edit_singleline(&mut self.base_query);
            if ui.button("Clear").clicked() {
                self.base_query.clear();
            }
        });
        let bases = bases_for_class(&self.data, &self.class);
        if !bases.iter().any(|base| base.name == self.base_name) {
            self.base_name = bases
                .first()
                .map(|base| base.name.clone())
                .unwrap_or_default();
        }
        let needle = self.base_query.to_ascii_lowercase();
        let matches = bases
            .iter()
            .filter(|base| needle.is_empty() || base.name.to_ascii_lowercase().contains(&needle))
            .take(24)
            .collect::<Vec<_>>();
        egui::ScrollArea::vertical()
            .id_salt("builder_bases")
            .max_height(120.0)
            .show(ui, |ui| {
                for base in matches {
                    if ui
                        .selectable_label(self.base_name == base.name, &base.name)
                        .clicked()
                    {
                        self.base_name = base.name.clone();
                    }
                }
            });
        ui.horizontal(|ui| {
            ui.label("Selected");
            ui.monospace(if self.base_name.is_empty() {
                "none"
            } else {
                self.base_name.as_str()
            });
            ui.label("Item level");
            ui.add(egui::DragValue::new(&mut self.plan.base.item_level).range(1..=100));
            ui.checkbox(&mut self.plan.base.unrestricted, "Unrestricted");
        });
        let base = bases
            .iter()
            .find(|base| base.name == self.base_name)
            .copied();
        if let Some(base) = base {
            let supported = base_influences(base);
            if !supported.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Influences (max 2)");
                    for influence in supported {
                        let mut checked = self.plan.base.influences.contains(&influence);
                        if ui.checkbox(&mut checked, influence.display()).changed() {
                            if checked {
                                if self.plan.base.influences.len() < 2 {
                                    self.plan.base.influences.push(influence);
                                }
                            } else {
                                self.plan
                                    .base
                                    .influences
                                    .retain(|entry| *entry != influence);
                            }
                        }
                    }
                });
            }
        }
        self.sync_base();
        if self.plan.base.influences.len() > 2 {
            self.plan.base.influences.truncate(2);
        }
        if !self.plan.base.influences.is_empty() {
            ui.small(format!(
                "Influences: {}",
                self.plan
                    .base
                    .influences
                    .iter()
                    .map(|influence| influence.display())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        ui.separator();
        ui.heading("Plan");
        ui.horizontal_wrapped(|ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut self.plan.name).desired_width(220.0));
            ui.label("Start");
            egui::ComboBox::from_id_salt("builder_start")
                .selected_text(format!("{:?}", self.plan.start))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.plan.start, StartRarity::Normal, "normal");
                    ui.selectable_value(&mut self.plan.start, StartRarity::Magic, "magic");
                    ui.selectable_value(&mut self.plan.start, StartRarity::Rare, "rare");
                });
            ui.label("Confidence");
            ui.add(
                egui::DragValue::new(&mut self.plan.options.confidence)
                    .range(0.01..=0.999)
                    .speed(0.005),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Costs");
            ui.label("alt");
            ui.add(egui::DragValue::new(&mut self.plan.costs.alt).speed(0.01));
            ui.label("aug");
            ui.add(egui::DragValue::new(&mut self.plan.costs.aug).speed(0.01));
            ui.label("regal");
            ui.add(egui::DragValue::new(&mut self.plan.costs.regal).speed(0.01));
            ui.label("alch");
            ui.add(egui::DragValue::new(&mut self.plan.costs.alch).speed(0.01));
            ui.label("scour");
            ui.add(egui::DragValue::new(&mut self.plan.costs.scour).speed(0.01));
            ui.label("chaos");
            ui.add(egui::DragValue::new(&mut self.plan.costs.chaos).speed(0.01));
            ui.label("transmute");
            ui.add(egui::DragValue::new(&mut self.plan.costs.transmute).speed(0.01));
        });

        ui.separator();
        ui.heading("Steps");
        let mut remove_step = None;
        for index in 0..self.plan.steps.len() {
            let mut step = self.plan.steps[index].clone();
            ui.push_id(("builder_step", index), |ui| {
                egui::CollapsingHeader::new(format!("{}. {}", index + 1, step.name))
                    .default_open(index == self.active_step)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Name");
                            ui.text_edit_singleline(&mut step.name);
                            if ui
                                .selectable_label(self.active_step == index, "Edit mods")
                                .clicked()
                            {
                                self.active_step = index;
                            }
                            if ui.button("Remove").clicked() {
                                remove_step = Some(index);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Action");
                            egui::ComboBox::from_id_salt(("builder_action", index))
                                .selected_text(step.action.label())
                                .show_ui(ui, |ui| {
                                    for action in all_step_actions() {
                                        ui.selectable_value(
                                            &mut step.action,
                                            action,
                                            action.label(),
                                        );
                                    }
                                });
                            ui.label("On fail");
                            egui::ComboBox::from_id_salt(("builder_fail", index))
                                .selected_text(format!("{:?}", step.on_fail))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut step.on_fail,
                                        OnFail::Restart,
                                        "restart",
                                    );
                                    ui.selectable_value(
                                        &mut step.on_fail,
                                        OnFail::Continue,
                                        "continue",
                                    );
                                    ui.selectable_value(&mut step.on_fail, OnFail::Abort, "abort");
                                });
                        });
                        ui.horizontal(|ui| {
                            let mut limited = step.max_attempts.is_some();
                            if ui.checkbox(&mut limited, "Max attempts").changed() {
                                step.max_attempts = limited.then_some(50_000);
                            }
                            if let Some(limit) = &mut step.max_attempts {
                                ui.add(egui::DragValue::new(limit).range(1..=10_000_000));
                            }
                            if step.action == StepAction::AltAug {
                                ui.label("Augment");
                                egui::ComboBox::from_id_salt(("builder_aug", index))
                                    .selected_text(format!("{:?}", step.augment))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut step.augment,
                                            AugmentMode::Never,
                                            "never",
                                        );
                                        ui.selectable_value(
                                            &mut step.augment,
                                            AugmentMode::IfNeeded,
                                            "if needed",
                                        );
                                        ui.selectable_value(
                                            &mut step.augment,
                                            AugmentMode::Always,
                                            "always",
                                        );
                                    });
                            }
                        });
                        ui.horizontal(|ui| {
                            let mut has_accept = step.goal.is_some();
                            if ui.checkbox(&mut has_accept, "Accept goal").changed() {
                                step.goal = has_accept.then(default_goal);
                            }
                            if step.goal.is_none() {
                                ui.small("one-shot step advances after the action");
                            }
                        });
                        if let Some(goal) = &mut step.goal {
                            edit_goal(ui, goal);
                        }
                    });
            });
            self.plan.steps[index] = step;
        }
        if let Some(index) = remove_step {
            if self.plan.steps.len() > 1 {
                self.plan.steps.remove(index);
                self.active_step = self.active_step.min(self.plan.steps.len() - 1);
            }
        }
        ui.horizontal(|ui| {
            if ui.button("Add step").clicked() {
                self.plan.steps.push(PlanStep {
                    name: format!("Step {}", self.plan.steps.len() + 1),
                    action: StepAction::Alt,
                    goal: Some(default_goal()),
                    on_fail: OnFail::Restart,
                    max_attempts: Some(50_000),
                    augment: AugmentMode::IfNeeded,
                });
                self.active_step = self.plan.steps.len() - 1;
            }
            ui.label("Edit mods for");
            egui::ComboBox::from_id_salt("builder_active_step")
                .selected_text(
                    self.plan
                        .steps
                        .get(self.active_step)
                        .map(|step| step.name.clone())
                        .unwrap_or_default(),
                )
                .show_ui(ui, |ui| {
                    for (index, step) in self.plan.steps.iter().enumerate() {
                        ui.selectable_value(&mut self.active_step, index, &step.name);
                    }
                });
        });

        ui.separator();
        ui.heading("Add mod to active step");
        ui.horizontal(|ui| {
            ui.label("Search");
            ui.text_edit_singleline(&mut self.mod_query);
            egui::ComboBox::from_id_salt("builder_mod_slot")
                .selected_text(format!("{:?}", self.mod_slot))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.mod_slot, Slot::Prefix, "prefix");
                    ui.selectable_value(&mut self.mod_slot, Slot::Suffix, "suffix");
                    ui.selectable_value(&mut self.mod_slot, Slot::Any, "any");
                });
            ui.checkbox(&mut self.mod_weighted, "Weighted");
        });
        let mut chosen = None;
        if let Some(pool) = &self.pool {
            let results = pool.search(&self.mod_query, self.mod_slot, 40);
            if results.is_empty() {
                ui.label("No eligible mods for the current base/level/influences.");
            }
            egui::ScrollArea::vertical()
                .id_salt("builder_mods")
                .max_height(220.0)
                .show(ui, |ui| {
                    for entry in results {
                        let stat = entry
                            .stat_lines
                            .first()
                            .map(|line| line.template.as_str())
                            .unwrap_or("");
                        let label = format!(
                            "{} | {:?} | T{} | {}",
                            entry.group, entry.slot, entry.tier, stat
                        );
                        if ui.selectable_label(false, label).clicked() {
                            chosen = Some(Requirement {
                                slot: entry.slot,
                                group: Some(entry.group.clone()),
                                affix: None,
                                min_tier: None,
                                min_value: None,
                                min_percentile: None,
                                value_index: 0,
                                weight: 1,
                                label: None,
                            });
                        }
                    }
                });
        } else {
            ui.label("Pool is unavailable; fix the base or influences first.");
        }
        if let Some(requirement) = chosen {
            if let Some(step) = self.plan.steps.get_mut(self.active_step) {
                let goal = step.goal.get_or_insert_with(default_goal);
                if self.mod_weighted {
                    goal.weighted.push(requirement);
                } else {
                    goal.required.push(requirement);
                }
                self.rebuild_pool();
            }
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Validate").clicked() {
                self.validate();
            }
            if ui
                .add_enabled(!self.simulating, egui::Button::new("Simulate 1k"))
                .clicked()
            {
                self.simulate_requested = true;
                self.simulating = true;
                self.output = "Simulating 1,000 runs...".to_string();
            }
            if ui.button("Validate & Save").clicked() {
                self.save(plan_path);
            }
            if ui.button("Recompute probabilities").clicked() {
                self.rebuild_pool();
            }
        });
        if !self.estimates.is_empty() {
            ui.separator();
            ui.label("Step probabilities");
            for (index, estimate) in self.estimates.iter().enumerate() {
                if let Some(estimate) = estimate {
                    ui.monospace(format!(
                        "{}. p={} expected {} confidence {}",
                        index + 1,
                        estimate
                            .p_success
                            .map(|value| format!("{:.4}%", value * 100.0))
                            .unwrap_or_else(|| "-".to_string()),
                        estimate
                            .expected_attempts
                            .map(|value| format!("{value:.2}"))
                            .unwrap_or_else(|| "-".to_string()),
                        estimate
                            .confidence_attempts
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "-".to_string())
                    ));
                } else if let Some(step) = self.plan.steps.get(index) {
                    ui.monospace(format!(
                        "{}. {}: run simulation for probability",
                        index + 1,
                        step.name
                    ));
                }
            }
        }
        ui.separator();
        output(ui, &self.output);
    }
}

fn default_goal() -> Goal {
    Goal {
        min_prefixes: Some(1),
        ..Goal::default()
    }
}

fn all_step_actions() -> [StepAction; 9] {
    [
        StepAction::Transmute,
        StepAction::Alt,
        StepAction::AltAug,
        StepAction::Augment,
        StepAction::Regal,
        StepAction::Alch,
        StepAction::Scour,
        StepAction::ScourAlch,
        StepAction::Chaos,
    ]
}

fn base_key(plan: &BuildPlan) -> String {
    format!(
        "{:?}|{:?}|{}|{}|{:?}",
        plan.base.item_class,
        plan.base.base_names,
        plan.base.item_level,
        plan.base.unrestricted,
        plan.base.influences
    )
}

fn optional_u8(ui: &mut egui::Ui, label: &str, value: &mut Option<u8>, default: u8) {
    ui.horizontal(|ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, label).changed() {
            *value = enabled.then_some(default);
        }
        if let Some(value) = value {
            ui.add(egui::DragValue::new(value).range(0..=3));
        }
    });
}

fn optional_u32(ui: &mut egui::Ui, label: &str, value: &mut Option<u32>, default: u32) {
    ui.horizontal(|ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, label).changed() {
            *value = enabled.then_some(default);
        }
        if let Some(value) = value {
            ui.add(egui::DragValue::new(value).range(1..=1_000));
        }
    });
}

fn optional_f64(ui: &mut egui::Ui, label: &str, value: &mut Option<f64>, default: f64) {
    ui.horizontal(|ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, label).changed() {
            *value = enabled.then_some(default);
        }
        if let Some(value) = value {
            ui.add(egui::DragValue::new(value).speed(1.0));
        }
    });
}

fn edit_goal(ui: &mut egui::Ui, goal: &mut Goal) {
    ui.horizontal_wrapped(|ui| {
        optional_u8(ui, "min prefixes", &mut goal.min_prefixes, 1);
        optional_u8(ui, "max prefixes", &mut goal.max_prefixes, 1);
        optional_u8(ui, "min suffixes", &mut goal.min_suffixes, 1);
        optional_u8(ui, "max suffixes", &mut goal.max_suffixes, 1);
    });
    ui.horizontal_wrapped(|ui| {
        optional_u8(ui, "min open prefixes", &mut goal.min_open_prefixes, 1);
        optional_u8(ui, "min open suffixes", &mut goal.min_open_suffixes, 1);
        ui.label("Min score");
        ui.add(egui::DragValue::new(&mut goal.min_score).range(0..=1000));
    });
    edit_requirements(ui, "Required", &mut goal.required, false);
    edit_requirements(ui, "Weighted", &mut goal.weighted, true);
}

fn edit_requirements(
    ui: &mut egui::Ui,
    title: &str,
    requirements: &mut Vec<Requirement>,
    weighted: bool,
) {
    ui.separator();
    ui.label(title);
    let mut remove = None;
    for (index, requirement) in requirements.iter_mut().enumerate() {
        ui.push_id((title, index), |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("{}. Slot", index + 1));
                egui::ComboBox::from_id_salt((title, index, "slot"))
                    .selected_text(format!("{:?}", requirement.slot))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut requirement.slot, Slot::Prefix, "prefix");
                        ui.selectable_value(&mut requirement.slot, Slot::Suffix, "suffix");
                        ui.selectable_value(&mut requirement.slot, Slot::Any, "any");
                    });
                ui.label("Group");
                let mut group = requirement.group.clone().unwrap_or_default();
                if ui
                    .add(egui::TextEdit::singleline(&mut group).desired_width(130.0))
                    .changed()
                {
                    requirement.group = (!group.is_empty()).then_some(group);
                }
                ui.label("Affix");
                let mut affix = requirement.affix.clone().unwrap_or_default();
                if ui
                    .add(egui::TextEdit::singleline(&mut affix).desired_width(110.0))
                    .changed()
                {
                    requirement.affix = (!affix.is_empty()).then_some(affix);
                }
                optional_u32(ui, "Tier <=", &mut requirement.min_tier, 1);
            });
            ui.horizontal_wrapped(|ui| {
                optional_f64(ui, "Value >=", &mut requirement.min_value, 1.0);
                ui.label("Value index");
                ui.add(egui::DragValue::new(&mut requirement.value_index).range(0..=8));
                if weighted {
                    ui.label("Weight");
                    ui.add(egui::DragValue::new(&mut requirement.weight).range(1..=100));
                }
                if ui.button("Remove").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        requirements.remove(index);
    }
}

fn load_ui_data(cache: &Path) -> Result<PobData, String> {
    if poe_data::cache_is_current(cache) {
        match load_cache(cache) {
            Ok(data) => return Ok(data),
            Err(error) => {
                let _ = std::fs::remove_file(cache);
                eprintln!(
                    "Data cache {} could not be decoded ({error}); rebuilding...",
                    cache.display()
                );
            }
        }
    }
    poe_data::ensure_cache(Path::new(DATA_LOCK), Path::new(DATA_DIR), cache, None)
        .map_err(|error| error.to_string())?;
    load_cache(cache).map_err(|error| error.to_string())
}

fn validate_plan(cache: &Path, plan: &Path) -> Result<String, String> {
    let data = load_ui_data(cache)?;
    let plan = PlanFile::load(plan).map_err(|error| error.to_string())?;
    match plan {
        PlanFile::Legacy(plan) => {
            let simulator = Simulator::new(&data, plan).map_err(|error| error.to_string())?;
            let pool = &simulator.pool;
            Ok(format!(
                "Plan: {}\nBase: {}\nItem level: {}\nTags: {}\nPool: {} prefixes, {} suffixes\nGoal: {} required, {} weighted, min score {}\nStrategy: {:?}\nPlan is valid.\n",
                simulator.plan.name,
                pool.base_label,
                pool.item_level,
                pool.base_tags.join(", "),
                pool.prefixes.len(),
                pool.suffixes.len(),
                simulator.plan.goal.required.len(),
                simulator.plan.goal.weighted.len(),
                simulator.plan.goal.min_score,
                simulator.plan.strategy.method
            ))
        }
        PlanFile::Steps(plan) => {
            let summary = plan.validate(&data).map_err(|error| error.to_string())?;
            let pool = plan.pool(&data).map_err(|error| error.to_string())?;
            let mut out = format!(
                "Plan: {}\nBase: {}\nItem level: {}\nUnrestricted: {}\nTags: {}\nPool: {} prefixes, {} suffixes\nSteps: {}\n",
                plan.name,
                summary.base,
                summary.item_level,
                plan.base.unrestricted,
                summary.base_tags.join(", "),
                summary.prefixes,
                summary.suffixes,
                summary.steps
            );
            for (index, step) in plan.steps.iter().enumerate() {
                out.push_str(&format!(
                    "  {}. {} ({}) on_fail {:?}\n",
                    index + 1,
                    step.name,
                    step.action.label(),
                    step.on_fail
                ));
            }
            for (index, estimate) in analytic_estimates(&plan, &pool).into_iter().enumerate() {
                if let Some(estimate) = estimate {
                    out.push_str(&format!(
                        "  p{}: {:.4}% | expected {} | confidence {}\n",
                        index + 1,
                        estimate.p_success.unwrap_or(0.0) * 100.0,
                        estimate
                            .expected_attempts
                            .map(|value| format!("{value:.2}"))
                            .unwrap_or_else(|| "-".to_string()),
                        estimate
                            .confidence_attempts
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "-".to_string())
                    ));
                }
            }
            out.push_str("Plan is valid.\n");
            Ok(out)
        }
    }
}

fn run_simulation(cache: &Path, plan: &Path, runs: u64, seed: u64) -> Result<String, String> {
    let data = load_ui_data(cache)?;
    let plan = PlanFile::load(plan).map_err(|error| error.to_string())?;
    match plan {
        PlanFile::Legacy(plan) => {
            let report = simulate(&data, plan, runs, seed).map_err(|error| error.to_string())?;
            let mut out = format!(
                "Plan: {}\nRuns: {} completed / {} requested (seed {})\nSuccess: {:.2}% ({}/{})\nAttempts: mean {:.2}\nCost: mean {:.3}\n",
                report.plan_name,
                report.completed_runs,
                report.runs,
                report.seed,
                report.success_rate * 100.0,
                report.successes,
                report.completed_runs,
                report.mean_attempts,
                report.mean_cost
            );
            if report.truncated {
                out.push_str("Stopped at the attempt budget.\n");
            }
            out.push_str(&format!(
                "Currency/run: {}\nFailures: max attempts {}, no mods {}\n",
                format_currency_map(&report.mean_currency),
                report.max_attempts_reached,
                report.failed_no_mods
            ));
            Ok(out)
        }
        PlanFile::Steps(plan) => {
            let report = simulate_build(&data, plan, runs, seed, None, None)
                .map_err(|error| error.to_string())?;
            let mut out = format!(
                "Plan: {}\nRuns: {} completed / {} requested (seed {})\nSuccess: {:.2}% ({}/{})\nAttempts: mean {:.2}\nCost: mean {:.3}\nCurrency/run: {}\n",
                report.plan_name,
                report.completed_runs,
                report.runs,
                report.seed,
                report.success_rate * 100.0,
                report.successes,
                report.completed_runs,
                report.mean_attempts,
                report.mean_cost,
                format_currency_map(&report.mean_currency)
            );
            if report.truncated {
                out.push_str("Stopped at the attempt budget.\n");
            }
            out.push_str("Steps:\n");
            for step in &report.steps {
                out.push_str(&format!(
                    "  {}: p={} expected {} confidence {} | actions {} accepts {}\n",
                    step.step_index + 1,
                    step.p_success
                        .map(|value| format!("{:.4}%", value * 100.0))
                        .unwrap_or_else(|| "-".to_string()),
                    step.expected_attempts
                        .map(|value| format!("{value:.2}"))
                        .unwrap_or_else(|| "-".to_string()),
                    step.confidence_attempts
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    step.actions,
                    step.accepts
                ));
            }
            Ok(out)
        }
    }
}

fn format_currency_map(map: &std::collections::BTreeMap<String, f64>) -> String {
    if map.is_empty() {
        return "-".to_string();
    }
    map.iter()
        .map(|(name, value)| format!("{name} {value:.2}"))
        .collect::<Vec<_>>()
        .join(", ")
}

struct SessionRequest {
    cache: PathBuf,
    plan: PathBuf,
    calibration: PathBuf,
    delays: PathBuf,
    log_dir: PathBuf,
    config: PathBuf,
    dry_run: bool,
    max_steps: u32,
    max_seconds: u64,
    max_cost: Option<f64>,
    verify_currency: bool,
    hold_currency: bool,
    cancel: Arc<AtomicBool>,
    sender: Sender<WorkerMessage>,
}

enum SessionResult {
    Preview(Result<String, String>),
    Ran(Box<Result<SessionReport, String>>),
}

fn run_session(request: SessionRequest) -> SessionResult {
    match run_session_inner(&request) {
        Ok(SessionResult::Preview(text)) => SessionResult::Preview(text),
        Ok(SessionResult::Ran(result)) => SessionResult::Ran(result),
        Err(error) => {
            if request.dry_run {
                SessionResult::Preview(Err(error))
            } else {
                SessionResult::Ran(Box::new(Err(error)))
            }
        }
    }
}

fn run_session_inner(request: &SessionRequest) -> Result<SessionResult, String> {
    let data = load_ui_data(&request.cache)?;
    let plan = PlanFile::load(&request.plan).map_err(|error| error.to_string())?;
    let calibration = Calibration::load(&request.calibration).map_err(|error| error.to_string())?;
    if calibration.item.is_none() {
        return Err("Calibration item slot is missing.".to_string());
    }
    if !request.dry_run {
        let missing = calibration.missing(Calibration::all_targets());
        if !missing.is_empty() {
            return Err(format!(
                "Calibration is incomplete; missing: {}.",
                missing.join(", ")
            ));
        }
    }
    let delays =
        DelaySettings::load_or_default(&request.delays).map_err(|error| error.to_string())?;
    let config = AppConfig::load_or_default(&request.config).map_err(|error| error.to_string())?;
    let hotkeys = config.hotkeys.parse().map_err(|error| error.to_string())?;
    let mut game = Win32Game::find(&calibration.game_title).map_err(|error| error.to_string())?;
    game.set_action_delay(delays.action);
    game.apply_hotkeys(hotkeys);
    let mut io = Win32GameIo::new(game, calibration);
    io.game.focus().map_err(|error| error.to_string())?;

    let config = SessionConfig {
        max_steps: request.max_steps,
        max_currency_actions: request.max_steps.saturating_mul(2).max(2),
        max_seconds: request.max_seconds,
        max_cost: request.max_cost,
        max_same_fingerprint: 3,
        verify_currency: request.verify_currency,
        hold_currency: request.hold_currency,
        require_foreground: true,
    };

    if request.dry_run {
        let mut session = CraftSession::new(plan, &data, &mut io, config, None)
            .map_err(|error| error.to_string())?;
        let preview = session.preview().map_err(|error| error.to_string())?;
        return Ok(SessionResult::Preview(Ok(format_preview(&preview))));
    }

    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let log_path = request.log_dir.join(format!("session-{epoch}.jsonl"));
    let logger = SessionLogger::create(&log_path).map_err(|error| error.to_string())?;
    let mut session = CraftSession::new(plan, &data, &mut io, config, Some(logger))
        .map_err(|error| error.to_string())?;
    let sender = request.sender.clone();
    session.set_progress_sink(Box::new(move |progress| {
        let _ = sender.send(WorkerMessage::Progress(progress));
    }));
    session.set_cancel_flag(request.cancel.clone());
    let result = session.run().map_err(|error| error.to_string());
    Ok(SessionResult::Ran(Box::new(result)))
}

fn format_preview(preview: &poe_session::PreviewReport) -> String {
    let mut out = format!(
        "Item: {} / {} | {:?}\nGoal: {} | score {}/{} | required {}/{}\nNext: {}\n",
        preview.item.item_class.as_deref().unwrap_or("?"),
        preview.item.name.as_deref().unwrap_or("?"),
        preview.item.rarity,
        if preview.evaluation.satisfied {
            "satisfied"
        } else {
            "not satisfied"
        },
        preview.evaluation.score,
        preview.min_score,
        preview.evaluation.required_matched,
        preview.evaluation.required_total,
        preview.next_action
    );
    if let (Some(index), Some(name), Some(total)) = (
        preview.step_index,
        preview.step_name.as_deref(),
        preview.step_total,
    ) {
        out.push_str(&format!("Step: {}/{total} | {name}\n", index + 1));
    }
    if !preview.currencies.is_empty() {
        out.push_str(&format!(
            "Currencies: {}\n",
            preview
                .currencies
                .iter()
                .map(|currency| currency.label())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out
}

fn format_report(report: &SessionReport) -> String {
    let mut out = format!(
        "Session: {} ({:?})\nSteps: {} | actions: {} | elapsed: {:.1}s\n",
        if report.success { "success" } else { "stopped" },
        report.stop_reason,
        report.steps,
        report.currency_actions,
        report.elapsed_seconds
    );
    out.push_str(&format!(
        "Final item: {} / {} | goal: {}\n",
        report.final_item.item_class.as_deref().unwrap_or("?"),
        report.final_item.name.as_deref().unwrap_or("?"),
        if report.final_evaluation.satisfied {
            "satisfied"
        } else {
            "not satisfied"
        }
    ));
    if !report.currency_used.is_empty() {
        out.push_str(&format!(
            "Currency used: {}\n",
            report
                .currency_used
                .iter()
                .map(|(currency, count)| format!("{currency} {count}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(path) = &report.log_path {
        out.push_str(&format!("Log: {}\n", path.display()));
    }
    for warning in &report.warnings {
        out.push_str(&format!("Warning: {warning}\n"));
    }
    out
}
