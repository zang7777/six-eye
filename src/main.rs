mod gui;
mod models;
mod monitoring;
mod oui;
mod pose;
mod scanner;
mod signal_health;
mod vitals;

fn main() -> eframe::Result<()> {
    env_logger::init();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1100.0, 720.0]),
        ..Default::default()
    };

    eframe::run_native(
        "gojosix.eye net // Signal Observatory",
        options,
        Box::new(|cc| Ok(Box::new(gui::RadarApp::new(cc)))),
    )
}
