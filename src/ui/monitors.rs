//! Monitors: live weight per head (or the bench scale), CAN bus scan, PLC messages.
use super::*;
use pump_studio::{devices::DevId,machine::Head,workflow::Action};

impl App{
    pub fn monitors_page(&mut self,ui:&mut egui::Ui){
        page_title(ui,"Monitors","Live gewicht, CAN-bus test, PLC-berichten en scannertest. Elk apparaat leest op een eigen achtergrondtaak; een stille weegschaal houdt niets anders op.");
        self.connections_card(ui);ui.add_space(8.);
        if self.profile()==ProfileKind::Vsd{
            ui.columns(2,|c|{for (k,h) in Head::ALL.into_iter().enumerate(){self.scale_card(&mut c[k],h);}});
        }else{self.bench_scale_card(ui);}
        ui.add_space(8.);
        let wide=ui.available_width()>=900.;
        if wide{ui.columns(2,|c|{self.scan_card(&mut c[0]);if self.profile()==ProfileKind::Vsd{self.plc_card(&mut c[1]);}});}
        else{self.scan_card(ui);if self.profile()==ProfileKind::Vsd{ui.add_space(8.);self.plc_card(ui);}}
        if self.profile()==ProfileKind::Vsd{ui.add_space(8.);self.scanner_card(ui);}
    }
    fn scale_card(&mut self,ui:&mut egui::Ui,h:Head){
        let id=if h==Head::LH1{DevId::Lh1}else{DevId::Lh2};let s=self.dev.status(id);
        card(ui,&format!("Weegschaal {}",h.name()),|ui|{
            ui.horizontal(|ui|{dot(ui,link_color(s.link,s.simulation));ui.label(format!("{} · {}",link_text(s.link,s.simulation),if s.endpoint.is_empty(){"—"}else{&s.endpoint}));});
            if s.link!=Link::On{empty(ui,"Niet verbonden",70.);return;}
            let age=s.weight_at.map(|t|t.elapsed().as_secs_f64());
            ui.label(RichText::new(s.weight.map(|w|format!("{:.4} g",w-s.tare)).unwrap_or("—".into())).size(26.).strong().monospace().color(if age.is_some_and(|a|a<2.){INK}else{MUTED}));
            ui.horizontal(|ui|{
                ui.label(RichText::new(if s.weight.is_none(){"geen waarde"}else if s.stable{"stabiel"}else{"beweegt"}).strong().color(if s.stable{OK}else{WARN}));
                hint(ui,&format!("netto · bruto {} · tarra {:.4} g{}",grams(s.weight),s.tare,age.map(|a|format!(" · {a:.1} s geleden")).unwrap_or_default()));
            });
            if !s.weight_error.is_empty(){ui.label(RichText::new(&s.weight_error).color(DANGER));}
            ui.add_enabled_ui(!s.busy,|ui|{ui.horizontal(|ui|{
                if ui.button("Tarra").clicked(){self.click(&format!("{} tarra",h.name()));let r=self.dev.scale(h,true);self.result(r);}
                if ui.button("Stabiel wegen").clicked(){self.click(&format!("{} stabiel wegen",h.name()));let r=self.dev.scale(h,false);self.result(r);}
                if s.busy{ui.spinner();}
            });});
            result_line(ui,if s.text=="Verbonden"{""}else{&s.text},s.ok);
        });
    }
    fn bench_scale_card(&mut self,ui:&mut egui::Ui){
        let s=self.dev.status(DevId::BenchScale);let b=self.dev.bench.clone();
        card(ui,"Weegschaal pompbank",|ui|{
            ui.horizontal(|ui|{dot(ui,link_color(s.link,s.simulation));ui.label(format!("{} · {}",link_text(s.link,s.simulation),if s.endpoint.is_empty(){"—"}else{&s.endpoint}));});
            if s.link!=Link::On{empty(ui,"Niet verbonden",70.);return;}
            ui.label(RichText::new(grams(b.live_g)).size(26.).strong().monospace());
            ui.horizontal(|ui|{
                if b.live_on{ui.label(RichText::new(if b.live_g.is_none(){"geen waarde"}else if b.live_stable{"stabiel"}else{"beweegt"}).strong().color(if b.live_stable{OK}else{WARN}));}
                else{hint(ui,"Live meten staat uit");}
            });
            if !b.live_error.is_empty(){ui.label(RichText::new(&b.live_error).color(DANGER));}
            ui.add_enabled_ui(!s.busy,|ui|{ui.horizontal(|ui|{
                let mut on=b.live_on;
                if ui.checkbox(&mut on,"Live meten").on_hover_text("Leest de weegschaal ~3 keer per seconde als de pompbank niets doet. Met hardware bezet elke weging de koppeling even.").changed(){
                    self.click(&format!("Live wegen {}",if on{"aan"}else{"uit"}));let r=self.dev.bench_action(Action::LiveWeight(on));self.result(r);}
            });});
            hint(ui,"Weging met referentie: Pompen > Weegschaal lezen.");
        });
    }
    fn scan_card(&mut self,ui:&mut egui::Ui){
        let s=self.dev.status(DevId::Can);
        card(ui,"CAN-bus test",|ui|{
            hint(ui,"Luistert op alle IXXAT-kanalen zonder iets te versturen en meldt welke borden een hartslag geven. Hardware: CAN en pompbank eerst verbreken.");
            let mut sim=self.sim_mode(DevId::Can);
            ui.horizontal(|ui|{
                ui.selectable_value(&mut sim,true,"Simulatie");ui.selectable_value(&mut sim,false,"Hardware");
                ui.label("Luisteren");ui.add(egui::DragValue::new(&mut self.scan_s).range(1. ..=40.).suffix(" s"));
            });
            if !matches!(s.link,Link::On|Link::Connecting){self.mode.insert(DevId::Can,sim);}
            ui.horizontal(|ui|{
                if ui.add_enabled(!s.busy,egui::Button::new("Scan starten")).clicked(){
                    self.click(&format!("CAN-bus testen ({}, {:.0} s)",if sim{"simulatie"}else{"hardware"},self.scan_s));let r=self.dev.scan(sim,self.scan_s);self.result(r);}
                if s.busy && s.scan.contains("loopt"){ui.spinner();}
            });
            if s.scan.is_empty(){empty(ui,"Nog geen test gedaan.",50.);}else{ui.label(RichText::new(&s.scan).color(if s.scan.contains("mislukt"){DANGER}else{INK}));}
        });
    }
    fn plc_card(&mut self,ui:&mut egui::Ui){
        let s=self.dev.status(DevId::Plc);
        card(ui,"PLC-berichten",|ui|{
            ui.horizontal(|ui|{dot(ui,link_color(s.link,s.simulation));ui.label(format!("PLC: {}",link_text(s.link,s.simulation)));if s.busy{ui.spinner();}});
            if s.messages.is_empty(){empty(ui,"Nog geen berichten.",60.);}
            else{egui::ScrollArea::vertical().id_salt("plc-msg").max_height(170.).show(ui,|ui|{for m in s.messages.iter().rev().take(100){ui.label(RichText::new(m).monospace().size(12.));}});}
            ui.add_space(4.);section(ui,"OPDRACHT VERSTUREN (DIAGNOSE)");
            ui.horizontal_wrapped(|ui|{
                ui.label("Proces");ui.add(egui::DragValue::new(&mut self.plc_process).range(1..=9999));
                // P5: a real PLC only gets Reset/Abort until iteration 5; free commands in simulation.
                let allowed:&[u32]=if s.simulation || s.link!=Link::On{&[2,5,1,8,11,12]}else{&[2,5]};
                if !allowed.contains(&self.plc_cmd){self.plc_cmd=2;}
                ui.label("Opdracht");egui::ComboBox::from_id_salt("plc-cmd").width(110.).selected_text(cmd_name(self.plc_cmd)).show_ui(ui,|ui|{
                    for c in allowed{ui.selectable_value(&mut self.plc_cmd,*c,cmd_name(*c));}});
                ui.label("P1");ui.add(egui::DragValue::new(&mut self.plc_param).range(0..=100000));
            });
            let real=!s.simulation;
            if real && s.link==Link::On{ui.checkbox(&mut self.plc_confirm,RichText::new("Echte PLC: iemand staat bij de machine").color(DANGER));}
            if ui.add_enabled(s.link==Link::On && !s.busy && (!real || self.plc_confirm),egui::Button::new("Versturen")).clicked(){
                self.click(&format!("PLC proces {} opdracht {} P1 {}",self.plc_process,cmd_name(self.plc_cmd),self.plc_param));
                let r=self.dev.plc_command(self.plc_process,self.plc_cmd,vec![self.plc_param]);self.result(r);self.plc_confirm=false;
            }
            result_line(ui,if s.text=="Verbonden"{""}else{&s.text},s.ok);
            hint(ui,"Echte PLC: hier alleen Reset en Abort (zonder wachten, zoals Camelot). Lijn, transport, liften en vrije opdrachten: pagina Machine/lijn (met bevestiging).");
        });
    }
}
fn cmd_name(c:u32)->String{match c{1=>"Start (1)".into(),2=>"Reset (2)".into(),5=>"Abort (5)".into(),8=>"Stop (8)".into(),11=>"Omhoog (11)".into(),12=>"Omlaag (12)".into(),x=>x.to_string()}}
