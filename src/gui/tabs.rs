use std::collections::VecDeque;

use chrono::{DateTime, Local};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Pos2, RichText, Sense, Shape, Stroke, Ui,
};

use crate::gui::*;
use crate::models::{AlertLevel, ConnectionInfo, Network};
use crate::monitoring::{ActivitySample, MonitoringSummary};
use crate::pose::{PoseEstimate, ZoneActivity};
use crate::vitals::VitalSigns;

pub fn draw_overview_tab(app: &mut RadarApp, ui: &mut Ui) {
    ui.add_space(8.0);
    draw_alert_banner(ui, app.monitoring_summary.alert_level);
    ui.add_space(8.0);

    draw_metric_grid(ui, &app.monitoring_summary);
    ui.add_space(8.0);

    draw_signal_health_card(ui, &app.monitoring_summary);
    ui.add_space(10.0);

    ui.horizontal_wrapped(|ui| {
        info_chip(
            ui,
            &format!(
                "👤 Presence {:.0}%",
                app.monitoring_summary.presence_score * 100.0
            ),
            ACCENT,
        );
        info_chip(
            ui,
            &format!(
                "🏃 Motion {:.0}%",
                app.monitoring_summary.motion_score * 100.0
            ),
            MOTION_COLOR,
        );
        info_chip(
            ui,
            &format!("📡 Band {}", app.monitoring_summary.dominant_band),
            ACCENT_SOFT,
        );
        info_chip(
            ui,
            &format!(
                "⏱ {}",
                app.last_scan_at
                    .as_ref()
                    .map(relative_time_label)
                    .unwrap_or_else(|| "waiting".to_string())
            ),
            TEXT_SECONDARY,
        );
    });

    ui.add_space(12.0);
    section_title(ui, "📈 Occupancy Trace");
    draw_activity_timeline(ui, &app.activity_history);

    ui.add_space(12.0);
    draw_connection_section(ui, app.local_connection.as_ref(), app.last_scan_at.as_ref());

    ui.add_space(12.0);
    draw_export_section(app, ui);
}

pub fn draw_pose_vitals_tab(app: &mut RadarApp, ui: &mut Ui) {
    ui.add_space(8.0);
    section_title(ui, "🧍 Posture Inference");
    draw_pose_summary(ui, &app.monitoring_summary.pose);

    ui.add_space(12.0);
    section_title(ui, "🧭 Zone Map");
    draw_zone_activity_list(ui, &app.monitoring_summary.pose.zones);

    ui.add_space(12.0);
    section_title(ui, "🕘 Pose Trace");
    draw_pose_history_strip(ui, &app.pose_history);

    ui.add_space(16.0);
    section_title(ui, "💓 Vital Signs (Estimates)");
    draw_vitals_summary(ui, &app.monitoring_summary.vitals);

    ui.add_space(12.0);
    section_title(ui, "📊 Vitals History");
    draw_vitals_timeline(ui, &app.vitals_history);
}

pub fn draw_radios_tab(app: &mut RadarApp, ui: &mut Ui) {
    ui.add_space(8.0);

    let selected_network = app.selected_network().cloned();
    draw_selected_network_section(ui, selected_network.as_ref());

    ui.add_space(12.0);
    section_title(
        ui,
        &format!("📻 Nearby Radios ({})", app.local_networks.len()),
    );

    for network in &app.local_networks {
        let is_selected = app.selected_bssid.as_deref() == Some(&network.bssid);
        let band = band_color(network.frequency);

        egui::Frame::default()
            .fill(BG_CARD)
            .rounding(9.0)
            .stroke(Stroke::new(
                if is_selected { 1.4 } else { 1.0 },
                if is_selected { band } else { GRID_LINE },
            ))
            .inner_margin(10.0)
            .show(ui, |ui| {
                if ui
                    .selectable_label(
                        is_selected,
                        RichText::new(format!(
                            "{}  {:>3} dBm  {}",
                            if network.is_hidden() {
                                "<hidden>"
                            } else {
                                &network.ssid
                            },
                            network.signal_strength,
                            network.band_label()
                        ))
                        .size(11.8)
                        .color(band),
                    )
                    .clicked()
                {
                    app.selected_bssid = Some(network.bssid.clone());
                }

                ui.horizontal_wrapped(|ui| {
                    info_chip(ui, &format!("ch {}", network.channel), band);
                    info_chip(ui, &network.security, TEXT_SECONDARY);
                    info_chip(
                        ui,
                        network.vendor.as_deref().unwrap_or("Unknown vendor"),
                        TEXT_DIM,
                    );
                    info_chip(
                        ui,
                        if network.is_connected {
                            "🔗 uplink"
                        } else {
                            "👁 observed"
                        },
                        if network.is_connected {
                            SWEEP_GREEN
                        } else {
                            TEXT_DIM
                        },
                    );
                });

                ui.label(
                    RichText::new(format!(
                        "{}  avg {:.0} dBm  span {} dB",
                        network.bssid,
                        network.average_signal(),
                        network.signal_span()
                    ))
                    .monospace()
                    .size(10.0)
                    .color(TEXT_DIM),
                );
            });
        ui.add_space(6.0);
    }
}

pub fn draw_hotspots_tab(app: &mut RadarApp, ui: &mut Ui) {
    ui.add_space(8.0);
    section_title(ui, "📶 Channel Congestion");
    draw_channel_heatmap(ui, &app.monitoring_summary.signal_health.channel_congestion);

    ui.add_space(14.0);
    section_title(ui, "🔥 Variance Hotspots");

    if app.monitoring_summary.hotspots.is_empty() {
        ui.label(
            RichText::new("No active hotspots.")
                .size(11.0)
                .color(TEXT_SECONDARY),
        );
        return;
    }

    for hotspot in &app.monitoring_summary.hotspots {
        let is_selected = app.selected_bssid.as_deref() == Some(&hotspot.bssid);
        let color = if hotspot.intensity >= 0.72 {
            MOTION_COLOR
        } else if hotspot.intensity >= 0.4 {
            BAND_6
        } else {
            ACCENT
        };

        egui::Frame::default()
            .fill(BG_CARD)
            .rounding(8.0)
            .stroke(Stroke::new(
                if is_selected { 1.4 } else { 1.0 },
                if is_selected { color } else { GRID_LINE },
            ))
            .inner_margin(10.0)
            .show(ui, |ui| {
                if ui
                    .selectable_label(
                        is_selected,
                        RichText::new(format!("{}  {:.1}", hotspot.label, hotspot.activity_score))
                            .size(12.0)
                            .color(color),
                    )
                    .clicked()
                {
                    app.selected_bssid = Some(hotspot.bssid.clone());
                }

                ui.horizontal_wrapped(|ui| {
                    info_chip(
                        ui,
                        &format!(
                            "Band {}  {} dBm",
                            hotspot.band_label, hotspot.signal_strength
                        ),
                        TEXT_SECONDARY,
                    );
                    info_chip(ui, &format!("Angle {:03.0}°", hotspot.angle), ACCENT_SOFT);
                    info_chip(
                        ui,
                        &format!(
                            "Range {:.0}%",
                            (1.0 - hotspot.distance_ratio).clamp(0.0, 1.0) * 100.0
                        ),
                        ACCENT,
                    );
                    info_chip(
                        ui,
                        &format!("Intensity {:.0}%", hotspot.intensity * 100.0),
                        color,
                    );
                });
            });
        ui.add_space(6.0);
    }
}

pub fn draw_observatory_panel(app: &mut RadarApp, ctx: &egui::Context) {
    egui::SidePanel::right("observatory")
        .default_width(450.0)
        .min_width(400.0)
        .frame(
            egui::Frame::default()
                .fill(BG_PANEL)
                .inner_margin(14.0)
                .stroke(Stroke::new(1.0, GRID_LINE)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("◉").size(20.0).color(ACCENT_SOFT));
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("gojosix.eye net").size(22.0).color(TEXT_PRIMARY),
                    );
                    ui.label(
                        RichText::new("signal observatory")
                            .monospace()
                            .size(11.0)
                            .color(ACCENT_SOFT),
                    );
                    ui.label(
                        RichText::new(
                            "RuView-inspired local RF dashboard for WiFi visibility, presence drift, and room fingerprinting",
                        )
                        .size(10.5)
                        .color(TEXT_DIM),
                    );
                });
            });
            ui.add_space(8.0);

            ui.horizontal_wrapped(|ui| {
                info_chip(ui, "LOCAL-ONLY", SWEEP_GREEN);
                info_chip(ui, "RUST + ZIG", ACCENT_SOFT);
                info_chip(ui, "RSSI HEURISTICS", MOTION_COLOR);
            });
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("🔄 Scan Now").size(13.0).color(ACCENT))
                    .clicked()
                {
                    app.try_scan();
                }
                ui.label(
                    RichText::new(Local::now().format("%H:%M:%S").to_string())
                        .monospace()
                        .size(11.0)
                        .color(TEXT_DIM),
                );
            });

            let status_color = if *app.scan_ok.lock().unwrap() {
                SWEEP_GREEN
            } else {
                MOTION_COLOR
            };
            let status = app.status_line.lock().unwrap().clone();
            ui.label(RichText::new(status).size(11.0).color(status_color));

            ui.horizontal_wrapped(|ui| {
                info_chip(
                    ui,
                    &format!(
                        "Last scan {}",
                        app.last_scan_at
                            .as_ref()
                            .map(relative_time_label)
                            .unwrap_or_else(|| "pending".to_string())
                    ),
                    TEXT_SECONDARY,
                );
                if let Some(path) = &app.last_export_path {
                    info_chip(ui, &format!("Export {}", compact_path(path)), ACCENT_SOFT);
                }
            });

            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                for (tab, label) in [
                    (ActiveTab::Overview, "📡 Overview"),
                    (ActiveTab::PoseVitals, "🧬 Pose & Vitals"),
                    (ActiveTab::Devices, "📱 Devices"),
                    (ActiveTab::Radios, "📻 Radios"),
                    (ActiveTab::Hotspots, "🔥 Hotspots"),
                ] {
                    if ui
                        .selectable_label(app.active_tab == tab, RichText::new(label).size(12.0))
                        .clicked()
                    {
                        app.active_tab = tab;
                    }
                }
            });

            ui.separator();

            egui::ScrollArea::vertical()
                .id_salt("observatory-scroll")
                .show(ui, |ui| match app.active_tab {
                    ActiveTab::Overview => draw_overview_tab(app, ui),
                    ActiveTab::PoseVitals => draw_pose_vitals_tab(app, ui),
                    ActiveTab::Devices => devices::draw_devices_tab(app, ui),
                    ActiveTab::Radios => draw_radios_tab(app, ui),
                    ActiveTab::Hotspots => draw_hotspots_tab(app, ui),
                });
        });
}

fn draw_alert_banner(ui: &mut Ui, level: AlertLevel) {
    let (bg, fg) = match level {
        AlertLevel::Critical => (tint(ALERT_CRITICAL, 40), ALERT_CRITICAL),
        AlertLevel::Warning => (tint(ALERT_WARNING, 40), ALERT_WARNING),
        AlertLevel::Attention => (tint(ALERT_ATTENTION, 40), ALERT_ATTENTION),
        AlertLevel::Normal => (tint(SWEEP_GREEN, 20), SWEEP_GREEN),
    };

    egui::Frame::default()
        .fill(bg)
        .rounding(8.0)
        .stroke(Stroke::new(1.0, fg))
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(level.icon()).size(18.0));
                ui.label(
                    RichText::new(format!("Alert Status: {}", level.label()))
                        .color(fg)
                        .size(14.0),
                );
            });
        });
}

fn draw_metric_grid(ui: &mut Ui, summary: &MonitoringSummary) {
    ui.columns(2, |columns| {
        metric_card(
            &mut columns[0],
            "👤 Presence",
            if summary.presence_detected {
                "Present".to_string()
            } else {
                "Calm".to_string()
            },
            summary.presence_label.to_string(),
            ACCENT,
        );
        metric_card(
            &mut columns[1],
            "🏃 Motion",
            if summary.motion_detected {
                "Moving".to_string()
            } else {
                "Still".to_string()
            },
            summary.motion_label.to_string(),
            MOTION_COLOR,
        );
    });
    ui.add_space(6.0);
    ui.columns(2, |columns| {
        metric_card(
            &mut columns[0],
            "📶 Nearby Radios",
            summary.nearby_networks.to_string(),
            format!(
                "{} visible / {} hidden",
                summary.visible_networks, summary.hidden_networks
            ),
            ACCENT_SOFT,
        );
        metric_card(
            &mut columns[1],
            "📊 Signal Field",
            format!("{:.0} dBm", summary.average_signal),
            format!("peak {} dBm", summary.strongest_signal),
            BAND_6,
        );
    });
    ui.add_space(6.0);
    ui.columns(2, |columns| {
        metric_card(
            &mut columns[0],
            "🔗 LAN Peers",
            summary.connected_devices.to_string(),
            format!("variance {:.2}", summary.variance_index),
            SWEEP_GREEN,
        );
        metric_card(
            &mut columns[1],
            "🏠 Room Health",
            summary.signal_health.health_score.to_string(),
            summary.signal_health.health_label.to_string(),
            ACCENT,
        );
    });
}

fn draw_signal_health_card(ui: &mut Ui, summary: &MonitoringSummary) {
    let health = &summary.signal_health;
    egui::Frame::default()
        .fill(BG_CARD)
        .rounding(10.0)
        .stroke(Stroke::new(1.0, GRID_LINE))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(health.health_emoji).size(24.0));
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "Signal Health: {} — {}",
                            health.health_score, health.health_label
                        ))
                        .size(14.0)
                        .color(TEXT_PRIMARY),
                    );
                    ui.horizontal_wrapped(|ui| {
                        info_chip(
                            ui,
                            &format!("📊 Stability {:.0}%", health.stability * 100.0),
                            ACCENT,
                        );
                        info_chip(
                            ui,
                            &format!("📡 Coverage {:.0}%", health.coverage_quality * 100.0),
                            ACCENT_SOFT,
                        );
                        info_chip(
                            ui,
                            &format!("⚡ Interference {:.0}%", health.interference_score * 100.0),
                            MOTION_COLOR,
                        );
                    });
                });
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("🏠 Room Fingerprint: {}", health.room_fingerprint))
                    .monospace()
                    .size(10.0)
                    .color(TEXT_DIM),
            );
        });
}

fn draw_pose_summary(ui: &mut Ui, pose: &PoseEstimate) {
    egui::Frame::default()
        .fill(BG_CARD)
        .rounding(10.0)
        .stroke(Stroke::new(1.0, GRID_LINE))
        .inner_margin(16.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(pose.posture.icon()).size(32.0));
                ui.vertical(|ui| {
                    ui.label(RichText::new(pose.posture.label()).size(18.0).color(ACCENT));
                    ui.label(
                        RichText::new(format!("Confidence: {:.0}%", pose.confidence * 100.0))
                            .size(12.0)
                            .color(TEXT_SECONDARY),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    info_chip(ui, &clock_label(&pose.timestamp), TEXT_DIM);
                });
            });
            ui.add_space(10.0);
            ui.columns(2, |columns| {
                metric_card(
                    &mut columns[0],
                    "🧭 Body Centroid",
                    format!("{:03.0}°", pose.body_centroid_angle),
                    format!(
                        "{:.0}% inward confidence",
                        (1.0 - pose.body_centroid_distance).clamp(0.0, 1.0) * 100.0
                    ),
                    ACCENT_SOFT,
                );
                let zone_label = pose
                    .dominant_zone
                    .and_then(|zone| pose.zones.get(zone as usize))
                    .map(|zone| zone.label)
                    .unwrap_or("No dominant zone");
                metric_card(
                    &mut columns[1],
                    "📍 Dominant Zone",
                    zone_label.to_string(),
                    format!("{} sectors tracked", pose.zones.len()),
                    BAND_6,
                );
            });
        });
}

fn draw_zone_activity_list(ui: &mut Ui, zones: &[ZoneActivity]) {
    if zones.is_empty() {
        ui.label(
            RichText::new("No zone activity yet.")
                .size(11.0)
                .color(TEXT_SECONDARY),
        );
        return;
    }

    for zone in zones {
        let accent = if zone.dominant {
            ACCENT_SOFT
        } else {
            TEXT_SECONDARY
        };
        egui::Frame::default()
            .fill(BG_CARD)
            .rounding(8.0)
            .stroke(Stroke::new(
                if zone.dominant { 1.3 } else { 1.0 },
                if zone.dominant {
                    ACCENT_SOFT
                } else {
                    GRID_LINE
                },
            ))
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(zone.label).size(11.5).color(accent));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{:.0}%", zone.activity_level * 100.0))
                                .size(10.0)
                                .color(accent),
                        );
                    });
                });
                draw_level_bar(ui, zone.activity_level, accent);
                ui.label(
                    RichText::new(format!(
                        "{} radios  ·  mean {:.0} dBm  ·  variance {:.2}",
                        zone.network_count, zone.signal_mean, zone.signal_variance
                    ))
                    .monospace()
                    .size(10.0)
                    .color(TEXT_DIM),
                );
            });
        ui.add_space(6.0);
    }
}

fn draw_pose_history_strip(ui: &mut Ui, history: &VecDeque<PoseEstimate>) {
    egui::Frame::default()
        .fill(BG_CARD)
        .rounding(10.0)
        .stroke(Stroke::new(1.0, GRID_LINE))
        .inner_margin(10.0)
        .show(ui, |ui| {
            if history.is_empty() {
                ui.label(
                    RichText::new("No pose samples yet.")
                        .size(11.0)
                        .color(TEXT_SECONDARY),
                );
                return;
            }

            ui.horizontal_wrapped(|ui| {
                for estimate in history.iter().rev().take(10).rev() {
                    egui::Frame::default()
                        .fill(tint(BG_DARK, 120))
                        .rounding(8.0)
                        .stroke(Stroke::new(1.0, GRID_LINE))
                        .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new(estimate.posture.icon()).size(18.0));
                                ui.label(
                                    RichText::new(estimate.posture.label())
                                        .size(9.5)
                                        .color(TEXT_SECONDARY),
                                );
                                ui.label(
                                    RichText::new(clock_label(&estimate.timestamp))
                                        .monospace()
                                        .size(9.0)
                                        .color(TEXT_DIM),
                                );
                            });
                        });
                }
            });
        });
}

fn draw_vitals_summary(ui: &mut Ui, vitals: &VitalSigns) {
    egui::Frame::default()
        .fill(BG_CARD)
        .rounding(10.0)
        .stroke(Stroke::new(1.0, GRID_LINE))
        .inner_margin(16.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(vitals.status_label)
                        .size(14.0)
                        .color(ACCENT_SOFT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    info_chip(ui, &clock_label(&vitals.timestamp), TEXT_DIM);
                });
            });
            ui.add_space(12.0);

            ui.columns(2, |columns| {
                metric_card(
                    &mut columns[0],
                    "🫁 Breathing Rate",
                    vitals
                        .breathing_rate_bpm
                        .map(|value| format!("{value:.1} bpm"))
                        .unwrap_or_else(|| "—".to_string()),
                    format!("conf {:.0}%", vitals.breathing_confidence * 100.0),
                    ACCENT,
                );
                metric_card(
                    &mut columns[1],
                    "❤️ HR Proxy",
                    vitals
                        .heart_rate_proxy_bpm
                        .map(|value| format!("~{value:.0} bpm"))
                        .unwrap_or_else(|| "—".to_string()),
                    format!("conf {:.0}%", vitals.heart_rate_confidence * 100.0),
                    BAND_6,
                );
            });

            ui.add_space(6.0);
            ui.columns(2, |columns| {
                metric_card(
                    &mut columns[0],
                    "🔬 Micro-Motion",
                    format!("{:.2}", vitals.micro_motion_index),
                    "successive RSSI drift".to_string(),
                    MOTION_COLOR,
                );
                metric_card(
                    &mut columns[1],
                    "🌊 Periodicity",
                    format!("{:.0}%", vitals.signal_periodicity * 100.0),
                    "rhythmic field energy".to_string(),
                    ACCENT_SOFT,
                );
            });
        });
}

fn draw_connection_section(
    ui: &mut Ui,
    connection: Option<&ConnectionInfo>,
    last_scan_at: Option<&DateTime<Local>>,
) {
    section_title(ui, "🔗 Active Link");
    egui::Frame::default()
        .fill(BG_CARD)
        .rounding(10.0)
        .stroke(Stroke::new(1.0, GRID_LINE))
        .inner_margin(10.0)
        .show(ui, |ui| {
            if let Some(connection) = connection {
                row(ui, "SSID", connection.display_name(), ACCENT_SOFT);
                row(ui, "Iface", &connection.interface, TEXT_SECONDARY);
                row(
                    ui,
                    "Local IP",
                    connection.local_ipv4.as_deref().unwrap_or("—"),
                    TEXT_SECONDARY,
                );
                row(
                    ui,
                    "Gateway",
                    connection.gateway.as_deref().unwrap_or("—"),
                    TEXT_SECONDARY,
                );
                row(
                    ui,
                    "BSSID",
                    connection.bssid.as_deref().unwrap_or("—"),
                    TEXT_DIM,
                );
                row(
                    ui,
                    "Adapter",
                    connection.local_hwaddr.as_deref().unwrap_or("—"),
                    TEXT_DIM,
                );
                if let Some(last_scan_at) = last_scan_at {
                    row(ui, "Seen", &relative_time_label(last_scan_at), TEXT_DIM);
                }
            } else {
                ui.label(
                    RichText::new("No active WiFi uplink detected yet.")
                        .size(11.0)
                        .color(TEXT_SECONDARY),
                );
            }
        });
}

fn draw_selected_network_section(ui: &mut Ui, network: Option<&Network>) {
    section_title(ui, "📻 Selected Radio");
    let Some(network) = network else {
        ui.label(
            RichText::new("Pick a radio on the radar to inspect it.")
                .size(11.0)
                .color(TEXT_SECONDARY),
        );
        return;
    };

    let band = band_color(network.frequency);
    egui::Frame::default()
        .fill(BG_CARD)
        .rounding(10.0)
        .stroke(Stroke::new(1.0, GRID_LINE))
        .inner_margin(10.0)
        .show(ui, |ui| {
            row(
                ui,
                "SSID",
                if network.is_hidden() {
                    "<hidden>"
                } else {
                    &network.ssid
                },
                band,
            );
            row(ui, "BSSID", &network.bssid, TEXT_SECONDARY);
            row(ui, "Band", network.band_label(), band);
            row(ui, "Sec", &network.security, TEXT_SECONDARY);
            row(
                ui,
                "Vendor",
                network.vendor.as_deref().unwrap_or("Unknown vendor"),
                TEXT_DIM,
            );
            row(
                ui,
                "Signal",
                &format!("{} dBm", network.signal_strength),
                sig_color(network.signal_strength),
            );
            row(
                ui,
                "Avg",
                &format!("{:.0} dBm", network.average_signal()),
                TEXT_PRIMARY,
            );
            row(
                ui,
                "Peak",
                &format!("{} dBm", network.peak_signal()),
                SWEEP_GREEN,
            );
            row(
                ui,
                "Span",
                &format!("{} dB", network.signal_span()),
                MOTION_COLOR,
            );
            row(ui, "Channel", &network.channel.to_string(), band);
            row(
                ui,
                "Freq",
                &format!("{:.3} GHz", network.frequency),
                TEXT_SECONDARY,
            );
            row(
                ui,
                "Range",
                &network
                    .estimated_distance_m()
                    .map(|distance| format!("{distance:.1} m est"))
                    .unwrap_or_else(|| "—".to_string()),
                TEXT_DIM,
            );
            row(
                ui,
                "Seen",
                &network
                    .last_seen()
                    .map(relative_time_label)
                    .unwrap_or_else(|| "—".to_string()),
                TEXT_DIM,
            );

            ui.add_space(8.0);
            draw_signal_sparkline(ui, network);
        });
}

fn draw_export_section(app: &mut RadarApp, ui: &mut Ui) {
    section_title(ui, "💾 Snapshot Export");

    egui::Frame::default()
        .fill(BG_CARD)
        .rounding(10.0)
        .stroke(Stroke::new(1.0, GRID_LINE))
        .inner_margin(10.0)
        .show(ui, |ui| {
            if ui
                .button(
                    RichText::new("💾 Export Current State to JSON")
                        .size(12.0)
                        .color(TEXT_PRIMARY),
                )
                .clicked()
            {
                match export::export_json(app) {
                    Ok(path) => {
                        app.last_export_path = Some(path.clone());
                        *app.status_line.lock().unwrap() =
                            format!("● Live — exported snapshot to {}", compact_path(&path));
                    }
                    Err(error) => {
                        *app.status_line.lock().unwrap() = format!("⚠ export failed: {error}");
                    }
                }
            }

            let export_label = app
                .last_export_path
                .as_deref()
                .map(compact_path)
                .unwrap_or_else(|| "No export written yet".to_string());
            ui.label(
                RichText::new(export_label)
                    .monospace()
                    .size(10.0)
                    .color(TEXT_DIM),
            );
        });
}

fn draw_activity_timeline(ui: &mut Ui, history: &VecDeque<ActivitySample>) {
    let size = vec2(ui.available_width(), 116.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 8.0, BG_CARD);
    painter.rect_stroke(rect, 8.0, Stroke::new(1.0, GRID_LINE));

    if history.len() < 2 {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Waiting for scans",
            FontId::proportional(11.5),
            TEXT_DIM,
        );
        return;
    }

    for y in [0.25_f32, 0.5, 0.75] {
        let line_y = rect.bottom() - rect.height() * y;
        painter.line_segment(
            [pos2(rect.left(), line_y), pos2(rect.right(), line_y)],
            Stroke::new(0.6, tint(GRID_LINE, 180)),
        );
    }

    let dx = rect.width() / (history.len() - 1) as f32;
    let presence_pts: Vec<Pos2> = history
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            pos2(
                rect.left() + i as f32 * dx,
                rect.bottom() - sample.presence_score.clamp(0.0, 1.0) * rect.height(),
            )
        })
        .collect();
    let motion_pts: Vec<Pos2> = history
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            pos2(
                rect.left() + i as f32 * dx,
                rect.bottom() - sample.motion_score.clamp(0.0, 1.0) * rect.height(),
            )
        })
        .collect();
    let variance_pts: Vec<Pos2> = history
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            pos2(
                rect.left() + i as f32 * dx,
                rect.bottom() - (sample.variance_index / 5.0).clamp(0.0, 1.0) * rect.height(),
            )
        })
        .collect();

    painter.add(Shape::line(variance_pts, Stroke::new(1.0, TEXT_DIM)));
    painter.add(Shape::line(presence_pts, Stroke::new(1.7, ACCENT)));
    painter.add(Shape::line(motion_pts, Stroke::new(1.7, MOTION_COLOR)));

    if let Some(latest) = history.back() {
        painter.text(
            pos2(rect.left() + 8.0, rect.top() + 8.0),
            Align2::LEFT_TOP,
            format!(
                "{} radios  ·  {} devices  ·  {}",
                latest.network_count,
                latest.device_count,
                latest.posture.label()
            ),
            FontId::monospace(10.0),
            TEXT_SECONDARY,
        );
        painter.text(
            pos2(rect.right() - 8.0, rect.top() + 8.0),
            Align2::RIGHT_TOP,
            latest
                .breathing_bpm
                .map(|bpm| format!("var {:.2}  ·  {:.1} bpm", latest.variance_index, bpm))
                .unwrap_or_else(|| format!("var {:.2}", latest.variance_index)),
            FontId::monospace(10.0),
            TEXT_DIM,
        );
    }

    if let Some(first) = history.front() {
        painter.text(
            pos2(rect.left() + 8.0, rect.bottom() - 8.0),
            Align2::LEFT_BOTTOM,
            clock_label(&first.timestamp),
            FontId::monospace(9.0),
            TEXT_DIM,
        );
    }
    if let Some(last) = history.back() {
        painter.text(
            pos2(rect.right() - 8.0, rect.bottom() - 8.0),
            Align2::RIGHT_BOTTOM,
            clock_label(&last.timestamp),
            FontId::monospace(9.0),
            TEXT_DIM,
        );
    }
}

fn draw_vitals_timeline(ui: &mut Ui, history: &VecDeque<VitalSigns>) {
    let size = vec2(ui.available_width(), 90.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 8.0, BG_CARD);
    painter.rect_stroke(rect, 8.0, Stroke::new(1.0, GRID_LINE));

    if history.len() < 2 {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "No vitals history yet",
            FontId::proportional(11.5),
            TEXT_DIM,
        );
        return;
    }

    let dx = rect.width() / (history.len() - 1) as f32;
    let breathing_pts: Vec<Pos2> = history
        .iter()
        .enumerate()
        .filter_map(|(i, sample)| {
            sample.breathing_rate_bpm.map(|bpm| {
                let norm = ((bpm - 6.0) / (30.0 - 6.0)).clamp(0.0, 1.0);
                pos2(
                    rect.left() + i as f32 * dx,
                    rect.bottom() - norm * rect.height(),
                )
            })
        })
        .collect();
    let motion_pts: Vec<Pos2> = history
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            pos2(
                rect.left() + i as f32 * dx,
                rect.bottom() - sample.micro_motion_index.clamp(0.0, 1.0) * rect.height(),
            )
        })
        .collect();

    painter.add(Shape::line(motion_pts, Stroke::new(1.2, MOTION_COLOR)));
    if breathing_pts.len() >= 2 {
        painter.add(Shape::line(breathing_pts, Stroke::new(1.5, ACCENT_SOFT)));
    }

    if let Some(last) = history.back() {
        painter.text(
            pos2(rect.left() + 8.0, rect.top() + 8.0),
            Align2::LEFT_TOP,
            format!(
                "breathing conf {:.0}%  ·  hr conf {:.0}%",
                last.breathing_confidence * 100.0,
                last.heart_rate_confidence * 100.0
            ),
            FontId::monospace(10.0),
            TEXT_SECONDARY,
        );
        painter.text(
            pos2(rect.right() - 8.0, rect.top() + 8.0),
            Align2::RIGHT_TOP,
            clock_label(&last.timestamp),
            FontId::monospace(10.0),
            TEXT_DIM,
        );
    }
}

fn draw_signal_sparkline(ui: &mut Ui, network: &Network) {
    let size = vec2(ui.available_width(), 64.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 8.0, tint(BG_DARK, 110));
    painter.rect_stroke(rect, 8.0, Stroke::new(1.0, GRID_LINE));

    if network.signal_history.len() < 2 {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Need more signal samples",
            FontId::proportional(10.5),
            TEXT_DIM,
        );
        return;
    }

    let samples: Vec<i32> = network
        .signal_history
        .iter()
        .map(|(_, signal)| *signal)
        .collect();
    let min_signal = samples
        .iter()
        .copied()
        .min()
        .unwrap_or(network.signal_strength) as f32
        - 2.0;
    let max_signal = samples
        .iter()
        .copied()
        .max()
        .unwrap_or(network.signal_strength) as f32
        + 2.0;
    let range = (max_signal - min_signal).max(1.0);
    let dx = rect.width() / (samples.len() - 1) as f32;
    let pts: Vec<Pos2> = samples
        .iter()
        .enumerate()
        .map(|(i, signal)| {
            let norm = (*signal as f32 - min_signal) / range;
            pos2(
                rect.left() + i as f32 * dx,
                rect.bottom() - norm * rect.height(),
            )
        })
        .collect();

    painter.add(Shape::line(
        pts,
        Stroke::new(1.6, sig_color(network.signal_strength)),
    ));
    painter.text(
        pos2(rect.left() + 8.0, rect.top() + 8.0),
        Align2::LEFT_TOP,
        format!("latest {} dBm", network.signal_strength),
        FontId::monospace(9.5),
        TEXT_SECONDARY,
    );
    painter.text(
        pos2(rect.right() - 8.0, rect.top() + 8.0),
        Align2::RIGHT_TOP,
        format!("span {} dB", network.signal_span()),
        FontId::monospace(9.5),
        TEXT_DIM,
    );
}

fn draw_channel_heatmap(ui: &mut Ui, congestion: &[u8; 14]) {
    egui::Frame::default()
        .fill(BG_CARD)
        .rounding(10.0)
        .stroke(Stroke::new(1.0, GRID_LINE))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (index, load) in congestion.iter().enumerate() {
                    let color = match *load {
                        0 => TEXT_DIM,
                        1..=2 => SWEEP_GREEN,
                        3..=4 => ALERT_ATTENTION,
                        _ => ALERT_WARNING,
                    };
                    egui::Frame::default()
                        .fill(tint(color, 28))
                        .rounding(7.0)
                        .stroke(Stroke::new(1.0, tint(color, 120)))
                        .inner_margin(egui::Margin::symmetric(6.0, 5.0))
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(format!("{}", index + 1))
                                        .monospace()
                                        .size(10.0)
                                        .color(TEXT_PRIMARY),
                                );
                                ui.label(RichText::new(load.to_string()).size(10.0).color(color));
                            });
                        });
                }
            });
        });
}

fn draw_level_bar(ui: &mut Ui, level: f32, color: Color32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 8.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, tint(BG_DARK, 120));
    let fill = rect.intersect(egui::Rect::from_min_size(
        rect.min,
        vec2(rect.width() * level.clamp(0.0, 1.0), rect.height()),
    ));
    painter.rect_filled(fill, 4.0, tint(color, 180));
}

fn metric_card(
    ui: &mut Ui,
    title: &str,
    value: impl AsRef<str>,
    subtitle: impl AsRef<str>,
    accent: Color32,
) {
    egui::Frame::default()
        .fill(BG_CARD)
        .rounding(10.0)
        .stroke(Stroke::new(1.0, GRID_LINE))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.label(RichText::new(title).size(10.0).color(TEXT_DIM));
            ui.add_space(3.0);
            ui.label(RichText::new(value.as_ref()).size(20.0).color(accent));
            ui.label(
                RichText::new(subtitle.as_ref())
                    .size(10.5)
                    .color(TEXT_SECONDARY),
            );
        });
}

pub fn section_title(ui: &mut Ui, title: &str) {
    ui.label(RichText::new(title).size(13.5).color(TEXT_PRIMARY));
    ui.add_space(4.0);
}

fn row(ui: &mut Ui, key: &str, value: &str, value_color: Color32) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{:<9}", key))
                .monospace()
                .size(11.3)
                .color(TEXT_DIM),
        );
        ui.label(
            RichText::new(value)
                .monospace()
                .size(11.3)
                .color(value_color),
        );
    });
}

fn info_chip(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::default()
        .fill(BG_CARD)
        .rounding(8.0)
        .stroke(Stroke::new(1.0, GRID_LINE))
        .inner_margin(egui::Margin::symmetric(8.0, 4.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(10.0).color(color));
        });
}

fn band_color(frequency: f32) -> Color32 {
    if frequency >= 5.925 {
        BAND_6
    } else if frequency >= 4.9 {
        BAND_5
    } else {
        BAND_24
    }
}

fn relative_time_label(timestamp: &DateTime<Local>) -> String {
    let delta = Local::now().signed_duration_since(*timestamp);
    if delta.num_seconds() < 5 {
        "just now".to_string()
    } else if delta.num_seconds() < 60 {
        format!("{}s ago", delta.num_seconds())
    } else if delta.num_minutes() < 60 {
        format!("{}m ago", delta.num_minutes())
    } else {
        clock_label(timestamp)
    }
}

fn clock_label(timestamp: &DateTime<Local>) -> String {
    timestamp.format("%H:%M:%S").to_string()
}

fn compact_path(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}
