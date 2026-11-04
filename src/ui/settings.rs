//! Instellingen: profile, connection manager and the connection settings per device (stored in the
//! database with Toepassen, validated with clear messages).
use super::*;
use pump_studio::{machine::MachineConfig,model::{Config,CONTROLLER_LIMIT_MAX,MAX_POSITION},workflow::Action};

/// VSD connection settings as typed in the form; `Err` holds the first problem in Dutch.
pub fn validate_vsd(m:&MachineConfig,python:&str)->Result<(),String>{
    if m.bitrate==0 || m.can_channel>15{return Err("CAN: kanaal 0–15 en bitrate invullen".into());}
    m.plc.validate().map_err(|e|format!("PLC: {e}"))?;
    if m.plc.target_node==0 || m.plc.host_node==0{return Err("PLC: FINS-nodes 1–254".into());}
    for (k,s) in m.scales.iter().enumerate(){
        let h=s.host.trim();
        if h.is_empty() || h.contains(char::is_whitespace) || s.port==0{return Err(format!("Weegschaal LH{}: IP-adres en poort invullen",k+1));}
    }
    if m.scales[0].host.trim().eq_ignore_ascii_case(m.scales[1].host.trim()) && m.scales[0].port==m.scales[1].port{return Err("LH1 en LH2 hebben hetzelfde weegschaaladres; één weegschaal kan maar één kop bedienen".into());}
    let mut probe=m.clone();probe.pumps.clear();probe.validate()?;
    m.line.validate()?;
    if !m.scanner.host.trim().is_empty() || m.scanner.kind==pump_studio::scanner::ScanKind::Com{m.scanner.validate()?;}
    if python.trim().is_empty(){return Err("Pad naar Python invullen (nodig voor hardware)".into());}
    Ok(())
}
pub fn validate_bench(c:&Config,python:&str,repo:&str)->Result<(),String>{
    c.validate()?;
    if c.channel>15{return Err("CAN-kanaal 0–15".into());}
    if python.trim().is_empty() || repo.trim().is_empty(){return Err("Pad naar Python en CAN-map invullen (nodig voor hardware)".into());}
    Ok(())
}

impl App{
    pub fn settings_page(&mut self,ui:&mut egui::Ui){
        page_title(ui,"Instellingen",&format!("Profiel, verbindingen per apparaat en verbindingsinstellingen. Toepassen slaat direct op in de database ({}).",self.db.path().display()));
        card(ui,"Profiel",|ui|{
            ui.horizontal(|ui|{
                for p in ProfileKind::ALL{if ui.radio(self.profile()==p,match p{ProfileKind::Vsd=>"VSD-machine (LH1/LH2, 192 pompen, PLC)",ProfileKind::Keramisch=>"Keramische pompbank"}).clicked(){self.switch_profile(p);}}
            });
            hint(ui,"Wisselen zonder herstart. Apparaten van het huidige profiel worden eerst gestopt en verbroken.");
        });
        ui.add_space(8.);self.connections_card(ui);ui.add_space(8.);
        card(ui,"Logboek",|ui|{
            ui.horizontal(|ui|{
                ui.label("Logregels in de database bewaren");
                let r=ui.add(egui::DragValue::new(&mut self.log_days).range(0..=3650).suffix(" dagen"));
                if r.changed(){self.log_days=self.log_days.clamp(0,3650);}
                if ui.button("Opslaan").clicked(){
                    self.click(&format!("Logretentie {} dagen",self.log_days));
                    match self.db.set_meta("log_retention_days",&self.log_days.to_string()){Ok(())=>self.message=format!("Logretentie: {} dagen (geldt bij de volgende start)",self.log_days),Err(e)=>self.message=e}
                }
            });
            hint(ui,"Standaard 90 dagen; 0 = alles bewaren. Opruimen gebeurt bij het starten, op de achtergrond. De dagbestanden in de map logs blijven staan.");
        });
        ui.add_space(8.);
        if self.profile()==ProfileKind::Vsd{self.vsd_settings(ui)}else{self.bench_settings(ui)}
    }
    fn vsd_settings(&mut self,ui:&mut egui::Ui){
        let mut stored=self.dev.settings.machine.clone();stored.pumps.clear();
        let dirty=stored!=self.f_machine || self.f_python!=self.dev.settings.python.display().to_string();
        let check=validate_vsd(&self.f_machine,&self.f_python);
        ui.columns(2,|c|{
            card(&mut c[0],"CAN (pompborden)",|ui|{egui::Grid::new("s-can").num_columns(2).spacing([14.,6.]).show(ui,|ui|{
                ui.label("IXXAT-kanaal");ui.add(egui::DragValue::new(&mut self.f_machine.can_channel).range(0..=15));ui.end_row();
                ui.label("Bitrate");ui.add(egui::DragValue::new(&mut self.f_machine.bitrate).range(10000..=1000000).suffix(" bit/s"));ui.end_row();
                ui.label("Python");ui.add(egui::TextEdit::singleline(&mut self.f_python).desired_width(260.));ui.end_row();
            });hint(ui,"Hardware via bridge\\vsd_bridge.py (python-can + IXXAT), zelfde frames als Camelot.");});
            c[0].add_space(8.);
            card(&mut c[0],"Weegschalen (MT-SICS via TCP)",|ui|{egui::Grid::new("s-scale").num_columns(2).spacing([14.,6.]).show(ui,|ui|{
                for k in 0..2{ui.label(format!("LH{} IP-adres / poort",k+1));ui.horizontal(|ui|{
                    ui.add(egui::TextEdit::singleline(&mut self.f_machine.scales[k].host).desired_width(130.));
                    ui.add(egui::DragValue::new(&mut self.f_machine.scales[k].port).range(1..=65535));});ui.end_row();}
            });
                if ui.small_button("Camelot-adressen (LH1 .56, LH2 .55, poort 23)").clicked(){
                    self.f_machine.scales[0].host="192.168.0.56".into();self.f_machine.scales[0].port=23;self.f_machine.scales[1].host="192.168.0.55".into();self.f_machine.scales[1].port=23;}
                hint(ui,"Pc moet in 192.168.0.x zitten. Een weegschaal bedient één client tegelijk.");
            });
            card(&mut c[1],"PLC (Omron NJ, FINS/UDP)",|ui|{egui::Grid::new("s-plc").num_columns(2).spacing([14.,6.]).show(ui,|ui|{
                let p=&mut self.f_machine.plc;
                ui.label("IP-adres");ui.add(egui::TextEdit::singleline(&mut p.ip).desired_width(130.));ui.end_row();
                ui.label("Poort PLC / pc");ui.horizontal(|ui|{ui.add(egui::DragValue::new(&mut p.port).range(1..=65535));ui.add(egui::DragValue::new(&mut p.host_port).range(0..=65535));});ui.end_row();
                ui.label("FINS-node PLC / pc");ui.horizontal(|ui|{ui.add(egui::DragValue::new(&mut p.target_node).range(1..=254));ui.add(egui::DragValue::new(&mut p.host_node).range(1..=254));});ui.end_row();
                ui.label("Opdrachtkanaal");ui.add(egui::DragValue::new(&mut p.channel).range(1..=100));ui.end_row();
                ui.label("Time-out");ui.add(egui::DragValue::new(&mut p.timeout_s).range(1. ..=600.).suffix(" s"));ui.end_row();
            });
            section(ui,"LIFTHOOGTE PER KOP");
            egui::Grid::new("s-lift").num_columns(2).spacing([14.,6.]).show(ui,|ui|{
                for k in 0..2{let h=&mut self.f_machine.lift_height_cm[k];let mut on=h.is_some();
                    ui.label(format!("LH{}",k+1));ui.horizontal(|ui|{
                        if ui.checkbox(&mut on,"ingesteld").changed(){*h=if on{Some(10.)}else{None};}
                        if let Some(v)=h.as_mut(){ui.add(egui::DragValue::new(v).range(0.1..=100.).speed(0.1).fixed_decimals(1).suffix(" cm"));ui.label(format!("P1 = {}",pump_studio::machine::height_p1(*v)));}
                    });ui.end_row();}
            });
            hint(ui,"Lift omhoog krijgt P1 = hoogte × 10 (zoals Camelot). Een recept kan een eigen pothoogte hebben. Op hardware start een pot niet zonder hoogte.");
            section(ui,"LIJN (PLEKKEN)");
            egui::Grid::new("s-line").num_columns(2).spacing([14.,6.]).show(ui,|ui|{
                let l=&mut self.f_machine.line;
                ui.label("Scan → LH1");ui.add(egui::DragValue::new(&mut l.to_lh1).range(0..=5).suffix(" tussenplek(ken)"));ui.end_row();
                ui.label("LH1 → LH2");ui.add(egui::DragValue::new(&mut l.between).range(0..=5).suffix(" tussenplek(ken)"));ui.end_row();
                ui.label("Na LH2");ui.add(egui::DragValue::new(&mut l.after).range(0..=5).suffix(" plek(ken) vóór uit"));ui.end_row();
                ui.label("No-read / onbekende code");ui.horizontal(|ui|{ui.selectable_value(&mut l.ask_operator,false,"pot gaat ongedoseerd mee");ui.selectable_value(&mut l.ask_operator,true,"vragen aan operator (hold)");});ui.end_row();
            });
            hint(ui,&format!("Lijn: {} → uit (Camelot: scan → wacht → LH1 → LH2 → uit).",self.f_machine.line.names().join(" → ")));hint(ui,"Standaard PLC1 VSD uit Camelot: 10.6.1.15:9600, node 15, pc-node 50, kanaal 1. Camelot en Pump Studio nooit tegelijk met de PLC verbinden.");});
        });
        ui.add_space(8.);
        ui.columns(2,|c|{
            card(&mut c[0],"Barcodescanner (generiek)",|ui|{egui::Grid::new("s-scan").num_columns(2).spacing([14.,6.]).show(ui,|ui|{
                let sc=&mut self.f_machine.scanner;
                ui.label("Verbinding");ui.horizontal(|ui|{ui.selectable_value(&mut sc.kind,pump_studio::scanner::ScanKind::Tcp,"Netwerk (TCP)");ui.selectable_value(&mut sc.kind,pump_studio::scanner::ScanKind::Com,"COM-poort");});ui.end_row();
                if sc.kind==pump_studio::scanner::ScanKind::Tcp{ui.label("IP-adres / poort");ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut sc.host).desired_width(130.));ui.add(egui::DragValue::new(&mut sc.port).range(1..=65535));});ui.end_row();}
                else{ui.label("COM-poort / baud");ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut sc.com).desired_width(80.));ui.add(egui::DragValue::new(&mut sc.baud).range(1200..=115200));});ui.end_row();}
                ui.label("Trigger aan / uit");ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut sc.trigger_on).desired_width(60.));ui.add(egui::TextEdit::singleline(&mut sc.trigger_off).desired_width(60.));});ui.end_row();
                ui.label("Time-out / pogingen");ui.horizontal(|ui|{ui.add(egui::DragValue::new(&mut sc.timeout_s).range(0.5..=60.).speed(0.1).suffix(" s"));ui.add(egui::DragValue::new(&mut sc.retries).range(1..=10));});ui.end_row();
            });hint(ui,"Zoals Camelot GenericBarcodeReader: trigger BA / B, antwoord <STX>code<ETX><CR><LF>, <STX><NAK> = geen code; 10 s, 2 pogingen. \\r en \\xNN mogen in de trigger. COM loopt via bridge\\scanner_bridge.py.");});
            card(&mut c[1],"Labelprinter (optioneel, ZPL)",|ui|{egui::Grid::new("s-prn").num_columns(2).spacing([14.,6.]).show(ui,|ui|{
                ui.label("IP-adres / poort");ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut self.f_machine.printer_host).desired_width(130.).hint_text("leeg = geen"));ui.add(egui::DragValue::new(&mut self.f_machine.printer_port).range(1..=65535));});ui.end_row();
            });hint(ui,"Leeg: labels als labelvel (HTML) in de browser afdrukken. Ingevuld: knop ZPL bij de order stuurt de labels rechtstreeks (poort 9100).");});
        });
        ui.add_space(8.);
        self.apply_bar(ui,dirty,&check,|a|{
            let mut m=a.f_machine.clone();m.pumps.clear();
            a.db.set_setting(ProfileKind::Vsd,"machine",&m)?;a.db.set_setting(ProfileKind::Vsd,"python",&a.f_python.trim().to_string())?;
            m.pumps=std::mem::take(&mut a.dev.settings.machine.pumps);a.dev.settings.machine=m;a.dev.settings.python=a.f_python.trim().into();
            Ok(())
        },|a|{let mut m=a.dev.settings.machine.clone();m.pumps.clear();a.f_machine=m;a.f_python=a.dev.settings.python.display().to_string();});
    }
    fn bench_settings(&mut self,ui:&mut egui::Ui){
        let dirty=self.dev.settings.bench!=self.f_bench || self.f_python!=self.dev.settings.python.display().to_string() || self.f_repo!=self.dev.settings.repo.display().to_string();
        let check=validate_bench(&self.f_bench,&self.f_python,&self.f_repo);
        let c=&mut self.f_bench;let inv36=&mut self.inv36_steps;
        ui.columns(2,|col|{
            card(&mut col[0],"Pomp en motor",|ui|{egui::Grid::new("b-pump").num_columns(2).spacing([14.,6.]).show(ui,|ui|{
                ui.label("Pomptype");let old=c.pump_type.clone();
                egui::ComboBox::from_id_salt("b-type").selected_text(&c.pump_type).show_ui(ui,|ui|{for t in ["INV-13","INV-36"]{ui.selectable_value(&mut c.pump_type,t.into(),t);}});
                if old!=c.pump_type{if old=="INV-36"{*inv36=c.steps_rev;}c.steps_rev=if c.pump_type=="INV-13"{25600}else{*inv36};}ui.end_row();
                ui.label("Stappen / omwenteling");ui.add(egui::DragValue::new(&mut c.steps_rev).range(1..=MAX_POSITION));ui.end_row();
                ui.label("CAN-node / motor");ui.horizontal(|ui|{ui.add(egui::DragValue::new(&mut c.node).range(1..=2047));ui.add(egui::DragValue::new(&mut c.motor).range(1..=8));});ui.end_row();
                ui.label("Snelheid stappen/s");ui.add(egui::DragValue::new(&mut c.speed).range(1..=MAX_POSITION));ui.end_row();
                ui.label("Acceleratie / deceleratie");ui.horizontal(|ui|{ui.add(egui::DragValue::new(&mut c.accel).range(1..=MAX_POSITION));ui.add(egui::DragValue::new(&mut c.decel).range(1..=MAX_POSITION));});ui.end_row();
                ui.label("Slaginstelling");ui.text_edit_singleline(&mut c.stroke_setting);ui.end_row();
                ui.label("Vloeistof");ui.text_edit_singleline(&mut c.fluid);ui.end_row();
                ui.label("Controllergrens");ui.add(egui::DragValue::new(&mut c.controller_limit).speed(10000).range(100_000..=CONTROLLER_LIMIT_MAX)).on_hover_text("Boven ~10,7 miljoen stappen loopt het controllerdoel over. Bij deze grens zet de app de teller met bevestigde ZERO terug.");ui.end_row();
                ui.label("MOVE_TO_DONE verplicht");ui.checkbox(&mut c.require_feedback,"");ui.end_row();
            });hint(ui,"INV-13: bevestigd 25.600 stappen/omw. INV-36: eigen waarde controleren.");});
            col[0].add_space(8.);
            card(&mut col[0],"Wegen",|ui|{egui::Grid::new("b-weigh").num_columns(2).spacing([14.,6.]).show(ui,|ui|{
                ui.label("Stabiliteitsvenster");ui.add(egui::DragValue::new(&mut c.window_s).speed(0.1).range(0.1..=60.).suffix(" s"));ui.end_row();
                ui.label("Max. variatie");ui.add(egui::DragValue::new(&mut c.range_g).speed(0.001).max_decimals(4).range(0.0001..=1.).suffix(" g"));ui.end_row();
                ui.label("Weegtime-out");ui.add(egui::DragValue::new(&mut c.timeout_s).range(1. ..=300.).suffix(" s"));ui.end_row();
                ui.label("Uitlooptijd");ui.add(egui::DragValue::new(&mut c.settle_s).speed(0.1).range(0. ..=300.).suffix(" s"));ui.end_row();
            });});
            card(&mut col[1],"CAN en weegschaal",|ui|{egui::Grid::new("b-conn").num_columns(2).spacing([14.,6.]).show(ui,|ui|{
                ui.label("IXXAT-kanaal");ui.add(egui::DragValue::new(&mut c.channel).range(0..=15));ui.end_row();
                ui.label("Bitrate");ui.add(egui::DragValue::new(&mut c.bitrate).range(10000..=1000000).suffix(" bit/s"));ui.end_row();
                ui.label("Weegschaal");ui.horizontal(|ui|{ui.selectable_value(&mut c.scale_tcp,false,"COM-poort");ui.selectable_value(&mut c.scale_tcp,true,"Netwerk (TCP)");});ui.end_row();
                if c.scale_tcp{
                    ui.label("Koppen");ui.checkbox(&mut c.scale_dual,"LH1 en LH2 tegelijk");ui.end_row();
                    if !c.scale_dual{c.measure_head=0;}
                    ui.label(if c.scale_dual{"LH1 IP / poort"}else{"IP / poort"});ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut c.scale_host).desired_width(120.));ui.add(egui::DragValue::new(&mut c.scale_tcp_port).range(1..=65535));});ui.end_row();
                    if c.scale_dual{
                        ui.label("LH2 IP / poort");ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut c.scale_host2).desired_width(120.));ui.add(egui::DragValue::new(&mut c.scale_tcp_port2).range(1..=65535));});ui.end_row();
                        ui.label("Meten op");ui.horizontal(|ui|{ui.selectable_value(&mut c.measure_head,0,"LH1");ui.selectable_value(&mut c.measure_head,1,"LH2");});ui.end_row();
                    }
                }else{
                    ui.label("COM-poort");ui.add(egui::TextEdit::singleline(&mut c.port).desired_width(90.));ui.end_row();
                    ui.label("Baudrate");ui.add(egui::DragValue::new(&mut c.baud).range(1200..=115200));ui.end_row();
                }
            });});
            col[1].add_space(8.);
            card(&mut col[1],"Hardwarekoppeling",|ui|{egui::Grid::new("b-paths").num_columns(2).spacing([14.,6.]).show(ui,|ui|{
                ui.label("CAN-map");ui.add(egui::TextEdit::singleline(&mut self.f_repo).desired_width(250.));ui.end_row();
                ui.label("Python");ui.add(egui::TextEdit::singleline(&mut self.f_python).desired_width(250.));ui.end_row();
            });hint(ui,"Alleen nodig voor hardware (bridge\\hardware_bridge.py).");});
        });
        ui.add_space(8.);
        self.apply_bar(ui,dirty,&check,|a|{
            let cfg=a.f_bench.clone();
            a.db.set_bench_config(&cfg)?;a.db.set_setting(ProfileKind::Keramisch,"python",&a.f_python.trim().to_string())?;a.db.set_setting(ProfileKind::Keramisch,"repo",&a.f_repo.trim().to_string())?;
            a.dev.settings.bench=cfg.clone();a.dev.settings.python=a.f_python.trim().into();a.dev.settings.repo=a.f_repo.trim().into();
            a.load_profile()?;
            // Connected bench: pump and motion settings apply live (existing Configure); the scale keeps its connection.
            if a.dev.bench.connected{
                let b=a.dev.bench.config.clone();let mut live=cfg;
                live.use_scale=b.use_scale;live.port=b.port;live.baud=b.baud;live.scale_tcp=b.scale_tcp;live.scale_host=b.scale_host;live.scale_tcp_port=b.scale_tcp_port;
                live.scale_dual=b.scale_dual;live.scale_host2=b.scale_host2;live.scale_tcp_port2=b.scale_tcp_port2;live.measure_head=b.measure_head;
                a.dev.bench_action(Action::Configure{cfg:live})?;
            }
            Ok(())
        },|a|{a.f_bench=a.dev.settings.bench.clone();a.f_python=a.dev.settings.python.display().to_string();a.f_repo=a.dev.settings.repo.display().to_string();});
    }
    fn apply_bar(&mut self,ui:&mut egui::Ui,dirty:bool,check:&Result<(),String>,apply:impl FnOnce(&mut Self)->Result<(),String>,cancel:impl FnOnce(&mut Self)){
        card(ui,"",|ui|{
            ui.horizontal(|ui|{
                if primary(ui,dirty && check.is_ok(),"Toepassen"){
                    self.click(&format!("Instellingen {} toepassen",self.profile().name()));
                    match apply(self){
                        Ok(())=>{
                            let open:Vec<&str>=pump_studio::devices::DevId::of(self.profile()).iter().filter(|d|self.dev.is_on(**d)).map(|d|d.short()).collect();
                            applog::info("INSTELLINGEN",&format!("Instellingen {} opgeslagen",self.profile().name()));
                            self.message=if open.is_empty() || self.profile()==ProfileKind::Keramisch{"Instellingen opgeslagen".into()}
                                else{format!("Instellingen opgeslagen; geldt voor {} bij opnieuw verbinden",open.join(", "))};
                        }
                        Err(e)=>self.message=format!("Niet opgeslagen: {e}"),
                    }
                }
                if ui.add_enabled(dirty,egui::Button::new("Annuleren")).clicked(){cancel(self);}
                match check{
                    Err(e)=>{ui.label(RichText::new(e).color(DANGER));}
                    Ok(())=>{if dirty{ui.label(RichText::new("Niet opgeslagen wijzigingen").color(WARN));}else{hint(ui,"Alles opgeslagen");}}
                }
            });
        });
    }
}

#[cfg(test)] mod tests{
    use super::*;
    #[test]fn vsd_validation_messages(){
        let mut m=MachineConfig::default();m.pumps.clear();assert!(validate_vsd(&m,"python.exe").is_ok());
        let mut b=m.clone();b.plc.ip="10.6.1".into();assert!(validate_vsd(&b,"p").unwrap_err().contains("PLC"));
        let mut b=m.clone();b.scales[1]=b.scales[0].clone();assert!(validate_vsd(&b,"p").unwrap_err().contains("hetzelfde"));
        let mut b=m.clone();b.scales[0].host=" ".into();assert!(validate_vsd(&b,"p").unwrap_err().contains("LH1"));
        let mut b=m.clone();b.plc.timeout_s=0.;assert!(validate_vsd(&b,"p").is_err());
        assert!(validate_vsd(&m,"").unwrap_err().contains("Python"));
    }
    #[test]fn bench_validation_messages(){
        let c=Config::default();assert!(validate_bench(&c,"p","r").is_ok());
        let mut b=c.clone();b.motor=9;assert!(validate_bench(&b,"p","r").is_err());
        let mut b=c.clone();b.channel=16;assert!(validate_bench(&b,"p","r").unwrap_err().contains("kanaal"));
        assert!(validate_bench(&c,"p","").is_err());
    }
}
