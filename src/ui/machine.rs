//! Machine/lijn: hand operation of the VSD line (line 1310, transport 1220, lifts, units, barcode 1190), free PLC
//! commands, scanner test and "PLC initialiseren (zoals Camelot)". Every action only after an explicit click; on a
//! real PLC with a confirmation. Transport is refused while a lift is up.
use super::*;
use pump_studio::{devices::DevId,machine::{height_p1,Head},plc::{CMD_RESET,CMD_START,CMD_STOP}};

#[derive(Default)]
pub struct MachUi{pub confirm:bool,pub init_confirm:bool,pub process:u32,pub cmd:u32,pub p1:u32}

impl App{
    pub fn machine_page(&mut self,ui:&mut egui::Ui){
        page_title(ui,"Machine/lijn","Handbediening van de VSD-lijn. Alleen na een klik; op een echte PLC eerst bevestigen dat iemand bij de machine staat. Niet tijdens productie of een pot.");
        if self.profile()!=ProfileKind::Vsd{card(ui,"",|ui|empty(ui,"Machine/lijn hoort bij het VSD-profiel.",60.));return;}
        self.line_view(ui);ui.add_space(8.);
        let plc=self.dev.status(DevId::Plc);let real=plc.link==Link::On && !plc.simulation;
        let m=self.dev.settings.machine.clone();
        card(ui,"Handbediening",|ui|{
            ui.horizontal(|ui|{dot(ui,link_color(plc.link,plc.simulation));ui.label(format!("PLC: {}",link_text(plc.link,plc.simulation)));if plc.busy{ui.spinner();}});
            if real{ui.checkbox(&mut self.mach.confirm,RichText::new("Echte PLC: iemand staat bij de machine, de lijn mag bewegen").color(DANGER));}
            let en=plc.link==Link::On && !plc.busy && (!real || self.mach.confirm) && !self.dev.production_running();
            let mut act:Option<(String,Vec<(u32,u32,Vec<u32>)>)>=None;
            ui.add_enabled_ui(en,|ui|{
                egui::Grid::new("m-hand").num_columns(2).spacing([12.,6.]).show(ui,|ui|{
                    ui.label(format!("Lijn {}",m.line_process));ui.horizontal(|ui|{
                        if ui.button("Start").clicked(){act=Some(("Lijn start".into(),vec![(m.line_process,CMD_START,vec![])]));}
                        if ui.button("Stop").clicked(){act=Some(("Lijn stop".into(),vec![(m.line_process,CMD_STOP,vec![])]));}});ui.end_row();
                    ui.label(format!("Transport {}",m.transport_process));ui.horizontal(|ui|{
                        if ui.button("Eén plek verder").on_hover_text("Start (P1..P6 = 0, P9 = −1, P10 = 1234567890) en Stop, zoals Camelot; alleen met beide liften omlaag").clicked(){
                            act=Some(("Transport één plek".into(),vec![(m.transport_process,CMD_START,vec![0,0,0,0,0,0,0,0,u32::MAX,1234567890]),(m.transport_process,CMD_STOP,vec![])]));}});ui.end_row();
                    for h in Head::ALL{
                        let cm=m.lift_height_cm[h.idx()].unwrap_or(pump_studio::devices::SIM_LIFT_CM);let p1=height_p1(cm);
                        ui.label(format!("Lift {} ({})",h.name(),m.lift_process[h.idx()]));ui.horizontal(|ui|{
                            if ui.button(format!("Omhoog ({} cm, P1 {p1})",pump_studio::pumptest::nl(cm,1))).clicked(){
                                if real && m.lift_height_cm[h.idx()].is_none(){self.message=format!("Geen lifthoogte voor {} ingesteld (Instellingen)",h.name());}
                                else{act=Some((format!("Lift {} omhoog",h.name()),vec![(m.lift_process[h.idx()],CMD_START,vec![p1])]));}}
                            if ui.button("Omlaag").clicked(){act=Some((format!("Lift {} omlaag",h.name()),vec![(m.lift_process[h.idx()],CMD_STOP,vec![])]));}
                            if self.dev.lift_up(h){ui.label(RichText::new("staat omhoog").color(WARN));}
                        });ui.end_row();
                        ui.label(format!("Unit {} ({})",h.name(),m.unit_process[h.idx()]));ui.horizontal(|ui|{
                            if ui.button("Start").clicked(){act=Some((format!("Unit {} start",h.name()),vec![(m.unit_process[h.idx()],CMD_START,vec![])]));}
                            if ui.button("Stop").clicked(){act=Some((format!("Unit {} stop",h.name()),vec![(m.unit_process[h.idx()],CMD_STOP,vec![])]));}});ui.end_row();
                    }
                    ui.label(format!("Barcode {}",m.barcode_process));ui.horizontal(|ui|{
                        if ui.button("Start (P1 = 1)").clicked(){act=Some(("Barcode start".into(),vec![(m.barcode_process,CMD_START,vec![1])]));}
                        if ui.button("Stop").clicked(){act=Some(("Barcode stop".into(),vec![(m.barcode_process,CMD_STOP,vec![])]));}
                        if ui.button("Reset").clicked(){act=Some(("Barcode reset".into(),vec![(m.barcode_process,CMD_RESET,vec![])]));}});ui.end_row();
                });
                ui.separator();section(ui,"VRIJE OPDRACHT");
                ui.horizontal_wrapped(|ui|{
                    if self.mach.process==0{self.mach.process=m.line_process;self.mach.cmd=CMD_RESET;}
                    ui.label("Proces");ui.add(egui::DragValue::new(&mut self.mach.process).range(1..=9999));
                    ui.label("Opdracht");ui.add(egui::DragValue::new(&mut self.mach.cmd).range(1..=255));
                    ui.label("P1");ui.add(egui::DragValue::new(&mut self.mach.p1).range(0..=100000));
                    if ui.button("Versturen").clicked(){let (p,c,v)=(self.mach.process,self.mach.cmd,self.mach.p1);act=Some((format!("Proces {p} opdracht {c} P1 {v}"),vec![(p,c,vec![v])]));}
                });
                hint(ui,"Opdrachten: 1 Start, 2 Reset, 5 Abort, 6 Hold, 7 Restart, 8 Stop, 11 Omhoog, 12 Omlaag. Reset/Abort/Hold worden zonder wachten verstuurd (zoals Camelot).");
            });
            if let Some((label,steps))=act{self.click(&format!("Machine/lijn: {label}"));let r=self.dev.line_action(&label,steps,self.mach.confirm);self.result(r);}
            result_line(ui,if plc.text=="Verbonden"{""}else{&plc.text},plc.ok);
            if self.dev.production_running(){hint(ui,"Productie loopt: handbediening uit.");}
        });
        ui.add_space(8.);
        ui.columns(2,|c|{
            self.scanner_card(&mut c[0]);
            card(&mut c[1],"PLC initialiseren (zoals Camelot)",|ui|{
                ui.label(RichText::new("Stuurt per module Abort (5), PowerUp (98) en Reset (2): lijn, transport, barcode, units en liften. Camelot mag NIET draaien.").color(WARN));
                ui.checkbox(&mut self.mach.init_confirm,"Camelot is gesloten en de machine mag geïnitialiseerd worden");
                if ui.add_enabled(plc.link==Link::On && !plc.busy && self.mach.init_confirm && !self.dev.production_running(),egui::Button::new("PLC initialiseren")).clicked(){
                    self.click("PLC initialiseren");let r=self.dev.plc_init(true);self.result(r);self.mach.init_confirm=false;}
                hint(ui,"Nooit automatisch; alleen met deze knop.");
            });
        });
        ui.add_space(8.);
        card(ui,"PLC-berichten",|ui|{
            if plc.messages.is_empty(){empty(ui,"Nog geen berichten.",40.);}
            else{egui::ScrollArea::vertical().id_salt("m-plc").max_height(200.).show(ui,|ui|{for m in plc.messages.iter().rev().take(120){ui.label(RichText::new(m).monospace().size(12.));}});}
        });
    }
}
