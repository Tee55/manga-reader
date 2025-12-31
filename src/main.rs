use anyhow::{Context as AnyhowContext, Result};
use eframe::{egui, App, CreationContext, Frame, NativeOptions, run_native};
use egui::{Color32, ColorImage, Rect, Sense, TextureHandle, TextureOptions, Ui, IconData};
use image::{DynamicImage, ImageFormat};
use std::env;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use zip::ZipArchive;
use std::cmp::Ordering;
use std::ffi::OsStr;
use std::os::windows::fs::MetadataExt;

struct MangaReader {
    current_image: Option<TextureHandle>, // Handle to the currently displayed image
    current_path: Option<PathBuf>, // Path of the currently opened file
    files_in_folder: Vec<PathBuf>, // List of image files in the current directory or archive
    current_index: usize, // Index of the currently displayed image
    zoom: f32, // Current zoom level
    offset_x: f32, // Horizontal offset for panning
    offset_y: f32, // Vertical offset for panning
    dragging: bool, // Whether the user is currently dragging the image
    drag_start: Option<egui::Pos2>, // Starting position of the drag
    last_pos: Option<egui::Pos2>, // Last position during dragging
    status_message: Option<(String, f32)>, // Message and duration
    fullscreen: bool, // Whether the app is in fullscreen mode
    auto_fit: bool, // Whether to automatically fit images to view
    archive_files: Vec<PathBuf>, // List of archive files in directory
    current_archive_index: usize, // Index of the currently displayed archive file
    show_last_image_alert: bool, // Whether to show alert when reaching the last image in an archive
    is_in_archive: bool, // Whether the current image is from an archive
}

// Implement natural sorting for filenames
fn natural_sort_paths(a: &Path, b: &Path) -> Ordering {
    let a_name = a
        .file_name()
        .unwrap_or_else(|| OsStr::new(""))
        .to_string_lossy();
    let b_name = b
        .file_name()
        .unwrap_or_else(|| OsStr::new(""))
        .to_string_lossy();

    natural_sort(a_name.as_ref(), b_name.as_ref())
}

fn natural_sort(a: &str, b: &str) -> Ordering {
    let mut a_chars = a.chars().peekable();
    let mut b_chars = b.chars().peekable();

    loop {
        match (a_chars.peek(), b_chars.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a_char), Some(b_char)) => {
                if a_char.is_ascii_digit() && b_char.is_ascii_digit() {
                    // Both are digits, compare as numbers
                    let mut a_num_str = String::new();
                    let mut b_num_str = String::new();

                    // Extract the full number from a
                    while let Some(&ch) = a_chars.peek() {
                        if ch.is_ascii_digit() {
                            a_num_str.push(ch);
                            a_chars.next();
                        } else {
                            break;
                        }
                    }

                    // Extract the full number from b
                    while let Some(&ch) = b_chars.peek() {
                        if ch.is_ascii_digit() {
                            b_num_str.push(ch);
                            b_chars.next();
                        } else {
                            break;
                        }
                    }

                    // Parse and compare as numbers
                    let a_num: u64 = a_num_str.parse().unwrap_or(0);
                    let b_num: u64 = b_num_str.parse().unwrap_or(0);

                    match a_num.cmp(&b_num) {
                        Ordering::Equal => continue, // Numbers are equal, continue comparing
                        other => return other,
                    }
                } else {
                    // Compare as characters
                    let a_ch = a_chars.next().unwrap();
                    let b_ch = b_chars.next().unwrap();

                    // Case-insensitive comparison
                    let a_lower = a_ch.to_lowercase().to_string();
                    let b_lower = b_ch.to_lowercase().to_string();

                    match a_lower.cmp(&b_lower) {
                        Ordering::Equal => continue, // Characters are equal, continue comparing
                        other => return other,
                    }
                }
            }
        }
    }
}

impl Default for MangaReader {
    fn default() -> Self {
        Self {
            current_image: None,
            current_path: None,
            files_in_folder: Vec::new(),
            current_index: 0,
            zoom: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            dragging: false,
            drag_start: None,
            last_pos: None,
            status_message: None,
            fullscreen: false,
            auto_fit: true,
            archive_files: Vec::new(),
            current_archive_index: 0,
            show_last_image_alert: false,
            is_in_archive: false,
        }
    }
}

impl MangaReader {
    fn new(cc: &CreationContext<'_>) -> Self {
        // Get command-line arguments
        let args: Vec<String> = env::args().collect();
        let mut reader = Self::default();
        
        // If there's at least one argument (beyond the program name), try to open it
        if args.len() > 1 {
            // The first argument (index 0) is the program path, so we start from index 1
            let file_path = PathBuf::from(&args[1]);
            if file_path.exists() {
                // Schedule the file to be opened after initialization
                // We need to do this because the UI context isn't fully set up yet
                let _ctx = cc.egui_ctx.clone();
                let _file_path_clone = file_path.clone();
                
                // Use a one-shot timer to open the file after initialization
                cc.egui_ctx.request_repaint();
                
                // Store the path to open in the first update
                reader.current_path = Some(file_path);
            }
        }
        
        reader
    }

    fn set_status(&mut self, message: String, duration: f32) {
        self.status_message = Some((message, duration));
    }

    fn is_archive_file(path: &Path) -> bool {
        if let Some(extension) = path.extension() {
            let ext = extension.to_string_lossy().to_lowercase();
            ext == "cbz" || ext == "zip"
        } else {
            false
        }
    }

    fn list_archive_files_in_directory(&mut self, dir: &Path) -> Result<()> {
        self.archive_files.clear();
        
        for entry in WalkDir::new(dir).max_depth(1).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() && Self::is_archive_file(path) {
                self.archive_files.push(path.to_path_buf());
            }
        }
        
        // Use natural sorting for archive files
        self.archive_files.sort_by(|a, b| natural_sort_paths(a, b));
        
        Ok(())
    }

    fn open_file(&mut self, path: &Path, ctx: &egui::Context) -> Result<()> {
        self.current_path = Some(path.to_path_buf());
        
        // Reset viewing parameters
        self.zoom = 1.0;
        self.offset_x = 0.0;
        self.offset_y = 0.0;
        self.show_last_image_alert = false;
        
        // If path is a directory, list image files
        if path.is_dir() {
            self.is_in_archive = false;
            self.list_image_files_in_directory(path)?;
            if !self.files_in_folder.is_empty() {
                let first_file = self.files_in_folder[0].clone();
                self.current_index = 0;
                self.load_image(&first_file, ctx)
                    .with_context(|| format!("Failed to load first image in directory: {}", first_file.display()))?;
                self.set_status(format!("Opened directory: {}", path.display()), 3.0);
            } else {
                self.set_status(format!("No images found in directory: {}", path.display()), 3.0);
            }
            return Ok(());
        }
        
        // Check if path is a CBZ/ZIP file
        if Self::is_archive_file(path) {
            self.is_in_archive = true;
            // List archive files in the same directory for auto-loading
            if let Some(parent) = path.parent() {
                self.list_archive_files_in_directory(parent)?;
                // Find the index of the current archive
                self.current_archive_index = self.archive_files
                    .iter()
                    .position(|p| p == path)
                    .unwrap_or(0);
            }
            
            self.load_cbz(path, ctx)
                .with_context(|| format!("Failed to load archive: {}", path.display()))?;
            self.set_status(format!("Opened archive: {}", path.display()), 3.0);
            return Ok(());
        }
        
        // Otherwise, assume it's an image file
        self.is_in_archive = false;
        self.load_image(path, ctx)
            .with_context(|| format!("Failed to load image: {}", path.display()))?;
        self.set_status(format!("Opened image: {}", path.display()), 3.0);
        
        // Find other images in the same directory
        if let Some(parent) = path.parent() {
            self.list_image_files_in_directory(parent)?;
            // Find the index of the current file
            self.current_index = self.files_in_folder
                .iter()
                .position(|p| p == path)
                .unwrap_or(0);
        }
        
        Ok(())
    }

    fn list_image_files_in_directory(&mut self, dir: &Path) -> Result<()> {
        self.files_in_folder.clear();
        
        println!("Scanning directory: {}", dir.display());
        
        // Use WalkDir instead of read_dir to access hidden files
        for entry in WalkDir::new(dir)
            .max_depth(1)  // Don't recurse into subdirectories
            .into_iter()
            .filter_map(|e| e.ok()) 
        {
            let path = entry.path();
            
            // Skip the directory itself, only process files
            if !path.is_file() {
                continue;
            }
            
            // Check file attributes to skip hidden/system files (like Cortex XDR decoys)
            if let Ok(metadata) = path.metadata() {
                const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
                const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
                
                let attributes = metadata.file_attributes();
                
                // Skip hidden or system files
                if (attributes & FILE_ATTRIBUTE_HIDDEN) != 0 || (attributes & FILE_ATTRIBUTE_SYSTEM) != 0 {
                    println!("Skipping hidden/system file: {}", path.display());
                    continue;
                }
            }
            
            if let Some(extension) = path.extension() {
                let ext = extension.to_string_lossy().to_lowercase();
                if ["jpg", "jpeg", "png", "webp", "gif"].contains(&ext.as_str()) {
                    println!("Adding: {}", path.display());
                    self.files_in_folder.push(path.to_path_buf());
                }
            }
        }
        
        println!("Found {} images", self.files_in_folder.len());
        
        // Use natural sorting instead of lexicographical sorting
        self.files_in_folder.sort_by(|a, b| natural_sort_paths(a, b));
        
        Ok(())
    }

    fn load_image(&mut self, path: &Path, ctx: &egui::Context) -> Result<()> {
        let img = image::ImageReader::open(path)
            .with_context(|| format!("Failed to open image file: {}", path.display()))?
            .with_guessed_format()
            .with_context(|| format!("Failed to determine image format: {}", path.display()))?
            .decode()
            .with_context(|| format!("Failed to decode image: {}", path.display()))?;
        
        self.set_image(img, ctx);
        Ok(())
    }

    fn load_cbz(&mut self, path: &Path, ctx: &egui::Context) -> Result<()> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut archive = ZipArchive::new(reader)?;
        
        // List all files in the archive
        self.files_in_folder.clear();
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            let name = file.name().to_owned();
            
            // Filter for image files
            if let Some(extension) = Path::new(&name).extension() {
                let ext = extension.to_string_lossy().to_lowercase();
                if ["jpg", "jpeg", "png", "webp", "gif"].contains(&ext.as_str()) {
                    self.files_in_folder.push(PathBuf::from(name));
                }
            }
        }
        
        // Use natural sorting for files in archive
        self.files_in_folder.sort_by(|a, b| {
            let a_name = a.to_string_lossy();
            let b_name = b.to_string_lossy();
            natural_sort(&a_name, &b_name)
        });
        
        // Load the first image if available
        if !self.files_in_folder.is_empty() {
            let first_image = self.files_in_folder[0].clone();
            self.current_index = 0;
            self.load_cbz_image(path, &first_image, ctx)?;
            self.set_status(format!("Loaded archive with {} images", self.files_in_folder.len()), 3.0);
        } else {
            self.set_status("No images found in archive".to_string(), 3.0);
        }
        
        Ok(())
    }

    fn load_cbz_image(&mut self, cbz_path: &Path, image_path: &Path, ctx: &egui::Context) -> Result<()> {
        let file = File::open(cbz_path)?;
        let reader = BufReader::new(file);
        let mut archive = ZipArchive::new(reader)?;
        
        let image_name = image_path.to_string_lossy();
        let mut file = archive.by_name(&image_name)?;
        
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        
        let format = match image_path.extension().and_then(|ext| ext.to_str()) {
            Some("jpg") | Some("jpeg") => ImageFormat::Jpeg,
            Some("png") => ImageFormat::Png,
            Some("webp") => ImageFormat::WebP,
            Some("gif") => ImageFormat::Gif,
            _ => return Err(anyhow::anyhow!("Unsupported image format")),
        };
        
        let img = image::load_from_memory_with_format(&buffer, format)?;
        self.set_image(img, ctx);
        
        Ok(())
    }

    fn set_image(&mut self, img: DynamicImage, ctx: &egui::Context) {
        let size = [img.width() as _, img.height() as _];
        let image_buffer = img.to_rgba8();
        let pixels = image_buffer.as_flat_samples();
        let color_image = ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());
        
        self.current_image = Some(ctx.load_texture(
            "current_image",
            color_image,
            TextureOptions::default(),
        ));
        
        // Auto-fit image if enabled
        if self.auto_fit {
            self.fit_to_view(ctx);
        }
    }

    fn fit_to_view(&mut self, ctx: &egui::Context) {
        if let Some(image) = &self.current_image {
            let image_size = image.size_vec2();
            
            // Get available screen size
            let screen_size = ctx.available_rect().size();
            
            // Calculate zoom required to fit image on screen
            let width_ratio = screen_size.x / image_size.x;
            let height_ratio = screen_size.y / image_size.y;
            
            // Use the smaller ratio to ensure image fits entirely
            self.zoom = width_ratio.min(height_ratio) * 0.9; // 90% of fit size for padding
            
            // Reset offsets
            self.offset_x = 0.0;
            self.offset_y = 0.0;
        }
    }

    fn load_next_archive(&mut self, ctx: &egui::Context) -> Result<bool> {
        if self.archive_files.is_empty() {
            return Ok(false);
        }
        
        let next_index = self.current_archive_index + 1;
        if next_index < self.archive_files.len() {
            let next_archive = self.archive_files[next_index].clone();
            self.current_archive_index = next_index;
            self.current_path = Some(next_archive.clone());
            self.load_cbz(&next_archive, ctx)?;
            self.set_status(format!("Loaded next archive: {}", next_archive.file_name().unwrap_or_default().to_string_lossy()), 3.0);
            Ok(true)
        } else {
            self.set_status("No more archives to load".to_string(), 3.0);
            Ok(false)
        }
    }

    fn next_image(&mut self, ctx: &egui::Context) -> Result<()> {
        if self.files_in_folder.is_empty() {
            return Ok(());
        }
        
        // Check if we're at the last image in an archive
        if self.is_in_archive && self.current_index == self.files_in_folder.len() - 1 {
            if self.show_last_image_alert {
                // Second scroll - try to load next archive
                self.show_last_image_alert = false;
                if !self.load_next_archive(ctx)? {
                    // No more archives, stay at current image
                    return Ok(());
                }
                return Ok(());
            } else {
                // First scroll at last image - show alert
                self.show_last_image_alert = true;
                self.set_status("Reaching last image. Scroll again to load next archive.".to_string(), 3.0);
                return Ok(());
            }
        }
        
        // Normal navigation
        self.show_last_image_alert = false;
        self.current_index = (self.current_index + 1) % self.files_in_folder.len();
        let path = self.files_in_folder[self.current_index].clone();
        
        if let Some(current_path) = &self.current_path {
            let current_path_clone = current_path.clone();
            if self.is_in_archive {
                // Inside a CBZ/ZIP file
                self.load_cbz_image(&current_path_clone, &path, ctx)?;
            } else {
                // Regular image file
                self.load_image(&path, ctx)?;
            }
        }
        
        Ok(())
    }

    fn previous_image(&mut self, ctx: &egui::Context) -> Result<()> {
        if self.files_in_folder.is_empty() {
            return Ok(());
        }
        
        // Reset alert state when going backwards
        self.show_last_image_alert = false;
        
        self.current_index = if self.current_index == 0 {
            self.files_in_folder.len() - 1
        } else {
            self.current_index - 1
        };
        
        let path = self.files_in_folder[self.current_index].clone();
        
        if let Some(current_path) = &self.current_path {
            let current_path_clone = current_path.clone();
            if self.is_in_archive {
                // Inside a CBZ/ZIP file
                self.load_cbz_image(&current_path_clone, &path, ctx)?;
            } else {
                // Regular image file
                self.load_image(&path, ctx)?;
            }
        }
        
        Ok(())
    }
    
    fn handle_keyboard_input(&mut self, ctx: &egui::Context) {
        // Get input outside of any UI closure
        let input = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::Plus) && i.modifiers.ctrl,
                i.key_pressed(egui::Key::Minus) && i.modifiers.ctrl,
                i.key_pressed(egui::Key::F),
                i.key_pressed(egui::Key::F11),
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::End),
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Space)
            )
        });
        
        let (left, right, ctrl_plus, ctrl_minus, f_key, f11_key, home_key, end_key, escape_key, space_key) = input;
        
        // Handle navigation
        if left {
            let _ = self.previous_image(ctx);
        }
        if right || space_key {
            let _ = self.next_image(ctx);
        }
        
        // Handle zoom shortcuts
        if ctrl_plus {
            self.zoom *= 1.2;
        }
        if ctrl_minus {
            self.zoom *= 0.8; 
        }
        
        // Handle fit to view
        if f_key {
            self.fit_to_view(ctx);
        }
        
        // Handle fullscreen toggle
        if f11_key {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        
        // Handle first/last image
        if home_key && !self.files_in_folder.is_empty() {
            self.current_index = 0;
            let path = self.files_in_folder[self.current_index].clone();
            if let Some(current_path) = &self.current_path {
                let current_path_clone = current_path.clone();
                if current_path.extension().map_or(false, |ext| {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    ext_str == "cbz" || ext_str == "zip"
                }) {
                    let _ = self.load_cbz_image(&current_path_clone, &path, ctx);
                } else {
                    let _ = self.load_image(&path, ctx);
                }
            }
        }
        
        if end_key && !self.files_in_folder.is_empty() {
            self.current_index = self.files_in_folder.len() - 1;
            let path = self.files_in_folder[self.current_index].clone();
            if let Some(current_path) = &self.current_path {
                let current_path_clone = current_path.clone();
                if current_path.extension().map_or(false, |ext| {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    ext_str == "cbz" || ext_str == "zip"
                }) {
                    let _ = self.load_cbz_image(&current_path_clone, &path, ctx);
                } else {
                    let _ = self.load_image(&path, ctx);
                }
            }
        }
        
        // Exit fullscreen mode
        if escape_key && self.fullscreen {
            self.fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }
}

impl App for MangaReader {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        // Check if we need to open a file from command line on first update
        if let Some(path) = self.current_path.clone() {
            if self.current_image.is_none() {
                if let Err(e) = self.open_file(&path, ctx) {
                    self.set_status(format!("Error opening file: {}", e), 5.0);
                }
            }
        }
        
        // Handle keyboard input first
        self.handle_keyboard_input(ctx);
        
        // Update status message timer
        if let Some((_, ref mut duration)) = self.status_message {
            *duration -= ctx.input(|i| i.unstable_dt);
            if *duration <= 0.0 {
                self.status_message = None;
            }
        }

        // Show alert modal if at last image
        if self.show_last_image_alert {
            egui::Window::new("Last Image")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label("You've reached the last image in this archive.");
                        ui.add_space(10.0);
                        
                        if self.current_archive_index + 1 < self.archive_files.len() {
                            ui.label("Scroll again to load the next archive:");
                            if let Some(next_archive) = self.archive_files.get(self.current_archive_index + 1) {
                                ui.label(format!("{}", next_archive.file_name().unwrap_or_default().to_string_lossy()));
                            }
                        } else {
                            ui.label("No more archives available.");
                        }
                        
                        ui.add_space(10.0);
                        
                        if ui.button("OK").clicked() {
                            self.show_last_image_alert = false;
                        }
                    });
                });
        }
        
        if !self.fullscreen {
            // Regular view with toolbar
            egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Open File").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Comics & Images", &["jpg", "jpeg", "png", "webp", "gif", "cbz", "zip"])
                            .pick_file() 
                        {
                            if let Err(e) = self.open_file(&path, ctx) {
                                self.set_status(format!("Error: {}", e), 5.0);
                            }
                        }
                    }
                    
                    if ui.button("Open Directory").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            if let Err(e) = self.open_file(&path, ctx) {
                                self.set_status(format!("Error: {}", e), 5.0);
                            }
                        }
                    }
                    
                    ui.separator();
                    
                    if ui.button("Previous (<-)").clicked() { 
                        if let Err(e) = self.previous_image(ctx) {
                            self.set_status(format!("Error: {}", e), 5.0);
                        }
                    }
                    
                    if ui.button("Next (->)").clicked() {
                        if let Err(e) = self.next_image(ctx) {
                            self.set_status(format!("Error: {}", e), 5.0);
                        }
                    }
                    
                    ui.separator();
                    
                    if ui.button("Zoom In (+)").clicked() {
                        self.zoom *= 1.2;
                    }
                    
                    if ui.button("Zoom Out (-)").clicked() {
                        self.zoom *= 0.8;
                    }
                    
                    if ui.button("Fit to View (F)").clicked() {
                        self.fit_to_view(ctx);
                    }
                    
                    let auto_fit_text = if self.auto_fit { "Auto-fit: ON" } else { "Auto-fit: OFF" };
                    if ui.button(auto_fit_text).clicked() {
                        self.auto_fit = !self.auto_fit;
                        if self.auto_fit {
                            self.fit_to_view(ctx);
                        }
                    }
                    
                    ui.separator();
                    
                    if ui.button("Fullscreen (F11)").clicked() {
                        self.fullscreen = !self.fullscreen;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
                    }
                });
            });

            egui::CentralPanel::default().show(ctx, |ui| {
                // Status bar at bottom
                egui::TopBottomPanel::bottom("status_bar").show_inside(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Display current position in folder
                        if !self.files_in_folder.is_empty() {
                            ui.label(format!(
                                "Image {}/{}", 
                                self.current_index + 1, 
                                self.files_in_folder.len()
                            ));
                            
                            // File name
                            if let Some(path) = self.files_in_folder.get(self.current_index) {
                                ui.separator();
                                ui.label(path.file_name().unwrap_or_default().to_string_lossy().to_string());
                            }
                            
                            // Zoom level
                            ui.separator();
                            ui.label(format!("Zoom: {:.0}%", self.zoom * 100.0));
                        }
                        
                        // Show status message if present
                        if let Some((ref message, _)) = self.status_message {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(message);
                            });
                        }
                    });
                });
                
                // Image area
                self.draw_image_view(ui, ctx);
            });
        } else {
            // Fullscreen mode - just the image
            egui::CentralPanel::default().show(ctx, |ui| {
                self.draw_image_view(ui, ctx);
                
                // Show minimal controls in fullscreen mode
                ui.allocate_space(ui.available_size()); // Ensure we can place UI at bottom
                
                // Small overlay at bottom with current image info
                if !self.files_in_folder.is_empty() {
                    egui::containers::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(0, 0, 0, 180))
                        .corner_radius(5.0)
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "Image {}/{} | Zoom: {:.0}% | Press ESC to exit fullscreen", 
                                    self.current_index + 1, 
                                    self.files_in_folder.len(),
                                    self.zoom * 100.0
                                ));
                                
                                // Show status message if present
                                if let Some((ref message, _)) = self.status_message {
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(message);
                                    });
                                }
                            });
                        });
                }
            });
        }
    }
}

impl MangaReader {
    fn draw_image_view(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        // Allocate all available space for the image
        let available_size = ui.available_size();
        let image_rect = Rect::from_min_size(ui.cursor().min, available_size);
        let response = ui.allocate_rect(image_rect, Sense::drag() | Sense::click());
            
        // Handle pan via dragging
        if response.drag_started() {
            self.dragging = true;
            self.drag_start = response.hover_pos();
            self.last_pos = self.drag_start;
        } else if response.dragged() && self.dragging {
            if let (Some(last_pos), Some(hover_pos)) = (self.last_pos, response.hover_pos()) {
                let delta = hover_pos - last_pos;
                self.offset_x += delta.x;
                self.offset_y += delta.y;
                self.last_pos = response.hover_pos();
            }
        } else if response.drag_stopped() {
            self.dragging = false;
            self.drag_start = None;
            self.last_pos = None;
        }
            
        // Handle zoom with mouse wheel
        // Handle mouse wheel - zoom if Ctrl is held, otherwise navigate
        let (scroll, ctrl_held) = ctx.input(|i| (i.raw_scroll_delta.y, i.modifiers.ctrl));
        if scroll != 0.0 {
            if ctrl_held {
                // Zoom functionality when Ctrl is held
                let zoom_factor = if scroll > 0.0 { 1.1 } else { 0.9 };
                let old_zoom = self.zoom;
                self.zoom *= zoom_factor;
                
                // Clamp zoom to reasonable bounds
                self.zoom = self.zoom.clamp(0.1, 10.0);
                
                // Zoom towards mouse cursor position for better UX
                if let Some(hover_pos) = response.hover_pos() {
                    let zoom_change = self.zoom / old_zoom;
                    let center_x = image_rect.center().x;
                    let center_y = image_rect.center().y;
                    
                    // Calculate the point relative to the image center
                    let relative_x = hover_pos.x - center_x - self.offset_x;
                    let relative_y = hover_pos.y - center_y - self.offset_y;
                    
                    // Adjust offset to zoom towards cursor
                    self.offset_x -= relative_x * (zoom_change - 1.0);
                    self.offset_y -= relative_y * (zoom_change - 1.0);
                }
            } else {
                // Navigation functionality when Ctrl is not held
                if scroll > 0.0 {
                    if let Err(e) = self.previous_image(ctx) {
                        self.set_status(format!("Error: {}", e), 5.0);
                    }
                } else {
                    if let Err(e) = self.next_image(ctx) {
                        self.set_status(format!("Error: {}", e), 5.0);
                    }
                }
            }
        }
            
        // Draw the image
        if let Some(image) = &self.current_image {
            let original_size = image.size_vec2();
            let scaled_size = original_size * self.zoom;
                
            let center_x = image_rect.center().x;
            let center_y = image_rect.center().y;
                
            let position = egui::pos2(
                center_x - scaled_size.x / 2.0 + self.offset_x,
                center_y - scaled_size.y / 2.0 + self.offset_y,
            );
                
            let image_rect = Rect::from_min_size(
                position,
                scaled_size,
            );
                
            ui.painter().image(
                image.id(),
                image_rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            
            // Double-click to toggle fullscreen
            if response.double_clicked() {
                self.fullscreen = !self.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            }
        } else {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.heading("No image loaded");
                    ui.label("Use 'Open File' to load an image or comic archive");
                    ui.label("Or 'Open Directory' to browse a folder of images");
                    
                    ui.add_space(20.0);
                    
                    ui.horizontal(|ui| {
                        if ui.button("Open File").clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Comics & Images", &["jpg", "jpeg", "png", "webp", "gif", "cbz", "zip"])
                                .pick_file() 
                            {
                                if let Err(e) = self.open_file(&path, ctx) {
                                    self.set_status(format!("Error: {}", e), 5.0);
                                }
                            }
                        }
                        
                        if ui.button("Open Directory").clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                if let Err(e) = self.open_file(&path, ctx) {
                                    self.set_status(format!("Error: {}", e), 5.0);
                                }
                            }
                        }
                    });
                    
                    ui.add_space(20.0);
                    
                    ui.collapsing("Keyboard Shortcuts", |ui| {
                        ui.label("Arrow Left/Right: Previous/Next image");
                        ui.label("Ctrl+Plus/Minus: Zoom in/out");
                        ui.label("F: Fit image to view");
                        ui.label("F11: Toggle fullscreen");
                        ui.label("Home/End: First/Last image");
                        ui.label("Space: Next image");
                        ui.label("Escape: Exit fullscreen");
                        ui.label("Mouse drag: Pan image");
                        ui.label("Mouse wheel: Zoom in/out");
                        ui.label("Double click: Toggle fullscreen");
                    });
                });
            });
        }
    }
}

fn load_icon() -> Option<IconData> {
    // Embed the icon at compile time
    let icon_bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/icon.png"));
    
    match image::load_from_memory(icon_bytes) {
        Ok(image) => {
            let image = image.into_rgba8();
            let (width, height) = image.dimensions();
            let rgba = image.into_raw();
            
            Some(IconData { rgba, width, height })
        }
        Err(e) => {
            eprintln!("Failed to load icon: {}", e);
            None
        }
    }
}

fn main() -> Result<()> {
    env_logger::init();
    
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1920.0, 1080.0])
        .with_title("Manga Reader")
        .with_maximized(true);
    
    // Add icon if available
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }
    
    let native_options = NativeOptions {
        viewport,
        ..Default::default()
    };
    
    run_native(
        "Manga Reader",
        native_options,
        Box::new(|cc| Ok(Box::new(MangaReader::new(cc)))),
    ).map_err(|e| anyhow::anyhow!("Failed to start application: {}", e))
}

fn main() -> Result<()> {
    env_logger::init();
    
    let native_options = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1920.0, 1080.0])
            .with_title("Manga Reader")
            .with_maximized(true)
            .with_icon(load_icon()),
        ..Default::default()
    };
    
    run_native(
        "Manga Reader",
        native_options,
        Box::new(|cc| Ok(Box::new(MangaReader::new(cc)))),
    ).map_err(|e| anyhow::anyhow!("Failed to start application: {}", e))
}