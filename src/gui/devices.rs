use eframe::egui::{self, RichText, Stroke, Ui};

use crate::gui::*;
use crate::models::{ConnectedDevice, DeviceRole};

pub fn draw_devices_tab(app: &mut RadarApp, ui: &mut Ui) {
    ui.add_space(8.0);
    tabs::section_title(ui, "📱 LAN Neighbors");

    ui.horizontal(|ui| {
        ui.label(RichText::new("Search:").size(11.0).color(TEXT_DIM));
        ui.text_edit_singleline(&mut app.device_search);
    });

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Filter:").size(11.0).color(TEXT_DIM));
        for role in ["All", "Gateway", "Router", "Peer", "Unknown"] {
            if ui
                .selectable_label(
                    app.device_role_filter == role,
                    RichText::new(role).size(11.0),
                )
                .clicked()
            {
                app.device_role_filter = role.to_string();
            }
        }
    });

    let filtered: Vec<&ConnectedDevice> = app
        .local_connected_devices
        .iter()
        .filter(|device| matches_search(device, &app.device_search))
        .filter(|device| {
            app.device_role_filter == "All" || device.role.label() == app.device_role_filter
        })
        .collect();

    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        info_chip(ui, &format!("Shown {}", filtered.len()), ACCENT_SOFT);
        info_chip(
            ui,
            &format!(
                "Gateways {}",
                filtered
                    .iter()
                    .filter(|device| device.role == DeviceRole::Gateway)
                    .count()
            ),
            BAND_6,
        );
        info_chip(
            ui,
            &format!(
                "Routers {}",
                filtered
                    .iter()
                    .filter(|device| device.role == DeviceRole::Router)
                    .count()
            ),
            ACCENT,
        );
        info_chip(
            ui,
            &format!(
                "Peers {}",
                filtered
                    .iter()
                    .filter(|device| device.role == DeviceRole::Peer)
                    .count()
            ),
            SWEEP_GREEN,
        );
    });

    ui.add_space(8.0);

    if filtered.is_empty() {
        ui.label(
            RichText::new("No LAN neighbors match this filter.")
                .size(11.0)
                .color(TEXT_SECONDARY),
        );
        return;
    }

    for device in filtered {
        draw_device_card(&mut app.device_expanded, ui, device);
    }
}

fn draw_device_card(
    expanded: &mut std::collections::HashSet<String>,
    ui: &mut Ui,
    device: &ConnectedDevice,
) {
    let (role_emoji, role_color) = match device.role {
        DeviceRole::Gateway => ("🌐", BAND_6),
        DeviceRole::Router => ("📡", ACCENT_SOFT),
        DeviceRole::Peer => ("💻", ACCENT),
        DeviceRole::Unknown => ("❓", TEXT_DIM),
    };

    let is_expanded = expanded.contains(device.primary_address());

    egui::Frame::default()
        .fill(BG_CARD)
        .rounding(10.0)
        .stroke(Stroke::new(
            if is_expanded { 1.3 } else { 1.0 },
            if is_expanded { role_color } else { GRID_LINE },
        ))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} {}", role_emoji, device.role.label()))
                        .size(11.0)
                        .color(role_color),
                );
                ui.label(
                    RichText::new(device.primary_address())
                        .monospace()
                        .size(11.0)
                        .color(TEXT_PRIMARY),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(if is_expanded { "▲" } else { "▼" }).clicked() {
                        if is_expanded {
                            expanded.remove(device.primary_address());
                        } else {
                            expanded.insert(device.primary_address().to_string());
                        }
                    }
                });
            });

            ui.horizontal_wrapped(|ui| {
                info_chip(ui, device.vendor_label(), TEXT_SECONDARY);
                info_chip(ui, &device.state, TEXT_DIM);
                info_chip(ui, &format!("{} addr", device.addresses.len()), role_color);
            });
            ui.label(
                RichText::new(&device.fingerprint)
                    .size(10.5)
                    .color(TEXT_DIM),
            );

            if is_expanded {
                ui.add_space(6.0);
                if let Some(mac) = &device.mac_address {
                    ui.label(
                        RichText::new(format!("MAC: {}", mac))
                            .monospace()
                            .size(10.0)
                            .color(TEXT_DIM),
                    );
                }
                ui.label(
                    RichText::new(format!("State: {} via {}", device.state, device.interface))
                        .monospace()
                        .size(10.0)
                        .color(TEXT_DIM),
                );
                ui.label(
                    RichText::new(format!("IPs: {}", device.address_label()))
                        .monospace()
                        .size(10.0)
                        .color(TEXT_DIM),
                );
            }
        });
    ui.add_space(6.0);
}

fn matches_search(device: &ConnectedDevice, search: &str) -> bool {
    if search.trim().is_empty() {
        return true;
    }

    let search = search.to_lowercase();
    device.primary_address().to_lowercase().contains(&search)
        || device.address_label().to_lowercase().contains(&search)
        || device.vendor_label().to_lowercase().contains(&search)
        || device.fingerprint.to_lowercase().contains(&search)
}

fn info_chip(ui: &mut Ui, text: &str, color: eframe::egui::Color32) {
    egui::Frame::default()
        .fill(BG_CARD)
        .rounding(8.0)
        .stroke(Stroke::new(1.0, GRID_LINE))
        .inner_margin(egui::Margin::symmetric(8.0, 4.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(10.0).color(color));
        });
}
