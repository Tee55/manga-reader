use anyhow::{Context as AnyhowContext, Result};
use eframe::{egui, run_native, App, CreationContext, Frame, NativeOptions};
use egui::{Color32, ColorImage, IconData, Rect, Sense, TextureHandle, TextureOptions, Ui};
use image::{DynamicImage, ImageFormat};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use zip::ZipArchive;

struct MangaReader {
    current_image: Option<TextureHandle>,
    current_path: Option<PathBuf>,
    files_in_folder: Vec<PathBuf>,
    current_index: usize,
    zoom: f32,
    offset_x: f32,
    offset_y: f32,
    dragging: bool,
    drag_start: Option<egui::Pos2>,
    last_pos: Option<egui::Pos2>,
    status_message: Option<(String, f32)>,
    fullscreen: bool,
    auto_fit: bool,
    archive_files: Vec<PathBuf>,
    current_archive_index: usize,
    show_last_image_alert: bool,
    is_in_archive: bool,
    show_delete_confirmation: bool,
    pending_delete_path: Option<PathBuf>,
    // Thumbnail support
    thumbnail_cache: HashMap<PathBuf, TextureHandle>,
    thumbnail_size: usize,
    show_thumbnail_panel: bool,
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
                    let mut a_num_str = String::new();
                    let mut b_num_str = String::new();

                    while let Some(&ch) = a_chars.peek() {
                        if ch.is_ascii_digit() {
                            a_num_str.push(ch);
                            a_chars.next();
                        } else {
                            break;
                        }
                    }

                    while let Some(&ch) = b_chars.peek() {
                        if ch.is_ascii_digit() {
                            b_num_str.push(ch);
                            b_chars.next();
                        } else {
                            break;
                        }
                    }

                    let a_num: u64 = a_num_str.parse().unwrap_or(0);
                    let b_num: u64 = b_num_str.parse().unwrap_or(0);

                    match a_num.cmp(&b_num) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                } else {
                    let a_ch = a_chars.next().unwrap();
                    let b_ch = b_chars.next().unwrap();

                    let a_lower = a_ch.to_lowercase().to_string();
                    let b_lower = b_ch.to_lowercase().to_string();

                    match a_lower.cmp(&b_lower) {
                        Ordering::Equal => continue,
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
            show_delete_confirmation: false,
            pending_delete_path: None,
            thumbnail_cache: HashMap::new(),
            thumbnail_size: 200,
            show_thumbnail_panel: false,
        }
    }
}

impl MangaReader {
    fn new(cc: &CreationContext<'_>) -> Self {
        let args: Vec<String> = env::args().collect();
        let mut reader = Self::default();

        if args.len() > 1 {
            let file_path = PathBuf::from(&args[1]);
            if file_path.exists() {
                let _ctx = cc.egui_ctx.clone();
                let _file_path_clone = file_path.clone();

                cc.egui_ctx.request_repaint();

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

        for entry in WalkDir::new(dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.is_file() && Self::is_archive_file(path) {
                self.archive_files.push(path.to_path_buf());
            }
        }

        self.archive_files.sort_by(|a, b| natural_sort_paths(a, b));

        Ok(())
    }

    fn generate_thumbnail(&self, path: &Path) -> Result<DynamicImage> {
        if Self::is_archive_file(path) {
            self.generate_archive_thumbnail(path)
        } else {
            self.generate_image_thumbnail(path)
        }
    }

    fn generate_image_thumbnail(&self, path: &Path) -> Result<DynamicImage> {
        let img = image::ImageReader::open(path)
            .with_context(|| format!("Failed to open image: {}", path.display()))?
            .with_guessed_format()
            .with_context(|| format!("Failed to determine format: {}", path.display()))?
            .decode()
            .with_context(|| format!("Failed to decode image: {}", path.display()))?;

        let thumbnail = img.thumbnail(self.thumbnail_size as u32, self.thumbnail_size as u32);
        Ok(thumbnail)
    }

    fn generate_archive_thumbnail(&self, path: &Path) -> Result<DynamicImage> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut archive = ZipArchive::new(reader)?;

        let mut image_files = Vec::new();
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            let name = file.name().to_owned();

            if let Some(extension) = Path::new(&name).extension() {
                let ext = extension.to_string_lossy().to_lowercase();
                if ["jpg", "jpeg", "png", "webp", "gif"].contains(&ext.as_str()) {
                    image_files.push((i, name));
                }
            }
        }

        image_files.sort_by(|a, b| natural_sort(&a.1, &b.1));

        if image_files.is_empty() {
            return Err(anyhow::anyhow!("No images found in archive"));
        }

        // Get the first image
        let (first_idx, first_name) = &image_files[0];
        let mut file = archive.by_index(*first_idx)?;

        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;

        let format = match Path::new(first_name)
            .extension()
            .and_then(|ext| ext.to_str())
        {
            Some("jpg") | Some("jpeg") => ImageFormat::Jpeg,
            Some("png") => ImageFormat::Png,
            Some("webp") => ImageFormat::WebP,
            Some("gif") => ImageFormat::Gif,
            _ => return Err(anyhow::anyhow!("Unsupported image format")),
        };

        let img = image::load_from_memory_with_format(&buffer, format)?;
        let thumbnail = img.thumbnail(self.thumbnail_size as u32, self.thumbnail_size as u32);
        Ok(thumbnail)
    }

    fn load_thumbnail(&mut self, path: &Path, ctx: &egui::Context) {
        if self.thumbnail_cache.contains_key(path) {
            return;
        }

        if let Ok(thumbnail) = self.generate_thumbnail(path) {
            let size = [thumbnail.width() as _, thumbnail.height() as _];
            let image_buffer = thumbnail.to_rgba8();
            let pixels = image_buffer.as_flat_samples();
            let color_image = ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());

            let texture = ctx.load_texture(
                format!("thumb_{}", path.display()),
                color_image,
                TextureOptions::default(),
            );

            self.thumbnail_cache.insert(path.to_path_buf(), texture);
        }
    }

    fn load_visible_thumbnails(&mut self, ctx: &egui::Context) {
        // Load thumbnails for current and nearby files
        let start_idx = self.current_index.saturating_sub(5);
        let end_idx = (self.current_index + 10).min(self.files_in_folder.len());

        // Clone paths to avoid borrow conflicts
        let paths_to_load: Vec<PathBuf> = self.files_in_folder[start_idx..end_idx].to_vec();
        for path in paths_to_load {
            self.load_thumbnail(&path, ctx);
        }

        // Also load archive thumbnails if we have them
        if !self.is_in_archive {
            let start_archive = self.current_archive_index.saturating_sub(3);
            let end_archive = (self.current_archive_index + 6).min(self.archive_files.len());

            let archive_paths: Vec<PathBuf> =
                self.archive_files[start_archive..end_archive].to_vec();
            for path in archive_paths {
                self.load_thumbnail(&path, ctx);
            }
        }
    }

    fn open_file(&mut self, path: &Path, ctx: &egui::Context) -> Result<()> {
        self.current_path = Some(path.to_path_buf());

        self.zoom = 1.0;
        self.offset_x = 0.0;
        self.offset_y = 0.0;
        self.show_last_image_alert = false;

        if path.is_dir() {
            self.is_in_archive = false;
            self.list_image_files_in_directory(path)?;
            if !self.files_in_folder.is_empty() {
                let first_file = self.files_in_folder[0].clone();
                self.current_index = 0;
                self.load_image(&first_file, ctx).with_context(|| {
                    format!(
                        "Failed to load first image in directory: {}",
                        first_file.display()
                    )
                })?;
                self.set_status(format!("Opened directory: {}", path.display()), 3.0);
            } else {
                self.set_status(
                    format!("No images found in directory: {}", path.display()),
                    3.0,
                );
            }
            return Ok(());
        }

        if Self::is_archive_file(path) {
            self.is_in_archive = true;
            if let Some(parent) = path.parent() {
                self.list_archive_files_in_directory(parent)?;
                self.current_archive_index = self
                    .archive_files
                    .iter()
                    .position(|p| p == path)
                    .unwrap_or(0);
            }

            self.load_cbz(path, ctx)
                .with_context(|| format!("Failed to load archive: {}", path.display()))?;
            self.set_status(format!("Opened archive: {}", path.display()), 3.0);
            return Ok(());
        }

        self.is_in_archive = false;
        self.load_image(path, ctx)
            .with_context(|| format!("Failed to load image: {}", path.display()))?;
        self.set_status(format!("Opened image: {}", path.display()), 3.0);

        if let Some(parent) = path.parent() {
            self.list_image_files_in_directory(parent)?;
            self.current_index = self
                .files_in_folder
                .iter()
                .position(|p| p == path)
                .unwrap_or(0);
        }

        Ok(())
    }

    fn list_image_files_in_directory(&mut self, dir: &Path) -> Result<()> {
        self.files_in_folder.clear();

        println!("Scanning directory: {}", dir.display());

        for entry in WalkDir::new(dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            if let Ok(metadata) = path.metadata() {
                const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
                const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

                let attributes = metadata.file_attributes();

                if (attributes & FILE_ATTRIBUTE_HIDDEN) != 0
                    || (attributes & FILE_ATTRIBUTE_SYSTEM) != 0
                {
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

        self.files_in_folder
            .sort_by(|a, b| natural_sort_paths(a, b));

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

        self.files_in_folder.clear();
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            let name = file.name().to_owned();

            if let Some(extension) = Path::new(&name).extension() {
                let ext = extension.to_string_lossy().to_lowercase();
                if ["jpg", "jpeg", "png", "webp", "gif"].contains(&ext.as_str()) {
                    self.files_in_folder.push(PathBuf::from(name));
                }
            }
        }

        self.files_in_folder.sort_by(|a, b| {
            let a_name = a.to_string_lossy();
            let b_name = b.to_string_lossy();
            natural_sort(&a_name, &b_name)
        });

        if !self.files_in_folder.is_empty() {
            let first_image = self.files_in_folder[0].clone();
            self.current_index = 0;
            self.load_cbz_image(path, &first_image, ctx)?;
            self.set_status(
                format!("Loaded archive with {} images", self.files_in_folder.len()),
                3.0,
            );
        } else {
            self.set_status("No images found in archive".to_string(), 3.0);
        }

        Ok(())
    }

    fn load_cbz_image(
        &mut self,
        cbz_path: &Path,
        image_path: &Path,
        ctx: &egui::Context,
    ) -> Result<()> {
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

        self.current_image =
            Some(ctx.load_texture("current_image", color_image, TextureOptions::default()));

        if self.auto_fit {
            self.fit_to_view(ctx);
        }
    }

    fn fit_to_view(&mut self, ctx: &egui::Context) {
        if let Some(image) = &self.current_image {
            let image_size = image.size_vec2();
            let screen_size = ctx.available_rect().size();
            let width_ratio = screen_size.x / image_size.x;
            let height_ratio = screen_size.y / image_size.y;
            self.zoom = width_ratio.min(height_ratio) * 0.9;
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
            self.set_status(
                format!(
                    "Loaded next archive: {}",
                    next_archive
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ),
                3.0,
            );
            Ok(true)
        } else {
            self.set_status("No more archives to load".to_string(), 3.0);
            Ok(false)
        }
    }

    fn delete_current_file(&mut self, ctx: &egui::Context) -> Result<()> {
        if self.is_in_archive {
            self.set_status("Cannot delete files inside archives".to_string(), 3.0);
            return Ok(());
        }

        if self.files_in_folder.is_empty() {
            return Ok(());
        }

        let file_to_delete = self.files_in_folder[self.current_index].clone();

        fs::remove_file(&file_to_delete)
            .with_context(|| format!("Failed to delete file: {}", file_to_delete.display()))?;

        self.set_status(
            format!(
                "Deleted: {}",
                file_to_delete
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ),
            3.0,
        );

        self.files_in_folder.remove(self.current_index);

        if !self.files_in_folder.is_empty() {
            if self.current_index >= self.files_in_folder.len() {
                self.current_index = self.files_in_folder.len() - 1;
            }

            let next_file = self.files_in_folder[self.current_index].clone();
            self.load_image(&next_file, ctx)?;
        } else {
            self.current_image = None;
            self.set_status("No more images in directory".to_string(), 3.0);
        }

        Ok(())
    }

    fn next_image(&mut self, ctx: &egui::Context) -> Result<()> {
        if self.files_in_folder.is_empty() {
            return Ok(());
        }

        if self.is_in_archive && self.current_index == self.files_in_folder.len() - 1 {
            if self.show_last_image_alert {
                self.show_last_image_alert = false;
                if !self.load_next_archive(ctx)? {
                    return Ok(());
                }
                return Ok(());
            } else {
                self.show_last_image_alert = true;
                self.set_status(
                    "Reaching last image. Scroll again to load next archive.".to_string(),
                    3.0,
                );
                return Ok(());
            }
        }

        self.show_last_image_alert = false;
        self.current_index = (self.current_index + 1) % self.files_in_folder.len();
        let path = self.files_in_folder[self.current_index].clone();

        if let Some(current_path) = &self.current_path {
            let current_path_clone = current_path.clone();
            if self.is_in_archive {
                self.load_cbz_image(&current_path_clone, &path, ctx)?;
            } else {
                self.load_image(&path, ctx)?;
            }
        }

        Ok(())
    }

    fn previous_image(&mut self, ctx: &egui::Context) -> Result<()> {
        if self.files_in_folder.is_empty() {
            return Ok(());
        }

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
                self.load_cbz_image(&current_path_clone, &path, ctx)?;
            } else {
                self.load_image(&path, ctx)?;
            }
        }

        Ok(())
    }

    fn handle_keyboard_input(&mut self, ctx: &egui::Context) {
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
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::Delete),
                i.key_pressed(egui::Key::T),
            )
        });

        let (
            left,
            right,
            ctrl_plus,
            ctrl_minus,
            f_key,
            f11_key,
            home_key,
            end_key,
            escape_key,
            space_key,
            delete_key,
            t_key,
        ) = input;

        if left {
            let _ = self.previous_image(ctx);
        }
        if right || space_key {
            let _ = self.next_image(ctx);
        }

        if ctrl_plus {
            self.zoom *= 1.2;
        }
        if ctrl_minus {
            self.zoom *= 0.8;
        }

        if f_key {
            self.fit_to_view(ctx);
        }

        if f11_key {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }

        if t_key {
            self.show_thumbnail_panel = !self.show_thumbnail_panel;
        }

        if delete_key && !self.files_in_folder.is_empty() {
            self.show_delete_confirmation = true;
            self.pending_delete_path = Some(self.files_in_folder[self.current_index].clone());
        }

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

        if escape_key {
            if self.fullscreen {
                self.fullscreen = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            } else if self.show_thumbnail_panel {
                self.show_thumbnail_panel = false;
            }
        }
    }
}

impl App for MangaReader {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        if let Some(path) = self.current_path.clone() {
            if self.current_image.is_none() {
                if let Err(e) = self.open_file(&path, ctx) {
                    self.set_status(format!("Error opening file: {}", e), 5.0);
                }
            }
        }

        self.handle_keyboard_input(ctx);

        if self.show_thumbnail_panel {
            self.load_visible_thumbnails(ctx);
        }

        if let Some((_, ref mut duration)) = self.status_message {
            *duration -= ctx.input(|i| i.unstable_dt);
            if *duration <= 0.0 {
                self.status_message = None;
            }
        }

        if self.show_delete_confirmation {
            egui::Window::new("Confirm Delete")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label("Are you sure you want to delete this file?");

                        if let Some(path) = &self.pending_delete_path {
                            ui.add_space(10.0);
                            ui.label(format!(
                                "{}",
                                path.file_name().unwrap_or_default().to_string_lossy()
                            ));
                            ui.add_space(10.0);
                        }

                        ui.horizontal(|ui| {
                            if ui.button("Yes, Delete").clicked() {
                                if let Err(e) = self.delete_current_file(ctx) {
                                    self.set_status(format!("Error deleting file: {}", e), 5.0);
                                }
                                self.show_delete_confirmation = false;
                                self.pending_delete_path = None;
                            }

                            if ui.button("Cancel").clicked() {
                                self.show_delete_confirmation = false;
                                self.pending_delete_path = None;
                            }
                        });
                    });
                });
        }
        // CONTINUATION FROM: if self.show_delete_confirmation { ... }

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
                            if let Some(next_archive) =
                                self.archive_files.get(self.current_archive_index + 1)
                            {
                                ui.label(format!(
                                    "{}",
                                    next_archive
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                ));
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
            egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Open File").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter(
                                "Comics & Images",
                                &["jpg", "jpeg", "png", "webp", "gif", "cbz", "zip"],
                            )
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

                    let auto_fit_text = if self.auto_fit {
                        "Auto-fit: ON"
                    } else {
                        "Auto-fit: OFF"
                    };
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

                    ui.separator();

                    let thumbnail_text = if self.show_thumbnail_panel {
                        "Hide Thumbnails (T)"
                    } else {
                        "Show Thumbnails (T)"
                    };
                    if ui.button(thumbnail_text).clicked() {
                        self.show_thumbnail_panel = !self.show_thumbnail_panel;
                    }
                });
            });

            egui::CentralPanel::default().show(ctx, |ui| {
                egui::TopBottomPanel::bottom("status_bar").show_inside(ui, |ui| {
                    ui.horizontal(|ui| {
                        if !self.files_in_folder.is_empty() {
                            ui.label(format!(
                                "Image {}/{}",
                                self.current_index + 1,
                                self.files_in_folder.len()
                            ));

                            if let Some(path) = self.files_in_folder.get(self.current_index) {
                                ui.separator();
                                ui.label(
                                    path.file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .to_string(),
                                );
                            }

                            ui.separator();
                            ui.label(format!("Zoom: {:.0}%", self.zoom * 100.0));
                        }

                        if let Some((ref message, _)) = self.status_message {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(message);
                                },
                            );
                        }
                    });
                });

                if self.show_thumbnail_panel {
                    egui::SidePanel::right("thumbnail_panel")
                        .default_width(250.0)
                        .show_inside(ui, |ui| {
                            ui.heading("Thumbnails");
                            ui.separator();

                            egui::ScrollArea::vertical().show(ui, |ui| {
                                if self.is_in_archive {
                                    ui.label("Archive Contents");
                                    ui.add_space(5.0);
                                }

                                // Clone the data we need to avoid borrow conflicts
                                let files: Vec<(usize, PathBuf)> = self
                                    .files_in_folder
                                    .iter()
                                    .enumerate()
                                    .map(|(idx, path)| (idx, path.clone()))
                                    .collect();

                                let current_idx = self.current_index;

                                for (idx, file_path) in files.iter() {
                                    let is_current = *idx == current_idx;

                                    let frame = egui::Frame::default()
                                        .fill(if is_current {
                                            Color32::from_rgb(50, 50, 80)
                                        } else {
                                            Color32::TRANSPARENT
                                        })
                                        .corner_radius(5.0)
                                        .inner_margin(5.0);

                                    frame.show(ui, |ui| {
                                        ui.vertical(|ui| {
                                            let thumb_size = egui::vec2(200.0, 200.0);

                                            if let Some(texture) =
                                                self.thumbnail_cache.get(file_path)
                                            {
                                                let response = ui.add(
                                                    egui::Image::new(texture)
                                                        .fit_to_exact_size(thumb_size)
                                                        .sense(Sense::click()),
                                                );

                                                if response.clicked() {
                                                    self.current_index = *idx; // Note the * here
                                                    if let Some(current_path) = &self.current_path {
                                                        let current_path_clone =
                                                            current_path.clone();
                                                        if self.is_in_archive {
                                                            let _ = self.load_cbz_image(
                                                                &current_path_clone,
                                                                file_path,
                                                                ctx,
                                                            );
                                                        } else {
                                                            let _ = self.load_image(file_path, ctx);
                                                        }
                                                    }
                                                }
                                            } else {
                                                ui.allocate_space(thumb_size);
                                                ui.label("Loading...");
                                            }

                                            ui.label(format!("{}", idx + 1));
                                            ui.label(
                                                file_path
                                                    .file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy()
                                                    .to_string(),
                                            );
                                        });
                                    });

                                    ui.add_space(5.0);
                                }

                                if !self.is_in_archive && !self.archive_files.is_empty() {
                                    ui.separator();
                                    ui.heading("Archives in Directory");
                                    ui.add_space(5.0);

                                    let archives: Vec<(usize, PathBuf)> = self
                                        .archive_files
                                        .iter()
                                        .enumerate()
                                        .map(|(idx, path)| (idx, path.clone()))
                                        .collect();

                                    let current_archive_idx = self.current_archive_index;

                                    for (idx, archive_path) in archives.iter() {
                                        let is_current = *idx == current_archive_idx;

                                        let frame = egui::Frame::default()
                                            .fill(if is_current {
                                                Color32::from_rgb(50, 50, 80)
                                            } else {
                                                Color32::TRANSPARENT
                                            })
                                            .corner_radius(5.0)
                                            .inner_margin(5.0);

                                        frame.show(ui, |ui| {
                                            ui.vertical(|ui| {
                                                let thumb_size = egui::vec2(200.0, 200.0);

                                                if let Some(texture) =
                                                    self.thumbnail_cache.get(archive_path)
                                                {
                                                    let response = ui.add(
                                                        egui::Image::new(texture)
                                                            .fit_to_exact_size(thumb_size)
                                                            .sense(Sense::click()),
                                                    );

                                                    if response.clicked() {
                                                        let _ = self.open_file(archive_path, ctx);
                                                    }
                                                } else {
                                                    ui.allocate_space(thumb_size);
                                                    ui.label("Loading...");
                                                }

                                                ui.label(
                                                    archive_path
                                                        .file_name()
                                                        .unwrap_or_default()
                                                        .to_string_lossy()
                                                        .to_string(),
                                                );
                                            });
                                        });

                                        ui.add_space(5.0);
                                    }
                                }
                            });
                        });
                }

                self.draw_image_view(ui, ctx);
            });
        } else {
            egui::CentralPanel::default().show(ctx, |ui| {
                self.draw_image_view(ui, ctx);

                ui.allocate_space(ui.available_size());

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

                                if let Some((ref message, _)) = self.status_message {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(message);
                                        },
                                    );
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
        let available_size = ui.available_size();
        let image_rect = Rect::from_min_size(ui.cursor().min, available_size);
        let response = ui.allocate_rect(image_rect, Sense::drag() | Sense::click());

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

        let (scroll, ctrl_held) = ctx.input(|i| (i.raw_scroll_delta.y, i.modifiers.ctrl));
        if scroll != 0.0 {
            if ctrl_held {
                let zoom_factor = if scroll > 0.0 { 1.1 } else { 0.9 };
                let old_zoom = self.zoom;
                self.zoom *= zoom_factor;

                self.zoom = self.zoom.clamp(0.1, 10.0);

                if let Some(hover_pos) = response.hover_pos() {
                    let zoom_change = self.zoom / old_zoom;
                    let center_x = image_rect.center().x;
                    let center_y = image_rect.center().y;

                    let relative_x = hover_pos.x - center_x - self.offset_x;
                    let relative_y = hover_pos.y - center_y - self.offset_y;

                    self.offset_x -= relative_x * (zoom_change - 1.0);
                    self.offset_y -= relative_y * (zoom_change - 1.0);
                }
            } else {
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

        if let Some(image) = &self.current_image {
            let original_size = image.size_vec2();
            let scaled_size = original_size * self.zoom;

            let center_x = image_rect.center().x;
            let center_y = image_rect.center().y;

            let position = egui::pos2(
                center_x - scaled_size.x / 2.0 + self.offset_x,
                center_y - scaled_size.y / 2.0 + self.offset_y,
            );

            let image_rect = Rect::from_min_size(position, scaled_size);

            ui.painter().image(
                image.id(),
                image_rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );

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
                                .add_filter(
                                    "Comics & Images",
                                    &["jpg", "jpeg", "png", "webp", "gif", "cbz", "zip"],
                                )
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
                        ui.label("T: Toggle thumbnail panel");
                        ui.label("Home/End: First/Last image");
                        ui.label("Space: Next image");
                        ui.label("Delete: Delete current image");
                        ui.label("Escape: Exit fullscreen/Close panels");
                        ui.label("Mouse drag: Pan image");
                        ui.label("Mouse wheel: Navigate images");
                        ui.label("Ctrl+Mouse wheel: Zoom in/out");
                        ui.label("Double click: Toggle fullscreen");
                    });
                });
            });
        }
    }
}

fn load_icon() -> Option<IconData> {
    let icon_bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/icon.png"));

    match image::load_from_memory(icon_bytes) {
        Ok(image) => {
            let image = image.into_rgba8();
            let (width, height) = image.dimensions();
            let rgba = image.into_raw();

            Some(IconData {
                rgba,
                width,
                height,
            })
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
    )
    .map_err(|e| anyhow::anyhow!("Failed to start application: {}", e))
}
