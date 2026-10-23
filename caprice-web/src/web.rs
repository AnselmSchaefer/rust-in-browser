//! Caprice in a browser tab. The app is the same; what a desktop gives it and a page does not is
//! stood in for here: threads (work runs at once, in the frame that asks for it), the clock (the
//! page's), and file dialogs (there are none yet, so they answer as if cancelled).
//!
//! The page starts the app with `caprice_start` once the module is loaded; it draws into the canvas
//! with the id `caprice_canvas`.

use std::time::SystemTime;

/// Runs `work` on a thread of its own, or, in a browser, right away.
pub fn spawn<F: FnOnce() + Send + 'static>(work: F) {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(work);
    #[cfg(target_arch = "wasm32")]
    work();
}

/// The time now (`SystemTime::now` panics in a browser).
pub fn now() -> SystemTime {
    #[cfg(not(target_arch = "wasm32"))]
    return SystemTime::now();
    #[cfg(target_arch = "wasm32")]
    {
        let ms = eframe::web_sys::js_sys::Date::now();
        SystemTime::UNIX_EPOCH + std::time::Duration::from_micros((ms * 1000.0) as u64)
    }
}

/// This process's id, or 0 in a browser, which has none (`process::id` panics there).
pub fn pid() -> u32 {
    #[cfg(not(target_arch = "wasm32"))]
    return std::process::id();
    #[cfg(target_arch = "wasm32")]
    0
}

/// The directory for temporary files, or none in a browser (`env::temp_dir` panics there).
pub fn temp_dir() -> std::path::PathBuf {
    #[cfg(not(target_arch = "wasm32"))]
    return std::env::temp_dir();
    #[cfg(target_arch = "wasm32")]
    std::path::PathBuf::from("/tmp")
}

/// File dialogs in a browser: none yet, so every dialog is cancelled.
#[cfg(target_arch = "wasm32")]
pub mod rfd {
    use std::path::PathBuf;

    pub struct FileDialog;

    impl FileDialog {
        pub fn new() -> Self {
            FileDialog
        }
        pub fn add_filter(self, _name: &str, _extensions: &[&str]) -> Self {
            self
        }
        pub fn set_file_name(self, _name: impl Into<String>) -> Self {
            self
        }
        pub fn save_file(self) -> Option<PathBuf> {
            None
        }
        pub fn pick_file(self) -> Option<PathBuf> {
            None
        }
    }
}

/// Starts Caprice in the page's canvas `caprice_canvas`.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn caprice_start() {
    use eframe::wasm_bindgen::JsCast;

    let canvas = eframe::web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("caprice_canvas"))
        .and_then(|e| e.dyn_into::<eframe::web_sys::HtmlCanvasElement>().ok())
        .expect("the page has a canvas with the id caprice_canvas");
    // WebGL2 rather than WebGPU: the IDE reads the canvas back for screenshots, which WebGL allows.
    let mut options = eframe::WebOptions::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.instance_descriptor.backends = eframe::wgpu::Backends::GL;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let started = eframe::WebRunner::new()
            .start(canvas, options, Box::new(|cc| Ok(Box::new(crate::new_app(cc)))))
            .await;
        if let Err(e) = started {
            eframe::web_sys::console::error_1(&e);
        }
    });
}
