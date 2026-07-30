use egui::{Color32, CornerRadius};
use serde_json::{json, Value};
use std::sync::mpsc as std_mpsc;

use crate::rpc_client::{DaemonEvent, DaemonState, RpcClient};

#[derive(Debug, Clone, PartialEq)]
enum Tab {
    Control,
    Presets,
    Effects,
}

#[derive(Debug, Clone)]
struct PresetInfo {
    id: u64,
    name: String,
    r: u8,
    g: u8,
    b: u8,
    brightness: u8,
}

pub struct App {
    state: DaemonState,
    rpc: RpcClient,
    event_rx: std_mpsc::Receiver<DaemonEvent>,
    pending_presets_req: bool,
    current_tab: Tab,
    presets: Vec<PresetInfo>,
    save_preset_name: String,
    delete_confirm: Option<u64>,
    connection_status: String,
}

impl App {
    pub fn new(
        rpc: RpcClient,
        event_rx: std_mpsc::Receiver<DaemonEvent>,
    ) -> Self {
        let app = Self {
            state: DaemonState {
                power: false,
                brightness: 100,
                r: 255,
                g: 255,
                b: 255,
                connection: "Disconnected".into(),
                effect: None,
            },
            rpc,
            event_rx,
            pending_presets_req: false,
            current_tab: Tab::Control,
            presets: Vec::new(),
            save_preset_name: String::new(),
            delete_confirm: None,
            connection_status: "Disconnected".into(),
        };
        app.rpc.request("get_state", json!({}));
        app.rpc.request("list_presets", json!({}));
        app
    }

    pub fn drain_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                DaemonEvent::State(s) => {
                    self.state = s;
                }
                DaemonEvent::Connection(c) => {
                    self.connection_status = c;
                }
                DaemonEvent::Response { result, .. } => {
                    if let Some(arr) = result.as_array() {
                        self.presets = arr
                            .iter()
                            .filter_map(|v| {
                                Some(PresetInfo {
                                    id: v.get("id")?.as_u64()?,
                                    name: v.get("name")?.as_str()?.to_string(),
                                    r: v.get("rgb")?.get(0)?.as_u64()? as u8,
                                    g: v.get("rgb")?.get(1)?.as_u64()? as u8,
                                    b: v.get("rgb")?.get(2)?.as_u64()? as u8,
                                    brightness: v.get("brightness")?.as_u64()? as u8,
                                })
                            })
                            .collect();
                        self.pending_presets_req = false;
                    }
                }
                DaemonEvent::Error { message, .. } => {
                    eprintln!("[tui] rpc error: {message}");
                }
                DaemonEvent::Disconnected => {
                    self.connection_status = "Disconnected".into();
                }
            }
        }
    }

    fn send(&self, method: &str, params: Value) {
        self.rpc.request(method, params);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();

        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));

        egui::Panel::top("header").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("l-lightning");
                ui.separator();
                let conn_color = match self.connection_status.as_str() {
                    "Connected" => Color32::GREEN,
                    "Connecting" | "Scanning" => Color32::YELLOW,
                    _ => Color32::RED,
                };
                ui.colored_label(conn_color, format!("● {}", self.connection_status));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.selectable_value(&mut self.current_tab, Tab::Effects, "Effects");
                    ui.selectable_value(&mut self.current_tab, Tab::Presets, "Presets");
                    ui.selectable_value(&mut self.current_tab, Tab::Control, "Control");
                });
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            match self.current_tab {
                Tab::Control => self.show_control(ui),
                Tab::Presets => self.show_presets(ui),
                Tab::Effects => self.show_effects(ui),
            }
        });
    }
}

impl App {
    fn show_control(&mut self, ui: &mut egui::Ui) {
        let state = &self.state;

        ui.horizontal(|ui| {
            ui.heading("Power");
            let mut on = state.power;
            let label = if on { "  ON  " } else { " OFF " };
            if ui.toggle_value(&mut on, label).clicked() {
                self.send("set_power", json!({ "on": on }));
            }
        });

        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Brightness");
            let mut pct = state.brightness as f64;
            let resp = ui.add(
                egui::Slider::new(&mut pct, 0.0..=100.0)
                    .step_by(1.0)
                    .text("brightness")
                    .trailing_fill(true),
            );
            if resp.drag_stopped() || resp.changed() {
                self.send("set_brightness", json!({ "pct": pct as u8 }));
            }
        });

        ui.separator();

        ui.label("Color");
        ui.horizontal(|ui| {
            let mut r = state.r;
            let mut g = state.g;
            let mut b = state.b;

            let color = Color32::from_rgb(r, g, b);

            ui.spacing_mut().slider_width = 120.0;

            let resp_r = ui.add(
                egui::Slider::new(&mut r, 0..=255)
                    .text("R")
                    .trailing_fill(true),
            );
            let resp_g = ui.add(
                egui::Slider::new(&mut g, 0..=255)
                    .text("G")
                    .trailing_fill(true),
            );
            let resp_b = ui.add(
                egui::Slider::new(&mut b, 0..=255)
                    .text("B")
                    .trailing_fill(true),
            );

            let changed = resp_r.drag_stopped() || resp_g.drag_stopped() || resp_b.drag_stopped();
            if changed {
                self.send("set_color", json!({ "r": r, "g": g, "b": b }));
            }

            ui.separator();

            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(60.0, 60.0),
                egui::Sense::hover(),
            );
            ui.painter().rect_filled(rect, CornerRadius::same(8), color);
            ui.painter().rect_stroke(rect, CornerRadius::same(8), egui::Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Inside);

            ui.label(format!("#{:02X}{:02X}{:02X}", state.r, state.g, state.b));
        });

        ui.separator();

        if let Some(ref eff) = state.effect {
            ui.horizontal(|ui| {
                ui.label(format!("Effect: {}", eff));
                if ui.button("Stop").clicked() {
                    self.send("stop_effect", json!({}));
                }
            });
        }
    }

    fn show_presets(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Presets");
            if ui.button("Refresh").clicked() {
                self.pending_presets_req = true;
                self.send("list_presets", json!({}));
            }
        });

        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            let presets = self.presets.clone();
            for preset in presets {
                let color = Color32::from_rgb(preset.r, preset.g, preset.b);
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, CornerRadius::same(3), color);

                    ui.label(&preset.name);
                    ui.label(format!("{}%", preset.brightness));

                    if ui.button("Apply").clicked() {
                        self.send("apply_preset", json!({ "id": preset.id }));
                    }

                    if let Some(confirm_id) = self.delete_confirm {
                        if confirm_id == preset.id {
                            if ui.button("Confirm delete").clicked() {
                                self.send("delete_preset", json!({ "id": preset.id }));
                                self.delete_confirm = None;
                            }
                            if ui.button("Cancel").clicked() {
                                self.delete_confirm = None;
                            }
                        } else {
                            if ui.button("Delete").clicked() {
                                self.delete_confirm = Some(preset.id);
                            }
                        }
                    } else {
                        if ui.button("Delete").clicked() {
                            self.delete_confirm = Some(preset.id);
                        }
                    }
                });
            }
        });

        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Save current as:");
            ui.text_edit_singleline(&mut self.save_preset_name);
            if ui.button("Save").clicked() && !self.save_preset_name.is_empty() {
                let s = &self.state;
                self.send(
                    "save_preset",
                    json!({
                        "name": self.save_preset_name.clone(),
                        "r": s.r,
                        "g": s.g,
                        "b": s.b,
                        "brightness": s.brightness,
                    }),
                );
                self.save_preset_name.clear();
                self.pending_presets_req = true;
                self.send("list_presets", json!({}));
            }
        });
    }

    fn show_effects(&mut self, ui: &mut egui::Ui) {
        ui.heading("Effects");

        let state = &self.state;

        if let Some(ref eff) = state.effect {
            ui.horizontal(|ui| {
                ui.label(format!("Running: {}", eff));
                if ui.button("Stop").clicked() {
                    self.send("stop_effect", json!({}));
                }
            });
            ui.separator();
        }

        let effects = [
            ("Breathe", "breathe"),
            ("Color Cycle", "color_cycle"),
            ("Strobe", "strobe"),
            ("Fade To", "fade_to"),
        ];

        for (label, kind) in effects {
            ui.horizontal(|ui| {
                if ui.button(label).clicked() {
                    let mut params = json!({
                        "kind": kind,
                        "speed": 2000,
                    });
                    if kind == "fade_to" {
                        params["r"] = json!(0);
                        params["g"] = json!(0);
                        params["b"] = json!(0);
                        params["brightness"] = json!(50);
                    }
                    self.send("start_effect", params);
                }
                ui.label(format!("({})", kind));
            });
        }
    }
}
