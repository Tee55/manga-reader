use std::env;

fn main() {
    let target = env::var("TARGET").unwrap();
    
    if target.contains("windows") {
        let mut res = winres::WindowsResource::new();
        
        // Add icon
        res.set_icon("resources/icon.ico");
        
        // Add version information
        res.set("FileDescription", "Manga Reader");
        res.set("ProductName", "Manga Reader");
        res.set("LegalCopyright", "© 2025 Teerapath Sattabongkot");
        res.set("FileVersion", env!("CARGO_PKG_VERSION"));
        res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
        
        // Compile and link
        res.compile().unwrap();
    }
}