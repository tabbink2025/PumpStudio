//! Logboek: tail of the application log (day file stays primary; lines are also indexed in SQLite),
//! filter on text, level and area, open log folder, copy visible lines.
use super::*;
use pump_studio::applog::Level;

impl App{
    pub fn log_page(&mut self,ui:&mut egui::Ui){
        let dir=applog::dir();
        page_title(ui,"Logboek",&format!("Elke klik, opdracht, antwoord en fout van pompen, CAN, PLC, weegschalen en bridges. Dagbestand: {}",dir.as_ref().map(|d|d.display().to_string()).unwrap_or("—".into())));
        let f=self.log_filter.trim().to_lowercase();let min=[Level::Info,Level::Warn,Level::Error][self.log_level.min(2)];
        let mut areas:Vec<&str>=self.log_lines.iter().map(|l|l.area.as_str()).collect();areas.sort();areas.dedup();
        let shown:Vec<&applog::Line>=self.log_lines.iter().filter(|l|l.level>=min && (self.log_area.is_empty() || l.area==self.log_area) && (f.is_empty() || l.render().to_lowercase().contains(&f))).collect();
        let mut copy=None;let mut open=false;
        card(ui,"",|ui|{ui.horizontal_wrapped(|ui|{
            ui.add(egui::TextEdit::singleline(&mut self.log_filter).hint_text("filter, bv. P_A01, proces 1350, fout").desired_width(220.));
            for (i,l) in ["Alles","Waarschuwing + fout","Alleen fouten"].iter().enumerate(){ui.selectable_value(&mut self.log_level,i,*l);}
            egui::ComboBox::from_id_salt("log-area").width(120.).selected_text(if self.log_area.is_empty(){"Alle gebieden"}else{&self.log_area}).show_ui(ui,|ui|{
                ui.selectable_value(&mut self.log_area,String::new(),"Alle gebieden");for a in &areas{ui.selectable_value(&mut self.log_area,a.to_string(),*a);}});
            ui.separator();
            if ui.button("Open logmap").clicked(){open=true;}
            if ui.button("Kopieer zichtbare regels").clicked(){copy=Some(shown.iter().map(|l|l.render()).collect::<Vec<_>>().join("\n"));}
            hint(ui,&format!("{} van {} regels",shown.len(),self.log_lines.len()));
        });});
        if open{self.click("Open logmap");if let Some(d)=&dir{let _=std::process::Command::new("explorer.exe").arg(d).spawn();}}
        if let Some(t)=copy{self.click("Kopieer zichtbare logregels");ui.ctx().copy_text(t);}
        ui.add_space(8.);
        card(ui,"",|ui|{
            if shown.is_empty(){empty(ui,"Geen regels bij dit filter.",120.);return;}
            let h=ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::both().id_salt("log-lines").max_height((ui.available_height()-10.).max(320.)).stick_to_bottom(true).auto_shrink([false,false])
                .show_rows(ui,h,shown.len(),|ui,range|{for l in &shown[range]{
                    let c=match l.level{Level::Error=>DANGER,Level::Warn=>WARN,Level::Info=>INK};
                    ui.label(RichText::new(l.render()).monospace().size(12.).color(c));
                }});
        });
    }
}
