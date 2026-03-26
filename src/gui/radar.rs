use std::f32::consts::TAU;

use eframe::egui::{self, pos2, vec2, Align2, FontId, Painter, Pos2, Rect, Sense, Stroke};

use crate::gui::*;
use crate::models::{ConnectionInfo, Network};
use crate::monitoring::{MonitoringSummary, SignalHotspot};

pub fn draw_radar_panel(app: &mut RadarApp, ctx: &egui::Context) {
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(BG_DARK))
        .show(ctx, |ui| {
            let available = ui.available_size();
            let side = available.x.min(available.y).clamp(420.0, 900.0);
            let (response, painter) = ui.allocate_painter(vec2(side, side), Sense::click());

            let rect = response.rect;
            let center = rect.center();
            let radius = side * 0.46;
            let sweep = app.app_start.elapsed().as_secs_f32() * 0.58;
            let hover = response.hover_pos();

            draw_bg(&painter, rect, center, radius);
            draw_zone_overlays(
                &painter,
                center,
                radius,
                &app.monitoring_summary.zone_activity,
            );
            draw_activity_field(&painter, center, radius, &app.monitoring_summary.hotspots);
            draw_sweep(&painter, center, radius, sweep);
            draw_centroid_echo(&painter, center, radius, &app.monitoring_summary);

            if app.local_networks.is_empty() {
                painter.text(
                    center,
                    Align2::CENTER_CENTER,
                    "No radios detected yet\nUse Scan Now and wait",
                    FontId::proportional(15.0),
                    TEXT_DIM,
                );
            }

            let mut hovered_bssid = None;
            for network in &app.local_networks {
                let position = net_pos(network, center, radius);
                let is_selected = app.selected_bssid.as_deref() == Some(&network.bssid);
                let is_hovered = hover.map(|p| p.distance(position) <= 14.0).unwrap_or(false);

                if is_hovered {
                    hovered_bssid = Some(network.bssid.clone());
                }

                draw_blip(
                    &painter,
                    network,
                    position,
                    center,
                    sweep,
                    is_selected || is_hovered,
                );
            }

            if response.clicked() {
                if let Some(hovered_bssid) = hovered_bssid {
                    app.selected_bssid = Some(hovered_bssid);
                }
            }

            draw_center_pose_indicator(&painter, center, &app.monitoring_summary);
            draw_hud(
                &painter,
                rect,
                sweep,
                &app.monitoring_summary,
                app.local_connection.as_ref(),
                app.local_networks.first(),
            );
        });
}

fn draw_bg(painter: &Painter, rect: Rect, center: Pos2, radius: f32) {
    painter.rect_filled(rect, 0.0, BG_DARK);

    for ring in [0.22_f32, 0.44, 0.66, 0.88] {
        painter.circle_stroke(center, radius * ring, Stroke::new(0.7, GRID_LINE));
    }

    for deg in (0..360).step_by(45) {
        let angle = deg as f32 / 360.0 * TAU;
        let delta = vec2(angle.cos(), angle.sin()) * radius;
        painter.line_segment(
            [center, center + delta],
            Stroke::new(0.35, tint(GRID_AXIS, 120)),
        );
    }

    // Cardinal directions
    let label_offset = radius + 14.0;
    for (label, angle_deg) in [("N", 270.0_f32), ("E", 0.0), ("S", 90.0), ("W", 180.0)] {
        let a = angle_deg.to_radians();
        let pos = center + vec2(a.cos(), a.sin()) * label_offset;
        painter.text(
            pos,
            Align2::CENTER_CENTER,
            label,
            FontId::monospace(11.0),
            tint(TEXT_DIM, 160),
        );
    }
}

fn draw_zone_overlays(painter: &Painter, center: Pos2, radius: f32, zones: &[f32; 8]) {
    for (i, &activity) in zones.iter().enumerate() {
        if activity > 0.1 {
            let start_angle = (i as f32 * 45.0 - 22.5).to_radians();
            let span = 45.0_f32.to_radians();

            // Draw an arc for active zones
            let segments = 8;
            let step = span / segments as f32;
            let mut pts = Vec::with_capacity(segments + 2);
            pts.push(center);
            for j in 0..=segments {
                let a = start_angle + j as f32 * step;
                pts.push(center + vec2(a.cos(), a.sin()) * radius);
            }

            painter.add(egui::Shape::convex_polygon(
                pts,
                tint(ACCENT, (activity * 15.0) as u8),
                Stroke::NONE,
            ));
        }
    }
}

fn draw_activity_field(painter: &Painter, center: Pos2, radius: f32, hotspots: &[SignalHotspot]) {
    for hotspot in hotspots.iter().take(4) {
        let angle = hotspot.angle.to_radians();
        let position = center + vec2(angle.cos(), angle.sin()) * radius * hotspot.distance_ratio;
        let color = if hotspot.intensity >= 0.72 {
            MOTION_COLOR
        } else {
            ACCENT
        };

        for (scale, alpha) in [(68.0_f32, 18_u8), (44.0, 34), (24.0, 64)] {
            let r = scale * hotspot.intensity.max(0.24);
            let a = ((alpha as f32) * hotspot.intensity.max(0.25)).round() as u8;
            painter.circle_filled(position, r, tint(color, a.max(8)));
        }
        painter.line_segment(
            [center, position],
            Stroke::new(0.5, tint(color, (38.0 * hotspot.intensity) as u8)),
        );
    }
}

fn draw_sweep(painter: &Painter, center: Pos2, radius: f32, angle: f32) {
    let trail = 0.4;
    for index in 0..30 {
        let t = index as f32 / 30.0;
        let diff = angle - trail * t;
        let delta = vec2(diff.cos(), diff.sin()) * radius;
        let alpha = ((1.0 - t) * 58.0) as u8;
        painter.line_segment(
            [center, center + delta],
            Stroke::new(1.5, tint(SWEEP_GREEN, alpha)),
        );
    }
    let tip = center + vec2(angle.cos(), angle.sin()) * radius;
    painter.circle_filled(tip, 2.5, tint(SWEEP_GREEN, 180));
}

fn draw_blip(
    painter: &Painter,
    network: &Network,
    position: Pos2,
    center: Pos2,
    sweep: f32,
    emphasized: bool,
) {
    let strength_ratio = ((network.signal_strength + 90) as f32 / 60.0).clamp(0.0, 1.0);
    let radius = 3.0 + strength_ratio * 5.0;
    let angle = network.angle.to_radians();
    let sweep_alignment = (angle - sweep).cos().max(0.0);

    let color = if network.frequency >= 5.9 {
        BAND_6
    } else if network.frequency >= 4.9 {
        BAND_5
    } else {
        BAND_24
    };
    let dot_alpha = if emphasized {
        230
    } else {
        (60.0 + sweep_alignment * 88.0) as u8
    };

    painter.circle_filled(position, radius * 2.2, tint(color, 20));
    painter.circle_filled(position, radius, tint(color, dot_alpha));

    if emphasized {
        painter.circle_stroke(
            position,
            radius + 2.8,
            Stroke::new(1.0, tint(TEXT_PRIMARY, 90)),
        );
        painter.text(
            position + vec2(11.0, -10.0),
            Align2::LEFT_BOTTOM,
            if network.is_hidden() {
                "<hidden>"
            } else {
                &network.ssid
            },
            FontId::proportional(12.0),
            color,
        );
        painter.text(
            position + vec2(11.0, 3.0),
            Align2::LEFT_TOP,
            format!("{} dBm", network.signal_strength),
            FontId::monospace(10.0),
            sig_color(network.signal_strength),
        );
    }
    painter.line_segment([center, position], Stroke::new(0.3, tint(color, 22)));
}

fn draw_center_pose_indicator(painter: &Painter, center: Pos2, summary: &MonitoringSummary) {
    let icon = summary.pose.posture.icon();
    let pulse = summary.vitals.signal_periodicity * 8.0;

    if pulse > 0.0 {
        painter.circle_stroke(
            center,
            12.0 + pulse,
            Stroke::new(1.5, tint(ACCENT_SOFT, 120)),
        );
    }

    painter.circle_filled(center, 14.0, BG_CARD);
    painter.circle_stroke(center, 14.0, Stroke::new(1.0, GRID_LINE));
    painter.text(
        center,
        Align2::CENTER_CENTER,
        icon,
        FontId::proportional(14.0),
        TEXT_PRIMARY,
    );
}

fn draw_centroid_echo(painter: &Painter, center: Pos2, radius: f32, summary: &MonitoringSummary) {
    if summary.pose.confidence <= 0.01 {
        return;
    }

    let angle = summary.pose.body_centroid_angle.to_radians();
    let distance = summary.pose.body_centroid_distance.clamp(0.08, 1.0) * radius;
    let centroid = center + vec2(angle.cos(), angle.sin()) * distance;
    let alpha = (summary.pose.confidence * 170.0).round() as u8;

    painter.circle_filled(centroid, 4.0, tint(ACCENT_SOFT, alpha.saturating_add(24)));
    painter.circle_stroke(centroid, 10.0, Stroke::new(1.0, tint(ACCENT_SOFT, alpha)));
    painter.line_segment(
        [center, centroid],
        Stroke::new(0.7, tint(ACCENT_SOFT, alpha.saturating_sub(24))),
    );
}

fn draw_hud(
    painter: &Painter,
    rect: Rect,
    sweep: f32,
    summary: &MonitoringSummary,
    _connection: Option<&ConnectionInfo>,
    _strongest: Option<&Network>,
) {
    painter.text(
        pos2(rect.left() + 12.0, rect.top() + 10.0),
        Align2::LEFT_TOP,
        "gojosix.eye net",
        FontId::monospace(12.0),
        tint(ACCENT_SOFT, 190),
    );
    painter.text(
        pos2(rect.left() + 12.0, rect.top() + 28.0),
        Align2::LEFT_TOP,
        "signal observatory / local rf view",
        FontId::monospace(10.0),
        TEXT_DIM,
    );
    painter.text(
        pos2(rect.left() + 12.0, rect.top() + 46.0),
        Align2::LEFT_TOP,
        format!(
            "{} ALERT {}",
            summary.alert_level.icon(),
            summary.alert_level.label().to_ascii_uppercase()
        ),
        FontId::monospace(10.0),
        match summary.alert_level {
            crate::models::AlertLevel::Critical => ALERT_CRITICAL,
            crate::models::AlertLevel::Warning => ALERT_WARNING,
            crate::models::AlertLevel::Attention => ALERT_ATTENTION,
            crate::models::AlertLevel::Normal => SWEEP_GREEN,
        },
    );
    painter.text(
        pos2(rect.left() + 12.0, rect.top() + 64.0),
        Align2::LEFT_TOP,
        format!(
            "{} visible / {} hidden / avg {:.0} dBm",
            summary.visible_networks, summary.hidden_networks, summary.average_signal
        ),
        FontId::monospace(10.0),
        TEXT_DIM,
    );

    painter.text(
        pos2(rect.right() - 12.0, rect.top() + 10.0),
        Align2::RIGHT_TOP,
        format!("🧭 SWEEP {:03.0}°", sweep.to_degrees().rem_euclid(360.0)),
        FontId::monospace(11.0),
        TEXT_DIM,
    );
    painter.text(
        pos2(rect.right() - 12.0, rect.top() + 28.0),
        Align2::RIGHT_TOP,
        format!(
            "CENTROID {:03.0}° / {:.0}%",
            summary.pose.body_centroid_angle,
            (1.0 - summary.pose.body_centroid_distance).clamp(0.0, 1.0) * 100.0
        ),
        FontId::monospace(10.0),
        tint(ACCENT_SOFT, 170),
    );
    painter.text(
        pos2(rect.right() - 12.0, rect.top() + 46.0),
        Align2::RIGHT_TOP,
        format!("STABILITY {:.0}%", summary.signal_health.stability * 100.0),
        FontId::monospace(10.0),
        TEXT_DIM,
    );
}

fn net_pos(network: &Network, center: Pos2, radius: f32) -> Pos2 {
    let angle = network.angle.to_radians();
    let normalized = ((network.signal_strength + 90) as f32 / 60.0).clamp(0.0, 1.0);
    let distance = (1.0 - normalized * 0.82) * radius;
    center + vec2(angle.cos(), angle.sin()) * distance
}
