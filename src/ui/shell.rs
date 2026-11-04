//! Frame around the pages: top bar (profile, device status, STOP), sidebar, status bar and the
//! connection manager card.
use super::*;
use pump_studio::devices::DevId;

impl App{
    pub fn top_bar(&mut self,ui:&mut egui::Ui){
        ui.horizontal(|ui|{
            ui.vertical(|ui|{ui.spacing_mut().item_spacing.y=0.;
                ui.label(RichText::new("Pump Studio").size(19.).strong().color(Color32::WHITE));
                ui.label(RichText::new(format!("Profiel {}",self.profile().name())).size(12.).color(Color32::from_rgb(160,176,198)));});
            ui.add_space(12.);
            for id in DevId::of(self.profile()){
                let st=self.dev.status(*id);let c=link_color(st.link,st.simulation);
                let text=format!("{}  {}",id.short(),link_text(st.link,st.simulation));
                let r=Frame::new().fill(c.gamma_multiply(0.35)).stroke(Stroke::new(W1,c)).corner_radius(CornerRadius::same(12)).inner_margin(Margin::symmetric(9,3))
                    .show(ui,|ui|{ui.label(RichText::new(text).size(13.).color(Color32::WHITE));}).response.interact(egui::Sense::click());
                let r=r.on_hover_text(format!("{}: {}{}\nKlik: naar Instellingen (verbindingen)",id.name(),st.text,if st.endpoint.is_empty(){String::new()}else{format!(" · {}",st.endpoint)}));
                if r.clicked(){self.page=Page::Instellingen;}
            }
            if let Some(w)=self.dev.mixed(){
                Frame::new().fill(DANGER).corner_radius(CornerRadius::same(12)).inner_margin(Margin::symmetric(9,3)).show(ui,|ui|{ui.label(RichText::new("LET OP: simulatie + hardware").strong().size(13.).color(Color32::WHITE)).on_hover_text(w);});
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                if ui.add(egui::Button::new(RichText::new("■  STOP  (Esc)").strong().size(15.).color(Color32::WHITE)).fill(DANGER).min_size(egui::vec2(136.,34.)))
                    .on_hover_text("Onderbreekt elke lopende pompopdracht, PLC-wachtstap en de pompbank (binnen 1 s).").clicked(){self.stop();}
            });
        });
    }
    pub fn sidebar(&mut self,ui:&mut egui::Ui){
        section(ui,"PAGINA'S");
        for p in Page::ALL{
            let (rect,resp)=ui.allocate_exact_size(egui::vec2(ui.available_width(),31.),egui::Sense::click());
            let selected=self.page==p;
            let fill=if selected{Color32::from_rgb(226,237,250)}else if resp.hovered(){Color32::from_rgb(242,245,249)}else{Color32::TRANSPARENT};
            let painter=ui.painter();painter.rect_filled(rect,CornerRadius::same(6),fill);
            if selected{painter.rect_filled(egui::Rect::from_min_size(rect.min,egui::vec2(3.,rect.height())),CornerRadius::same(2),ACCENT);}
            let color=if selected{ACCENT}else{INK};
            painter.text(rect.left_center()+egui::vec2(12.,0.),egui::Align2::LEFT_CENTER,p.label(),egui::FontId::proportional(14.),color);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && self.page!=p{self.click(&format!("Pagina {}",p.label()));self.page=p;let _=self.db.set_meta("page",p.label());}
        }
        ui.add_space(10.);ui.separator();
        section(ui,"PROFIEL");
        ui.horizontal(|ui|{for p in ProfileKind::ALL{if ui.selectable_label(self.profile()==p,p.name()).clicked(){self.switch_profile(p);}}});
        hint(ui,"Wisselen verbreekt eerst de apparaten van het huidige profiel.");
    }
    pub fn status_bar(&mut self,ui:&mut egui::Ui){
        ui.horizontal(|ui|{
            let busy=DevId::of(self.profile()).iter().any(|d|self.dev.busy(*d));
            if busy{ui.spinner();}
            if self.closing.is_some(){ui.strong("Afsluiten: stoppen en verbindingen sluiten…");ui.separator();}
            // One line only: long texts are cut off (full text in Logboek).
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                if !self.message.is_empty() && ui.small_button("Wissen").on_hover_text("Melding wissen").clicked(){self.message.clear();}
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center),|ui|{
                    if !self.message.is_empty(){ui.add(egui::Label::new(RichText::new(&self.message).color(WARN)).truncate());}
                    else if let Some(l)=self.log_lines.last(){ui.add(egui::Label::new(RichText::new(format!("{}  {}",l.area,l.text)).color(MUTED)).truncate());}
                });
            });
        });
    }
    /// Connection manager: every device separately, each with Simulatie or Hardware.
    pub fn connections_card(&mut self,ui:&mut egui::Ui){
        card(ui,"Verbindingen",|ui|{
            egui::Grid::new("conn").num_columns(5).spacing([12.,6.]).show(ui,|ui|{
                for t in ["Apparaat","Status","Adres","Modus",""]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                for id in DevId::of(self.profile()).to_vec(){
                    let st=self.dev.status(id);let on=matches!(st.link,Link::On|Link::Connecting);
                    ui.horizontal(|ui|{dot(ui,link_color(st.link,st.simulation));ui.label(RichText::new(id.name()).strong());});
                    let text=if st.link==Link::Failed{st.text.clone()}else{link_text(st.link,st.simulation).to_string()};
                    ui.add(egui::Label::new(RichText::new(text.clone()).color(if st.link==Link::Failed{DANGER}else{INK})).truncate()).on_hover_text(&st.text);
                    ui.label(RichText::new(if on{st.endpoint.clone()}else{self.planned_endpoint(id)}).size(12.).color(MUTED));
                    // The bench scale runs through the bench bridge: same mode as the bench.
                    let follows=id==DevId::BenchScale;
                    ui.add_enabled_ui(!on && !follows,|ui|{ui.horizontal(|ui|{
                        let mut sim=if follows{self.sim_mode(DevId::Bench)}else{self.sim_mode(id)};
                        ui.selectable_value(&mut sim,true,"Simulatie");ui.selectable_value(&mut sim,false,"Hardware");
                        if !follows{self.mode.insert(id,sim);}
                    });});
                    ui.horizontal(|ui|{
                        if !on{if ui.add_enabled(!st.busy,egui::Button::new("Verbinden")).clicked(){
                            let sim=if follows{self.sim_mode(DevId::Bench)}else{self.sim_mode(id)};
                            self.click(&format!("{} verbinden ({})",id.name(),if sim{"simulatie"}else{"hardware"}));
                            let r=self.dev.connect(id,sim);self.result(r);}}
                        else if ui.add_enabled(!st.busy || st.link==Link::Connecting,egui::Button::new("Verbreken")).clicked(){self.click(&format!("{} verbreken",id.name()));let r=self.dev.disconnect(id);self.result(r);}
                    });
                    ui.end_row();
                }
            });
            ui.add_space(4.);
            let mut mix=self.dev.settings.mix_allowed;
            if ui.checkbox(&mut mix,"Simulatie en hardware tegelijk toestaan (alleen voor tests)").on_hover_text("Standaard uit: een echt apparaat wordt nooit samen met een gesimuleerd apparaat gebruikt. Aan: wel toegestaan, met rode waarschuwing in de kopbalk.").changed(){
                self.click(&format!("Mengen simulatie/hardware {}",if mix{"toestaan"}else{"niet toestaan"}));
                self.dev.settings.mix_allowed=mix;if let Err(e)=self.db.set_meta("mix_allowed",if mix{"1"}else{"0"}){self.message=e;}
            }
            hint(ui,if self.profile()==ProfileKind::Vsd{"Elk apparaat apart; een trage of stille weegschaal blokkeert pompen en PLC niet. Camelot moet gesloten zijn bij hardware (zelfde CAN-borden, PLC-kanaal en weegschalen)."}
                else{"De weegschaal loopt via de koppeling van de pompbank (zelfde modus). Bestaande CAN-app en andere poortgebruikers eerst sluiten bij hardware."});
        });
    }
    fn planned_endpoint(&self,id:DevId)->String{
        let s=&self.dev.settings;
        match id{
            DevId::Can=>format!("kanaal {} · {} bit/s",s.machine.can_channel,s.machine.bitrate),
            DevId::Lh1=>format!("{}:{}",s.machine.scales[0].host.trim(),s.machine.scales[0].port),DevId::Lh2=>format!("{}:{}",s.machine.scales[1].host.trim(),s.machine.scales[1].port),
            DevId::Plc=>format!("{}:{} kanaal {}",s.machine.plc.ip.trim(),s.machine.plc.port,s.machine.plc.channel),
            DevId::Scanner=>s.machine.scanner.endpoint(),
            DevId::Bench=>format!("kanaal {} · node {} motor {}",s.bench.channel,s.bench.node,s.bench.motor),
            DevId::BenchScale=>if s.bench.scale_tcp{format!("{}:{}",s.bench.scale_host.trim(),s.bench.scale_tcp_port)}else{format!("{} {} baud",s.bench.port,s.bench.baud)},
        }
    }
}
