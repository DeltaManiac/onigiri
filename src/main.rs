use eframe::egui;
use log::{debug, error, info, trace};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::process::{Child, Command};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

const WINDOW_HEIGHT: f32 = 500.0;
const WINDOW_WIDTH: f32 = 400.0;
const APP_NAME: &str = "Onigiri";
static RUNNING: AtomicBool = AtomicBool::new(true);

#[derive(Debug)]
struct TunnelInfo {
    id: i64,
    name: String,
    ssh_server: String,
    local_ip: String,
    local_port: u16,
    remote_ip: String,
    remote_port: u16,
    process: Option<Child>,
}

impl TunnelInfo {
    fn start_tunnel(&mut self) -> Result<(), String> {
        if self.process.is_some() {
            debug!("Tunnel {} is already running", self.name);
            return Ok(());
        }

        debug!(
            "Starting tunnel: {} ({}:{}<-{}:{})",
            self.name, self.local_ip, self.local_port, self.remote_ip, self.remote_port
        );

        let ssh_command = Command::new("ssh")
            .args([
                "-N",
                "-p",
                "22",
                &self.ssh_server,
                "-L",
                &format!(
                    "{}:{}:{}:{}",
                    self.local_ip, self.local_port, self.remote_ip, self.remote_port
                ),
            ])
            .spawn();

        match ssh_command {
            Ok(mut child) => match child.try_wait() {
                Ok(Some(status)) => {
                    error!("Tunnel {} failed to start (status: {})", self.name, status);
                    Err(format!(
                        "SSH process exited immediately with status {}",
                        status
                    ))
                }
                Ok(None) => {
                    info!("Tunnel {} started successfully", self.name);
                    self.process = Some(child);
                    Ok(())
                }
                Err(e) => {
                    error!("Error checking tunnel {} status: {}", self.name, e);
                    Err(format!("Error checking tunnel process: {}", e))
                }
            },
            Err(e) => {
                error!("Failed to start tunnel {}: {}", self.name, e);
                Err(format!("Failed to start tunnel: {}", e))
            }
        }
    }

    fn stop_tunnel(&mut self) {
        if let Some(mut child) = self.process.take() {
            debug!("Stopping tunnel: {}", self.name);
            if let Err(e) = child.kill() {
                error!("Failed to stop tunnel {}: {}", self.name, e);
            } else {
                info!("Tunnel {} stopped successfully", self.name);
            }
        }
    }

    fn is_active(&mut self) -> bool {
        if let Some(child) = &mut self.process {
            match child.try_wait() {
                Ok(Some(_)) => {
                    debug!("Tunnel {} process has exited", self.name);
                    self.process = None;
                    false
                }
                Ok(None) => true,
                Err(e) => {
                    error!("Error checking tunnel {} status: {}", self.name, e);
                    self.process = None;
                    false
                }
            }
        } else {
            false
        }
    }
}

#[derive(Debug, Clone)]
struct Tunnel {
    id: i32,
    name: String,
    command: String,
    ssh_server: String,
    local_ip: String,
    local_port: u16,
    remote_ip: String,
    remote_port: u16,
    active: bool,
    deleted: bool,
}

struct Tunneler {
    tunnels: Vec<Tunnel>,
    active_tunnels: HashMap<i64, TunnelInfo>,
    expanded_tunnels: HashSet<i64>,
    search_query: String,
    show_new_tunnel_window: bool,
    show_edit_tunnel_window: bool,
    new_tunnel: NewTunnelForm,
    edit_tunnel: Option<(i32, NewTunnelForm)>,
}

#[derive(Debug, Clone)]
struct NewTunnelForm {
    name: String,
    ssh_server: String,
    local_ip: String,
    local_port: String,
    remote_ip: String,
    remote_port: String,
    name_error: Option<String>,
    ssh_server_error: Option<String>,
    local_ip_error: Option<String>,
    local_port_error: Option<String>,
    remote_ip_error: Option<String>,
    remote_port_error: Option<String>,
}

impl Default for NewTunnelForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            ssh_server: String::new(),
            local_ip: "127.0.0.1".to_string(),
            local_port: String::new(),
            remote_ip: "127.0.0.1".to_string(),
            remote_port: String::new(),
            name_error: None,
            ssh_server_error: None,
            local_ip_error: None,
            local_port_error: None,
            remote_ip_error: None,
            remote_port_error: None,
        }
    }
}

impl NewTunnelForm {
    fn clear_errors(&mut self) {
        self.name_error = None;
        self.ssh_server_error = None;
        self.local_ip_error = None;
        self.local_port_error = None;
        self.remote_ip_error = None;
        self.remote_port_error = None;
    }

    fn validate(&mut self) -> bool {
        self.clear_errors();
        let mut is_valid = true;

        // Required fields
        if self.name.trim().is_empty() {
            self.name_error = Some("Name is required".to_string());
            is_valid = false;
        }
        if self.ssh_server.trim().is_empty() {
            self.ssh_server_error = Some("SSH Server is required".to_string());
            is_valid = false;
        }
        if self.local_ip.trim().is_empty() {
            self.local_ip_error = Some("Local IP is required".to_string());
            is_valid = false;
        }
        if self.remote_ip.trim().is_empty() {
            self.remote_ip_error = Some("Remote IP is required".to_string());
            is_valid = false;
        }

        // Port validation
        self.local_port_error = match Self::validate_port(&self.local_port) {
            Ok(_) => None,
            Err(e) => {
                is_valid = false;
                Some(e)
            }
        };

        self.remote_port_error = match Self::validate_port(&self.remote_port) {
            Ok(_) => None,
            Err(e) => {
                is_valid = false;
                Some(e)
            }
        };

        is_valid
    }

    fn validate_port(port: &str) -> Result<u16, String> {
        match port.parse::<u16>() {
            Ok(p) if p > 0 => Ok(p),
            Ok(_) => Err("Port must be between 1 and 65535".to_string()),
            Err(_) => Err("Invalid port number".to_string()),
        }
    }
}

impl Tunneler {
    fn new() -> Self {
        debug!("Creating new Tunneler instance");
        let mut app = Self {
            tunnels: Vec::new(),
            active_tunnels: HashMap::new(),
            expanded_tunnels: HashSet::new(),
            search_query: String::new(),
            show_new_tunnel_window: false,
            show_edit_tunnel_window: false,
            new_tunnel: NewTunnelForm::default(),
            edit_tunnel: None,
        };

        // Initialize database and load tunnels
        Self::db();
        app.load_tunnels();
        info!("Application initialized with {} tunnels", app.tunnels.len());
        app
    }

    fn start_edit_tunnel(&mut self, id: i32) {
        if let Some(tunnel) = self.tunnels.iter().find(|t| t.id == id) {
            let form = NewTunnelForm {
                name: tunnel.name.clone(),
                ssh_server: tunnel.ssh_server.clone(),
                local_ip: tunnel.local_ip.clone(),
                local_port: tunnel.local_port.to_string(),
                remote_ip: tunnel.remote_ip.clone(),
                remote_port: tunnel.remote_port.to_string(),
                name_error: None,
                ssh_server_error: None,
                local_ip_error: None,
                local_port_error: None,
                remote_ip_error: None,
                remote_port_error: None,
            };
            self.edit_tunnel = Some((id, form));
            self.show_edit_tunnel_window = true;
        }
    }

    fn save_edited_tunnel(&mut self) -> Result<(), String> {
        if let Some((id, form)) = &self.edit_tunnel {
            let local_port: u16 = form.local_port.parse().unwrap_or(0);
            let remote_port: u16 = form.remote_port.parse().unwrap_or(0);

            let command = format!(
                "ssh -L {}:{}:{} {}",
                local_port, form.remote_ip, remote_port, form.ssh_server
            );

            let conn = Self::db();
            if let Err(e) = conn.execute(
                "UPDATE tunnels SET name = ?1, command = ?2, ssh_server = ?3, local_ip = ?4, local_port = ?5, remote_ip = ?6, remote_port = ?7 WHERE id = ?8",
                params![
                    form.name.trim(),
                    command,
                    form.ssh_server.trim(),
                    form.local_ip.trim(),
                    local_port,
                    form.remote_ip.trim(),
                    remote_port,
                    id,
                ],
            ) {
                return Err(format!("Failed to update tunnel: {}", e));
            }

            // If the tunnel is active, restart it with new settings
            if let Some(tunnel) = self.active_tunnels.get_mut(&(*id as i64)) {
                tunnel.stop_tunnel();
                self.active_tunnels.remove(&(*id as i64));
                if let Err(e) = self.toggle_tunnel(*id as i64) {
                    return Err(format!("Failed to restart tunnel: {}", e));
                }
            }

            self.load_tunnels();
            self.show_edit_tunnel_window = false;
            self.edit_tunnel = None;
        }
        Ok(())
    }

    fn db() -> Connection {
        debug!("Initializing database connection");
        let home_dir = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("No home dir")).unwrap();
        let db_path = home_dir.join("Library").join("Application Support").join("Onigiri");
        std::fs::create_dir_all(&db_path).unwrap();
        let db_file = db_path.join("ssh_tunnels.db");
    
        let conn = Connection::open(db_file).unwrap();
        trace!("Checking if tables are present");
        let table_exists = conn
            .query_row(
                "SELECT name from sqlite_master WHERE type='table' and name='tunnels'",
                [],
                |row| row.get::<usize, String>(0),
            )
            .optional()
            .unwrap();
        if table_exists.is_none() {
            info!("First time setup: Creating tunnels table");
            conn.execute(
                "CREATE TABLE IF NOT EXISTS tunnels (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                command TEXT NOT NULL,
                ssh_server TEXT NOT NULL,
                local_ip TEXT NOT NULL,
                local_port INTEGER NOT NULL,
                remote_ip TEXT NOT NULL,
                remote_port INTEGER NOT NULL,
                active BOOLEAN NOT NULL DEFAULT 0,
                deleted BOOLEAN NOT NULL DEFAULT 0
            )",
                [],
            )
            .unwrap();

            debug!("Creating sample tunnels");
            let sample_tunnels = vec![
                (
                    "Local MySQL",
                    "ssh -L 3306:localhost:3306 user@db-server",
                    "db-server",
                    "127.0.0.1",
                    3306,
                    "localhost",
                    3306,
                    false,
                    false,
                ),
                (
                    "Dev MongoDB",
                    "ssh -L 27017:mongodb:27017 user@dev-server",
                    "dev-server",
                    "127.0.0.1",
                    27017,
                    "mongodb",
                    27017,
                    false,
                    false,
                ),
                (
                    "Staging API",
                    "ssh -L 8080:api-internal:80 user@staging",
                    "staging",
                    "127.0.0.1",
                    8080,
                    "api-internal",
                    80,
                    false,
                    false,
                ),
            ];

            for tunnel in &sample_tunnels {
                debug!("Creating sample tunnel: {}", tunnel.0);
                conn.execute(
                    "INSERT INTO tunnels (name, command, ssh_server, local_ip, local_port, remote_ip, remote_port, active, deleted)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        tunnel.0,
                        tunnel.1,
                        tunnel.2,
                        tunnel.3,
                        tunnel.4,
                        tunnel.5,
                        tunnel.6,
                        tunnel.7,
                        tunnel.8,
                    ],
                )
                .unwrap();
            }

            info!(
                "Database initialized with {} sample tunnels",
                sample_tunnels.len()
            );
        } else {
            trace!("Database already initialized");
        }
        conn
    }

    fn load_tunnels(&mut self) {
        debug!("Loading tunnels from database");
        let conn = Self::db();
        let mut stmt = conn
            .prepare("SELECT id, name, command, ssh_server, local_ip, local_port, remote_ip, remote_port, active, deleted FROM tunnels WHERE deleted = 0")
            .unwrap();

        let tunnel_iter = stmt
            .query_map([], |row| {
                Ok(Tunnel {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    command: row.get(2)?,
                    ssh_server: row.get(3)?,
                    local_ip: row.get(4)?,
                    local_port: row.get(5)?,
                    remote_ip: row.get(6)?,
                    remote_port: row.get(7)?,
                    active: row.get(8)?,
                    deleted: row.get(9)?,
                })
            })
            .unwrap();

        self.tunnels = tunnel_iter.filter_map(Result::ok).collect();
        info!("Loaded {} active tunnels", self.tunnels.len());
    }

    fn toggle_tunnel(&mut self, id: i64) -> Result<(), String> {
        let conn = Self::db();
        let mut tunnel = conn.query_row(
            "SELECT id, name, ssh_server, local_ip, local_port, remote_ip, remote_port FROM tunnels WHERE id = ?",
            [id],
            |row| -> Result<TunnelInfo, rusqlite::Error> {
                Ok(TunnelInfo {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    ssh_server: row.get(2)?,
                    local_ip: row.get(3)?,
                    local_port: row.get(4)?,
                    remote_ip: row.get(5)?,
                    remote_port: row.get(6)?,
                    process: None,
                })
            },
        ).map_err(|e| format!("Failed to load tunnel: {}", e))?;

        if let Some(existing_tunnel) = self.active_tunnels.get_mut(&id) {
            existing_tunnel.stop_tunnel();
            self.active_tunnels.remove(&id);
            debug!("Tunnel stopped: {}", tunnel.name);
            Ok(())
        } else {
            tunnel.start_tunnel()?;
            self.active_tunnels.insert(id, tunnel);
            debug!(
                "Tunnel started: {}",
                self.active_tunnels.get(&id).unwrap().name
            );
            Ok(())
        }
    }

    fn update_tunnel_status(&mut self) {
        let mut inactive_tunnels = Vec::new();

        for (id, tunnel) in &mut self.active_tunnels {
            if !tunnel.is_active() {
                inactive_tunnels.push(*id);
                error!("Tunnel {} is no longer active", tunnel.name);
            }
        }

        for id in inactive_tunnels {
            if let Some(tunnel) = self.active_tunnels.remove(&id) {
                error!("Tunnel {} died unexpectedly", tunnel.name);
                if let Some(ui_tunnel) = self.tunnels.iter_mut().find(|t| t.id as i64 == id) {
                    debug!("Updated UI state for tunnel {}", ui_tunnel.name);
                }
            }
        }
    }

    fn add_new_tunnel(&mut self) -> Result<(), rusqlite::Error> {
        debug!("Adding new tunnel: {}", self.new_tunnel.name);
        let conn = Self::db();
        let local_port: u16 = self.new_tunnel.local_port.parse().unwrap_or(0);
        let remote_port: u16 = self.new_tunnel.remote_port.parse().unwrap_or(0);

        let command = format!(
            "ssh -L {}:{}:{} {}",
            local_port, self.new_tunnel.remote_ip, remote_port, self.new_tunnel.ssh_server
        );

        conn.execute(
            "INSERT INTO tunnels (name, command, ssh_server, local_ip, local_port, remote_ip, remote_port, active, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                self.new_tunnel.name.trim(),
                command,
                self.new_tunnel.ssh_server.trim(),
                self.new_tunnel.local_ip.trim(),
                local_port,
                self.new_tunnel.remote_ip.trim(),
                remote_port,
                false,
                false,
            ],
        )?;

        info!("New tunnel '{}' added successfully", self.new_tunnel.name);
        self.new_tunnel = NewTunnelForm::default();
        self.load_tunnels();
        Ok(())
    }

    fn delete_tunnel(&mut self, id: i32) -> Result<(), String> {
        debug!("Marking tunnel {} as deleted", id);

        if let Some(tunnel) = self.active_tunnels.get_mut(&(id as i64)) {
            tunnel.stop_tunnel();
            self.active_tunnels.remove(&(id as i64));
        }

        let conn = Self::db();
        conn.execute(
            "UPDATE tunnels SET deleted = TRUE WHERE id = ?1",
            params![id],
        )
        .map_err(|e| format!("Failed to mark tunnel as deleted: {}", e))?;

        if let Some(tunnel) = self.tunnels.iter_mut().find(|t| t.id == id) {
            tunnel.deleted = true;
            debug!("Tunnel {} marked as deleted", id);
        }

        Ok(())
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) {
            info!("Window close requested");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        
        self.update_tunnel_status();

        // Collect all the data we need upfront
        #[derive(Clone)]
        struct TunnelDisplayData {
            id: i32,
            name: String,
            is_active: bool,
            is_expanded: bool,
            local_ip: String,
            local_port: u16,
            remote_ip: String,
            remote_port: u16,
            pid: Option<u32>,
        }

        let tunnel_data: Vec<TunnelDisplayData> = self.tunnels.iter()
            .filter(|t| !t.deleted && (self.search_query.is_empty() || 
                t.name.to_lowercase().contains(&self.search_query.to_lowercase())))
            .map(|t| {
                let is_active = self.active_tunnels.contains_key(&(t.id as i64));
                let is_expanded = self.expanded_tunnels.contains(&(t.id as i64));
                let pid = if is_active {
                    self.active_tunnels.get(&(t.id as i64))
                        .and_then(|info| info.process.as_ref())
                        .map(|process| process.id())
                } else {
                    None
                };
                
                TunnelDisplayData {
                    id: t.id,
                    name: t.name.clone(),
                    is_active,
                    is_expanded,
                    local_ip: t.local_ip.clone(),
                    local_port: t.local_port,
                    remote_ip: t.remote_ip.clone(),
                    remote_port: t.remote_port,
                    pid,
                }
            })
            .collect();

        let mut tunnel_to_toggle = None;
        let mut tunnel_to_delete = None;
        let mut tunnel_to_toggle_expand = None;
        let mut tunnel_to_edit = None;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.heading("SSH Tunnel Manager");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Add Tunnel").clicked() {
                            self.show_new_tunnel_window = true;
                        }
                    });
                });

                // Search box
                ui.horizontal(|ui| {
                    ui.label("Search:");
                    ui.text_edit_singleline(&mut self.search_query);
                });

                ui.separator();

                // Tunnels list
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for tunnel in &tunnel_data {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                // Draw status circle
                                let color = if tunnel.is_active {
                                    egui::Color32::from_rgb(50, 205, 50) // Green
                                } else {
                                    egui::Color32::from_rgb(220, 50, 50) // Red
                                };
                                let circle_size = 8.0;
                                let (rect, _response) = ui.allocate_exact_size(
                                    egui::vec2(circle_size, circle_size),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(rect.center(), circle_size / 2.0, color);
                                ui.add_space(4.0); // Add a small gap between circle and name

                                ui.label(&tunnel.name);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("Delete").clicked() {
                                        tunnel_to_delete = Some(tunnel.id);
                                    }
                                    let toggle_text = if tunnel.is_active { "Stop" } else { "Start" };
                                    if ui.small_button(toggle_text).clicked() {
                                        tunnel_to_toggle = Some(tunnel.id as i64);
                                    }
                                    let info_text = if tunnel.is_expanded { "Hide" } else { "Info" };
                                    if ui.small_button(info_text).clicked() {
                                        tunnel_to_toggle_expand = Some(tunnel.id as i64);
                                    }
                                    if ui.small_button("Edit").clicked() {
                                        tunnel_to_edit = Some(tunnel.id);
                                    }
                                });
                            });

                            // Show expanded details
                            if tunnel.is_expanded {
                                ui.indent("details", |ui| {
                                    if let Some(pid) = tunnel.pid {
                                        ui.label(format!("PID: {}", pid));
                                    }
                                    ui.label(format!(
                                        "Local: {}:{} -> Remote: {}:{}",
                                        tunnel.local_ip, tunnel.local_port,
                                        tunnel.remote_ip, tunnel.remote_port
                                    ));
                                });
                            }
                            ui.separator();
                        });
                    }
                });
            });
        });

        // Handle actions after UI
        if let Some(id) = tunnel_to_toggle {
            if let Err(e) = self.toggle_tunnel(id) {
                error!("Failed to toggle tunnel: {}", e);
            }
        }

        if let Some(id) = tunnel_to_delete {
            if let Err(e) = self.delete_tunnel(id) {
                error!("Failed to delete tunnel: {}", e);
            }
        }

        if let Some(id) = tunnel_to_toggle_expand {
            if self.expanded_tunnels.contains(&id) {
                self.expanded_tunnels.remove(&id);
            } else {
                self.expanded_tunnels.insert(id);
            }
        }

        if let Some(id) = tunnel_to_edit {
            self.start_edit_tunnel(id);
        }

        if self.show_new_tunnel_window {
            self.show_new_tunnel_window(ctx);
        }

        if self.show_edit_tunnel_window {
            self.show_edit_tunnel_window(ctx);
        }
    }

    fn show_new_tunnel_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("Add New Tunnel")
            .fixed_size([300.0, 250.0])
            .collapsible(false)
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.text_edit_singleline(&mut self.new_tunnel.name);
                    });
                    if let Some(error) = &self.new_tunnel.name_error {
                        ui.colored_label(egui::Color32::RED, error);
                    }

                    ui.horizontal(|ui| {
                        ui.label("SSH Server:");
                        ui.text_edit_singleline(&mut self.new_tunnel.ssh_server);
                    });
                    if let Some(error) = &self.new_tunnel.ssh_server_error {
                        ui.colored_label(egui::Color32::RED, error);
                    }

                    ui.horizontal(|ui| {
                        ui.label("Local IP:");
                        ui.text_edit_singleline(&mut self.new_tunnel.local_ip);
                    });
                    if let Some(error) = &self.new_tunnel.local_ip_error {
                        ui.colored_label(egui::Color32::RED, error);
                    }

                    ui.horizontal(|ui| {
                        ui.label("Local Port:");
                        ui.text_edit_singleline(&mut self.new_tunnel.local_port);
                    });
                    if let Some(error) = &self.new_tunnel.local_port_error {
                        ui.colored_label(egui::Color32::RED, error);
                    }

                    ui.horizontal(|ui| {
                        ui.label("Remote IP:");
                        ui.text_edit_singleline(&mut self.new_tunnel.remote_ip);
                    });
                    if let Some(error) = &self.new_tunnel.remote_ip_error {
                        ui.colored_label(egui::Color32::RED, error);
                    }

                    ui.horizontal(|ui| {
                        ui.label("Remote Port:");
                        ui.text_edit_singleline(&mut self.new_tunnel.remote_port);
                    });
                    if let Some(error) = &self.new_tunnel.remote_port_error {
                        ui.colored_label(egui::Color32::RED, error);
                    }

                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.show_new_tunnel_window = false;
                            self.new_tunnel = NewTunnelForm::default();
                        }

                        if ui.button("Add").clicked() {
                            if self.new_tunnel.validate() {
                                if let Err(e) = self.add_new_tunnel() {
                                    error!("Failed to add tunnel: {}", e);
                                } else {
                                    self.show_new_tunnel_window = false;
                                    self.new_tunnel = NewTunnelForm::default();
                                }
                            }
                        }
                    });
                });
            });
    }

    fn show_edit_tunnel_window(&mut self, ctx: &egui::Context) {
        let mut should_save = false;
        let mut should_close = false;

        if let Some((_id, form)) = &mut self.edit_tunnel {
            egui::Window::new("Edit Tunnel")
                .fixed_size([300.0, 250.0])
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label("Name:");
                            ui.text_edit_singleline(&mut form.name);
                        });
                        if let Some(error) = &form.name_error {
                            ui.colored_label(egui::Color32::RED, error);
                        }

                        ui.horizontal(|ui| {
                            ui.label("SSH Server:");
                            ui.text_edit_singleline(&mut form.ssh_server);
                        });
                        if let Some(error) = &form.ssh_server_error {
                            ui.colored_label(egui::Color32::RED, error);
                        }

                        ui.horizontal(|ui| {
                            ui.label("Local IP:");
                            ui.text_edit_singleline(&mut form.local_ip);
                        });
                        if let Some(error) = &form.local_ip_error {
                            ui.colored_label(egui::Color32::RED, error);
                        }

                        ui.horizontal(|ui| {
                            ui.label("Local Port:");
                            ui.text_edit_singleline(&mut form.local_port);
                        });
                        if let Some(error) = &form.local_port_error {
                            ui.colored_label(egui::Color32::RED, error);
                        }

                        ui.horizontal(|ui| {
                            ui.label("Remote IP:");
                            ui.text_edit_singleline(&mut form.remote_ip);
                        });
                        if let Some(error) = &form.remote_ip_error {
                            ui.colored_label(egui::Color32::RED, error);
                        }

                        ui.horizontal(|ui| {
                            ui.label("Remote Port:");
                            ui.text_edit_singleline(&mut form.remote_port);
                        });
                        if let Some(error) = &form.remote_port_error {
                            ui.colored_label(egui::Color32::RED, error);
                        }

                        ui.add_space(8.0);

                        ui.horizontal(|ui| {
                            if ui.button("Cancel").clicked() {
                                should_close = true;
                            }

                            if ui.button("Save").clicked() {
                                if form.validate() {
                                    should_save = true;
                                }
                            }
                        });
                    });
                });
        }

        if should_save {
            if let Err(e) = self.save_edited_tunnel() {
                error!("Failed to save edited tunnel: {}", e);
            }
        } else if should_close {
            self.show_edit_tunnel_window = false;
            self.edit_tunnel = None;
        }
    }
}

impl Drop for Tunneler {
    fn drop(&mut self) {
        info!("Application shutting down, cleaning up tunnel processes...");
        for (id, tunnel) in self.active_tunnels.iter_mut() {
            info!("Stopping tunnel {}", id);
            tunnel.stop_tunnel();
        }
        info!("All tunnels stopped");
    }
}

impl eframe::App for Tunneler {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.update(ctx, frame);
    }
}

fn main() -> Result<(), eframe::Error> {
    std::env::set_var("RUST_LOG","info,onigiri=debug");
    pretty_env_logger::init_timed();
    info!("Starting {} application", APP_NAME);
    debug!("Window dimensions: {}x{}", WINDOW_WIDTH, WINDOW_HEIGHT);

    let icon = image::load_from_memory(include_bytes!("../resources/icon.png")).unwrap().to_rgba8();
    let (icon_width, icon_height) = icon.dimensions();
    let icon = Arc::new(egui::IconData {
        rgba: icon.into_raw(),
        width: icon_width,
        height: icon_height,
    });

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
            .with_resizable(false)
            .with_icon(icon),
        ..Default::default()
    };

    // Run the app
    let result = eframe::run_native(
        APP_NAME,
        options,
        Box::new(|_cc| Ok(Box::new(Tunneler::new()))),
    );

    info!("Application terminated");
    result
}

