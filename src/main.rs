#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod ui;
mod smoke;
use eframe::egui;

fn main()->eframe::Result{
    if std::env::args().nth(1).as_deref()==Some("--smoke-test"){
        let dir=std::env::args().nth(2).map(std::path::PathBuf::from).unwrap_or_else(||std::env::temp_dir().join("pump-rust-smoke"));
        match smoke::run(&dir){
            Ok(report)=>{let _=std::fs::write(dir.join("smoke-ok.txt"),report);std::process::exit(0);}
            Err(e)=>{let _=std::fs::create_dir_all(&dir);let _=std::fs::write(dir.join("smoke-error.txt"),&e);eprintln!("{e}");std::process::exit(1);}
        }
    }
    let options=eframe::NativeOptions{viewport:egui::ViewportBuilder::default().with_title("Pump Studio").with_inner_size([1366.,768.]).with_min_inner_size([1000.,640.]),..Default::default()};
    eframe::run_native("Pump Studio",options,Box::new(|cc|Ok(Box::new(ui::App::new(cc)))))
}
