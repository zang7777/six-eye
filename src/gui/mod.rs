use std::collections::VecDeque;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use eframe::egui::{self, Color32, Stroke};

use crate::models::{ConnectedDevice, ConnectionInfo, Network};
use crate::monitoring::{self, ActivitySample, MonitoringSummary};
use crate::oui::OuiDatabase;
use crate::pose::{self, PoseEstimate};
use crate::scanner;
use crate::vitals::{self, VitalSigns};

pub mod devices;
pub mod export;
pub mod radar;
pub mod tabs;

pub const LIVE_SCAN_INTERVAL_SECS: u64 = 3;

// ── Theme Colors ─────────────────────────────────────────────────────

pub const BG_DARK: Color32 = Color32::from_rgb(11, 15, 22);
pub const BG_PANEL: Color32 = Color32::from_rgb(15, 21, 31);
pub const BG_CARD: Color32 = Color32::from_rgb(22, 30, 43);
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(225, 232, 241);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(151, 167, 189);
pub const TEXT_DIM: Color32 = Color32::from_rgb(92, 106, 126);
pub const ACCENT: Color32 = Color32::from_rgb(80, 210, 220);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(105, 156, 255);
pub const SWEEP_GREEN: Color32 = Color32::from_rgb(82, 222, 145);
pub const MOTION_COLOR: Color32 = Color32::from_rgb(255, 164, 84);
pub const GRID_LINE: Color32 = Color32::from_rgb(31, 42, 58);
pub const GRID_AXIS: Color32 = Color32::from_rgb(45, 61, 82);
pub const BAND_24: Color32 = Color32::from_rgb(82, 210, 220);
pub const BAND_5: Color32 = Color32::from_rgb(92, 152, 255);
pub const BAND_6: Color32 = Color32::from_rgb(255, 194, 92);

pub const ALERT_CRITICAL: Color32 = Color32::from_rgb(255, 60, 80);
pub const ALERT_WARNING: Color32 = Color32::from_rgb(255, 164, 84);
pub const ALERT_ATTENTION: Color32 = Color32::from_rgb(242, 205, 77);

#[derive(PartialEq)]
pub enum ActiveTab {
    Overview,
    PoseVitals,
    Devices,
    Radios,
    Hotspots,
}

pub struct RadarApp {
    shared_networks: Arc<Mutex<Vec<Network>>>,
    shared_connected_devices: Arc<Mutex<Vec<ConnectedDevice>>>,
    shared_connection: Arc<Mutex<Option<ConnectionInfo>>>,
    shared_scan_tick: Arc<Mutex<u64>>,
    shared_last_scan_at: Arc<Mutex<Option<DateTime<Local>>>>,
    scan_request_tx: Sender<()>,
    pub local_networks: Vec<Network>,
    pub local_connected_devices: Vec<ConnectedDevice>,
    pub local_connection: Option<ConnectionInfo>,
    pub last_scan_at: Option<DateTime<Local>>,
    pub last_export_path: Option<String>,
    pub selected_bssid: Option<String>,
    pub status_line: Arc<Mutex<String>>,
    pub scan_ok: Arc<Mutex<bool>>,
    pub monitoring_summary: MonitoringSummary,
    pub activity_history: VecDeque<ActivitySample>,
    pub pose_history: VecDeque<PoseEstimate>,
    pub vitals_history: VecDeque<VitalSigns>,
    last_synced_tick: u64,
    pub app_start: Instant,
    pub active_tab: ActiveTab,

    // Devices tab state
    pub device_search: String,
    pub device_role_filter: String,
    pub device_expanded: std::collections::HashSet<String>,
}

impl RadarApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        apply_theme(&cc.egui_ctx);

        let oui_db = Arc::new(OuiDatabase::load());
        let shared_networks = Arc::new(Mutex::new(Vec::new()));
        let shared_connected_devices = Arc::new(Mutex::new(Vec::new()));
        let shared_connection = Arc::new(Mutex::new(None));
        let shared_scan_tick = Arc::new(Mutex::new(0_u64));
        let shared_last_scan_at = Arc::new(Mutex::new(None));
        let scan_ok = Arc::new(Mutex::new(false));
        let status_line = Arc::new(Mutex::new(
            "● Live — warming up observatory feed".to_string(),
        ));
        let (scan_request_tx, scan_request_rx) = mpsc::channel();

        let thread_networks = Arc::clone(&shared_networks);
        let thread_connected_devices = Arc::clone(&shared_connected_devices);
        let thread_connection = Arc::clone(&shared_connection);
        let thread_scan_tick = Arc::clone(&shared_scan_tick);
        let thread_last_scan_at = Arc::clone(&shared_last_scan_at);
        let thread_scan_ok = Arc::clone(&scan_ok);
        let thread_status_line = Arc::clone(&status_line);
        let thread_oui_db = Arc::clone(&oui_db);
        let egui_ctx = cc.egui_ctx.clone();

        thread::spawn(move || loop {
            match scanner::scan(&thread_oui_db) {
                Ok(bundle) => {
                    let scanned_at = Local::now();
                    let uplink_label = bundle
                        .connection
                        .as_ref()
                        .map(|connection| connection.display_name().to_string())
                        .unwrap_or_else(|| "offline".to_string());
                    let device_len = bundle.connected_devices.len();
                    let network_len = update_shared_networks(&thread_networks, bundle.networks);

                    *thread_connected_devices.lock().unwrap() = bundle.connected_devices;
                    *thread_connection.lock().unwrap() = bundle.connection;
                    *thread_scan_tick.lock().unwrap() += 1;
                    *thread_last_scan_at.lock().unwrap() = Some(scanned_at);
                    *thread_scan_ok.lock().unwrap() = true;
                    *thread_status_line.lock().unwrap() = format!(
                        "● Live — {network_len} radios / {device_len} LAN peers / uplink {uplink_label}"
                    );
                }
                Err(error) => {
                    *thread_scan_ok.lock().unwrap() = false;
                    *thread_status_line.lock().unwrap() = format!("⚠ {error}");
                }
            }

            egui_ctx.request_repaint();
            match scan_request_rx.recv_timeout(Duration::from_secs(LIVE_SCAN_INTERVAL_SECS)) {
                Ok(()) => while scan_request_rx.try_recv().is_ok() {},
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        });

        Self {
            shared_networks,
            shared_connected_devices,
            shared_connection,
            shared_scan_tick,
            shared_last_scan_at,
            scan_request_tx,
            local_networks: Vec::new(),
            local_connected_devices: Vec::new(),
            local_connection: None,
            last_scan_at: None,
            last_export_path: None,
            selected_bssid: None,
            status_line,
            scan_ok,
            monitoring_summary: MonitoringSummary::default(),
            activity_history: VecDeque::new(),
            pose_history: VecDeque::new(),
            vitals_history: VecDeque::new(),
            last_synced_tick: 0,
            app_start: Instant::now(),
            active_tab: ActiveTab::Overview,
            device_search: String::new(),
            device_role_filter: "All".to_string(),
            device_expanded: std::collections::HashSet::new(),
        }
    }

    pub fn selected_network(&self) -> Option<&Network> {
        self.selected_bssid.as_deref().and_then(|bssid| {
            self.local_networks
                .iter()
                .find(|network| network.bssid == bssid)
        })
    }

    pub fn try_scan(&mut self) {
        if self.scan_request_tx.send(()).is_ok() {
            *self.status_line.lock().unwrap() = "● Live — manual refresh requested".to_string();
        } else {
            *self.scan_ok.lock().unwrap() = false;
            *self.status_line.lock().unwrap() = "⚠ scanner thread is unavailable".to_string();
        }
    }

    fn sync_state(&mut self) {
        let scan_tick = *self.shared_scan_tick.lock().unwrap();
        if scan_tick == self.last_synced_tick {
            return;
        }

        self.local_networks = self.shared_networks.lock().unwrap().clone();
        self.local_connected_devices = self.shared_connected_devices.lock().unwrap().clone();
        self.local_connection = self.shared_connection.lock().unwrap().clone();
        self.last_scan_at = self.shared_last_scan_at.lock().unwrap().clone();

        let still_selected = self
            .selected_bssid
            .as_deref()
            .map(|bssid| {
                self.local_networks
                    .iter()
                    .any(|network| network.bssid == bssid)
            })
            .unwrap_or(false);
        if !still_selected {
            self.selected_bssid = self
                .local_networks
                .first()
                .map(|network| network.bssid.clone());
        }

        self.monitoring_summary =
            monitoring::summarize(&self.local_networks, &self.local_connected_devices);
        monitoring::push_activity_sample(&mut self.activity_history, &self.monitoring_summary);
        pose::push_pose_history(&mut self.pose_history, self.monitoring_summary.pose.clone());
        vitals::push_vitals_history(
            &mut self.vitals_history,
            self.monitoring_summary.vitals.clone(),
        );
        self.last_synced_tick = scan_tick;
    }
}

impl eframe::App for RadarApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        apply_theme(ctx);
        self.sync_state();
        ctx.request_repaint_after(Duration::from_millis(16));
        tabs::draw_observatory_panel(self, ctx);
        radar::draw_radar_panel(self, ctx);
    }
}

pub fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(TEXT_PRIMARY);
    visuals.panel_fill = BG_DARK;
    visuals.window_fill = BG_PANEL;
    visuals.extreme_bg_color = Color32::from_rgb(9, 12, 17);
    visuals.faint_bg_color = BG_CARD;
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(24, 33, 47);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_SECONDARY);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(33, 46, 67);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.active.bg_fill = Color32::from_rgb(44, 60, 88);
    visuals.selection.bg_fill = Color32::from_rgb(23, 74, 110);
    visuals.selection.stroke = Stroke::new(1.4, ACCENT);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(0.0, Color32::TRANSPARENT);
    ctx.set_visuals(visuals);
}

fn update_shared_networks(
    shared_networks: &Arc<Mutex<Vec<Network>>>,
    networks: Vec<Network>,
) -> usize {
    let mut locked_networks = shared_networks.lock().unwrap();
    let prior = std::mem::take(&mut *locked_networks);
    let mut by_bssid: std::collections::HashMap<String, Network> = prior
        .into_iter()
        .map(|network| (network.bssid.clone(), network))
        .collect();

    let mut merged = Vec::with_capacity(networks.len());
    for network in networks {
        if let Some(previous) = by_bssid.remove(&network.bssid) {
            let mut signal_history = previous.signal_history;
            signal_history.push_back((Local::now(), network.signal_strength));
            while signal_history.len() > 48 {
                signal_history.pop_front();
            }
            merged.push(Network {
                angle: previous.angle,
                vendor: previous.vendor.or(network.vendor.clone()),
                signal_history,
                is_connected: network.is_connected,
                ..network
            });
        } else {
            merged.push(network);
        }
    }

    merged.sort_by(|left, right| right.signal_strength.cmp(&left.signal_strength));
    let count = merged.len();
    *locked_networks = merged;
    count
}

pub fn tint(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

pub fn sig_color(sig: i32) -> Color32 {
    match sig {
        -50..=0 => Color32::from_rgb(67, 220, 136),
        -65..=-51 => Color32::from_rgb(132, 214, 98),
        -75..=-66 => Color32::from_rgb(242, 205, 77),
        _ => Color32::from_rgb(248, 110, 90),
    }
}
