//! Productie: pre-start check, Start / Pauze / Stop / Afbreken / Herstart, line view, messages, orders (new order,
//! priority, cancel, labels), the queue (order of pots, manual code) and the production history with filters.
use super::*;
use pump_studio::{code128,devices::DevId,production::{self,HistFilter,OrderRow,PotRow,ProdCmd,Slot}};

pub struct ProdUi{
    pub version:Option<i64>,pub name:String,pub batch:String,pub priority:i64,pub count:i64,pub confirm:bool,pub code:String,
    pub code_edit:Option<(i64,String)>,pub filter:HistFilter,pub days:i64,pub shown:Option<i64>,
    orders:Vec<OrderRow>,queue:Vec<PotRow>,hist:Vec<PotRow>,line:Option<production::LineState>,check:Vec<String>,
    key:Option<(u64,u64,i64)>,checked:Option<Instant>,
}
impl Default for ProdUi{fn default()->Self{Self{version:None,name:String::new(),batch:String::new(),priority:0,count:1,confirm:false,code:String::new(),code_edit:None,
    filter:HistFilter::default(),days:7,shown:None,orders:vec![],queue:vec![],hist:vec![],line:None,check:vec![],key:None,checked:None}}}

impl App{
    fn prod_refresh(&mut self){
        let pv=self.dev.prod.lock().unwrap().version;let sec=(production::now_ms()/1000) as i64;
        let key=(self.data_ver,pv,sec/2);
        if self.produi.key==Some(key){return;}
        self.produi.key=Some(key);
        self.produi.orders=self.db.orders(ProfileKind::Vsd).unwrap_or_default();
        self.produi.queue=self.db.queue(ProfileKind::Vsd).unwrap_or_default();
        self.produi.line=self.db.line_state().ok().flatten();
        let mut f=self.produi.filter.clone();f.from_ms=if self.produi.days>0{Some(production::now_ms()-self.produi.days*86_400_000)}else{None};
        self.produi.hist=self.db.history(ProfileKind::Vsd,&f,300).unwrap_or_default();
        if !self.dev.production_running() && self.produi.checked.is_none_or(|t|t.elapsed()>Duration::from_secs(2)){self.produi.check=self.dev.production_check(&self.db);self.produi.checked=Some(Instant::now());}
    }
    pub fn production_page(&mut self,ui:&mut egui::Ui){
        page_title(ui,"Productie","Orders → potten met eigen barcode → wachtrij. Start: de lijn draait zelf tot de wachtrij leeg is. LH1 en LH2 doseren tegelijk elk hun eigen pot; transport alleen met beide liften omlaag.");
        if self.profile()!=ProfileKind::Vsd{card(ui,"",|ui|empty(ui,"Productie draait op het VSD-profiel.",60.));return;}
        self.prod_refresh();
        self.prod_control(ui);ui.add_space(8.);
        self.line_view(ui);ui.add_space(8.);
        let wide=ui.available_width()>=1100.;
        if wide{ui.columns(2,|c|{self.orders_card(&mut c[0]);self.queue_card(&mut c[1]);});}else{self.orders_card(ui);ui.add_space(8.);self.queue_card(ui);}
        ui.add_space(8.);self.history_card(ui);
    }
    fn prod_control(&mut self,ui:&mut egui::Ui){
        let st=self.dev.prod.lock().unwrap().clone();let running=st.running;
        let hw=self.hardware_mode();
        card(ui,"Lijn",|ui|{
            ui.horizontal(|ui|{
                let (txt,col)=if running{(production::state_label(&st.state).to_string(),if st.hold.is_some(){DANGER}else{ACCENT})}
                    else{(self.produi.line.as_ref().map(|l|production::state_label(&l.state).to_string()).unwrap_or("klaar voor start".into()),INK)};
                ui.label(RichText::new(txt).size(18.).strong().color(col));
                if running && st.hold.is_none(){ui.spinner();}
                ui.label(RichText::new(if hw{"HARDWARE"}else{"simulatie"}).strong().color(if hw{DANGER}else{WARN}));
                if running{hint(ui,&format!("{} pot(ten) van de lijn",st.done));}
            });
            if let Some(h)=&st.hold{
                ui.label(RichText::new(h).strong().color(DANGER));
                ui.horizontal(|ui|{
                    if primary(ui,true,"Herstart"){self.click("Productie Herstart");let r=self.dev.production_cmd(ProdCmd::Restart);self.result(r);}
                    ui.label("Code pot op scanplek");ui.add(egui::TextEdit::singleline(&mut self.produi.code).desired_width(120.));
                    if ui.add_enabled(!self.produi.code.trim().is_empty(),egui::Button::new("Code doorgeven")).clicked(){
                        let c=self.produi.code.trim().to_string();self.click(&format!("Productie code {c}"));let r=self.dev.production_cmd(ProdCmd::Code(c));self.result(r);self.produi.code.clear();}
                });
            }
            ui.horizontal_wrapped(|ui|{
                if !running{
                    let restored=self.produi.line.as_ref().is_some_and(|l|l.slots.iter().any(|s|*s!=Slot::Empty));
                    if hw{ui.checkbox(&mut self.produi.confirm,RichText::new("Echte machine: lijn, transport, liften en pompen mogen zelf bewegen").color(DANGER));}
                    let ok=self.produi.check.is_empty() && (!hw || self.produi.confirm);
                    if primary(ui,ok,if restored{"Doorgaan met deze lijn (Start)"}else{"Start"}){
                        self.click("Productie Start");
                        match self.dev.start_production(&self.db,self.produi.confirm){Ok(())=>{self.produi.confirm=false;self.message="Productie gestart".into();}Err(e)=>self.message=e}
                        self.produi.key=None;
                    }
                    if restored && ui.button("Lijn leegmaken (potten handmatig weggehaald)").clicked(){
                        self.click("Lijn leegmaken");match self.db.clear_line(){Ok(n)=>{self.message=format!("Lijn leeggemaakt: {n} pot(ten) als onderbroken vastgelegd");self.data_ver+=1;}Err(e)=>self.message=e}
                        self.produi.key=None;
                    }
                }else{
                    if st.paused{if primary(ui,true,"Verder"){self.click("Productie Verder");let r=self.dev.production_cmd(ProdCmd::Resume);self.result(r);}}
                    else if ui.button("Pauze").on_hover_text("Lopende dosering maakt af; geen nieuwe start of transport").clicked(){self.click("Productie Pauze");let r=self.dev.production_cmd(ProdCmd::Pause);self.result(r);}
                    if ui.add_enabled(!st.stopping,egui::Button::new("Stop (leeglopen)")).on_hover_text("Geen nieuwe potten; de lijn loopt leeg en stopt").clicked(){self.click("Productie Stop");let r=self.dev.production_cmd(ProdCmd::Stop);self.result(r);}
                    if ui.add(egui::Button::new(RichText::new("Afbreken").strong().color(Color32::WHITE)).fill(DANGER)).on_hover_text("Pompen stoppen, liften omlaag, Abort naar lijn en units; potten blijven bekend als afgebroken").clicked(){
                        self.click("Productie Afbreken");let r=self.dev.production_cmd(ProdCmd::Abort);self.result(r);}
                }
            });
            if !running{
                if let Some(l)=&self.produi.line{if !l.message.is_empty() && l.state=="onderbroken"{ui.label(RichText::new(&l.message).color(WARN));}}
                if self.produi.check.is_empty(){ui.label(RichText::new("Controle vooraf: alles in orde").color(OK));}
                else{ui.label(RichText::new("Controle vooraf — eerst oplossen:").strong().color(DANGER));for w in self.produi.check.iter().take(12){ui.label(RichText::new(format!("• {w}")).color(DANGER));}}
            }
            let heads=self.dev.prod_heads.lock().unwrap().lines.clone();
            if !st.messages.is_empty() || !heads.is_empty(){
                ui.columns(2,|c|{
                    section(&mut c[0],"MELDINGEN");egui::ScrollArea::vertical().id_salt("prod-msg").max_height(150.).stick_to_bottom(true).show(&mut c[0],|ui|{for m in st.messages.iter().rev().take(120).rev(){ui.label(RichText::new(m).size(12.).monospace());}});
                    section(&mut c[1],"KOPPEN");egui::ScrollArea::vertical().id_salt("prod-heads").max_height(150.).stick_to_bottom(true).show(&mut c[1],|ui|{for m in heads.iter().rev().take(120).rev(){ui.label(RichText::new(m).size(12.).monospace());}});
                });
            }
        });
    }
    /// Spots of the line with the pot on each (live while running, else as stored in the database).
    pub fn line_view(&mut self,ui:&mut egui::Ui){
        let st=self.dev.prod.lock().unwrap().clone();
        let names=self.dev.settings.machine.line.names();
        let (slots,labels):(Vec<Slot>,Vec<String>)=if st.running && !st.slots.is_empty(){(st.slots.clone(),st.labels.clone())}else{
            let s=self.produi.line.as_ref().map(|l|l.slots.clone()).unwrap_or_default();
            let l=s.iter().map(|x|match x{Slot::Empty=>"—".into(),Slot::Unknown(c)=>format!("? {c}"),Slot::Pot(id)=>self.db.prod_pot(*id).map(|p|format!("{} · {} v{} · {}",p.barcode,p.recipe,p.version,p.status)).unwrap_or_default()}).collect();(s,l)};
        card(ui,"Lijnbeeld",|ui|{
            ui.horizontal_wrapped(|ui|{
                for (i,n) in names.iter().enumerate(){
                    let slot=slots.get(i).cloned().unwrap_or_default();let label=labels.get(i).cloned().unwrap_or("—".into());
                    let head=n=="LH1" || n=="LH2";
                    let fill=match slot{Slot::Empty=>BG,Slot::Unknown(_)=>Color32::from_rgb(253,236,206),Slot::Pot(_)=>if label.contains("doseren"){Color32::from_rgb(214,232,250)}else if label.contains("fout")||label.contains("afgebroken"){Color32::from_rgb(250,220,220)}else{Color32::from_rgb(225,242,230)}};
                    Frame::new().fill(fill).stroke(Stroke::new(if head{2.}else{W1},if head{ACCENT}else{BORDER})).corner_radius(CornerRadius::same(6)).inner_margin(Margin::same(8)).show(ui,|ui|{
                        ui.set_width(170.);ui.set_min_height(58.);
                        ui.label(RichText::new(n).strong().color(if head{ACCENT}else{MUTED}));
                        ui.label(RichText::new(label).size(12.));
                        if head{let h=if n=="LH1"{pump_studio::machine::Head::LH1}else{pump_studio::machine::Head::LH2};if self.dev.lift_up(h){ui.label(RichText::new("lift omhoog").size(11.).color(WARN));}}
                    });
                    if i+1<names.len(){ui.label(RichText::new("→").color(MUTED));}
                }
                ui.label(RichText::new("→ uit").color(MUTED));
            });
            hint(ui,"Aantal tussenplekken in Instellingen (VSD). Lijnbeeld en potstatus staan in de database: na een herstart van de app blijft bekend welke pot waar staat.");
        });
    }
    fn orders_card(&mut self,ui:&mut egui::Ui){
        let recipes:Vec<pump_studio::recipe::RecipeRow>=self.db.recipes(ProfileKind::Vsd).unwrap_or_default().into_iter().filter(|r|!r.archived).collect();
        card(ui,"Orders",|ui|{
            section(ui,"NIEUWE ORDER");
            egui::Grid::new("po-new").num_columns(2).spacing([10.,5.]).show(ui,|ui|{
                ui.label("Recept");let sel=recipes.iter().find(|r|Some(r.version_id)==self.produi.version);
                egui::ComboBox::from_id_salt("po-rec").width(220.).selected_text(sel.map(|r|format!("{} v{}",r.name,r.version)).unwrap_or("Kies".into())).show_ui(ui,|ui|{
                    for r in &recipes{if ui.selectable_label(Some(r.version_id)==self.produi.version,format!("{} v{}",r.name,r.version)).clicked(){self.produi.version=Some(r.version_id);}}});ui.end_row();
                ui.label("Aantal potten");ui.add(egui::DragValue::new(&mut self.produi.count).range(1..=500));ui.end_row();
                ui.label("Ordernaam");ui.add(egui::TextEdit::singleline(&mut self.produi.name).desired_width(180.));ui.end_row();
                ui.label("Batch");ui.add(egui::TextEdit::singleline(&mut self.produi.batch).desired_width(120.));ui.end_row();
                ui.label("Prioriteit");ui.add(egui::DragValue::new(&mut self.produi.priority).range(-9..=9)).on_hover_text("Hoger = eerder");ui.end_row();
            });
            if primary(ui,self.produi.version.is_some(),"Order aanmaken"){
                let v=self.produi.version.unwrap();self.click("Order aanmaken");
                match self.db.create_order(ProfileKind::Vsd,v,&self.produi.name,&self.produi.batch,self.produi.priority,self.produi.count){
                    Ok((id,p))=>{self.message=format!("Order {id}: {} pot(ten) in de wachtrij",p.len());self.produi.name.clear();self.data_ver+=1;self.produi.key=None;}Err(e)=>self.message=e}
            }
            ui.separator();
            let orders=self.produi.orders.clone();
            if orders.is_empty(){empty(ui,"Nog geen orders.",40.);return;}
            egui::ScrollArea::vertical().id_salt("po-list").max_height(260.).show(ui,|ui|egui::Grid::new("po-g").striped(true).spacing([8.,3.]).show(ui,|ui|{
                for t in ["Order","Recept","Potten","Prio",""]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                for o in &orders{
                    ui.label(format!("{} {}{}",o.id,o.name,if o.batch.is_empty(){String::new()}else{format!(" ({})",o.batch)}));ui.label(format!("{} v{}",o.recipe,o.version));
                    ui.label(format!("{} wacht · {} klaar · {} anders",o.waiting,o.done,o.other));
                    ui.horizontal(|ui|{if ui.small_button("−").clicked(){let r=self.db.set_order_priority(o.id,o.priority-1);self.result(r);self.produi.key=None;}ui.label(o.priority.to_string());
                        if ui.small_button("+").clicked(){let r=self.db.set_order_priority(o.id,o.priority+1);self.result(r);self.produi.key=None;}});
                    ui.horizontal(|ui|{
                        if ui.small_button("Labels").on_hover_text("Labelvel (HTML, Code 128) openen in de browser").clicked(){self.labels(o.id,false);}
                        if !self.dev.settings.machine.printer_host.trim().is_empty() && ui.small_button("ZPL").on_hover_text("Labels naar de labelprinter (ZPL, poort 9100)").clicked(){self.labels(o.id,true);}
                        if o.waiting>0 && ui.small_button("Annuleren").on_hover_text("Wachtende potten van deze order annuleren").clicked(){self.click(&format!("Order {} annuleren",o.id));match self.db.cancel_order(o.id){Ok(n)=>self.message=format!("{n} pot(ten) geannuleerd"),Err(e)=>self.message=e}self.produi.key=None;}
                    });
                    ui.end_row();
                }
            }));
        });
    }
    /// Labels of the order's pots: HTML sheet opened in the browser, or ZPL to the network printer.
    fn labels(&mut self,order:i64,zpl:bool){
        self.click(&format!("Labels order {order}{}",if zpl{" (ZPL)"}else{""}));
        let r=(||{
            let o=self.db.orders(ProfileKind::Vsd)?.into_iter().find(|x|x.id==order).ok_or("Order niet gevonden")?;
            let mut pots:Vec<PotRow>=self.db.queue(ProfileKind::Vsd)?.into_iter().filter(|p|p.order_id==Some(order)).collect();
            if pots.is_empty(){pots=self.db.history(ProfileKind::Vsd,&HistFilter{order:Some(order),..Default::default()},1000)?;}
            pots.sort_by_key(|p|p.nr);
            let labels:Vec<code128::Label>=pots.iter().map(|p|code128::Label{code:p.barcode.clone(),lines:vec![format!("{} {}",if o.name.is_empty(){format!("Order {}",o.id)}else{o.name.clone()},o.batch),format!("{} v{} · pot {}/{}",p.recipe,p.version,p.nr,p.count)]}).collect();
            if labels.is_empty(){return Err("Geen potten".to_string());}
            if zpl{let m=&self.dev.settings.machine;code128::send_zpl(&m.printer_host,m.printer_port,&code128::zpl(&labels))?;return Ok(format!("{} label(s) naar {}:{}",labels.len(),m.printer_host.trim(),m.printer_port));}
            let html=code128::label_sheet(&format!("Labels order {} {} — {} v{}",o.id,o.name,o.recipe,o.version),&labels)?;
            let p=code128::save_sheet(&self.dev.settings.output.join("labels"),&format!("labels_order_{}",o.id),&html)?;
            #[cfg(windows)] {let _=std::process::Command::new("cmd").args(["/c","start",""]).arg(&p).spawn();}
            Ok(format!("Labelvel geopend: {}",p.display()))
        })();
        match r{Ok(m)=>{applog::info("PRODUCTIE",&m);self.message=m;}Err(e)=>self.message=format!("Labels: {e}")}
    }
    fn queue_card(&mut self,ui:&mut egui::Ui){
        let q=self.produi.queue.clone();
        card(ui,&format!("Wachtrij ({})",q.len()),|ui|{
            if q.is_empty(){empty(ui,"Geen wachtende potten.",40.);return;}
            let mut act:Option<(i64,i64)>=None;let mut save:Option<(i64,String)>=None;
            egui::ScrollArea::vertical().id_salt("pq").max_height(320.).show(ui,|ui|egui::Grid::new("pq-g").striped(true).spacing([8.,3.]).show(ui,|ui|{
                for t in ["#","Code","Order","Recept","",""]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                for (k,p) in q.iter().enumerate(){
                    ui.label((k+1).to_string());
                    match &mut self.produi.code_edit{
                        Some((id,t)) if *id==p.id=>{ui.add(egui::TextEdit::singleline(t).desired_width(110.));}
                        _=>{ui.label(RichText::new(&p.barcode).monospace());}
                    }
                    ui.label(format!("{} ({}/{})",p.order,p.nr,p.count));ui.label(format!("{} v{}",p.recipe,p.version));
                    ui.horizontal(|ui|{if ui.small_button("↑").clicked(){act=Some((p.id,-1));}if ui.small_button("↓").clicked(){act=Some((p.id,1));}});
                    ui.horizontal(|ui|{
                        match &self.produi.code_edit{
                            Some((id,t)) if *id==p.id=>{if ui.small_button("Opslaan").clicked(){save=Some((p.id,t.clone()));}if ui.small_button("Annuleren").clicked(){save=Some((-1,String::new()));}}
                            _=>{if ui.small_button("Code…").on_hover_text("Eigen (voorgedrukte) code koppelen").clicked(){self.produi.code_edit=Some((p.id,p.barcode.clone()));}}
                        }
                    });
                    ui.end_row();
                }
            }));
            if let Some((id,d))=act{let r=self.db.move_pot(ProfileKind::Vsd,id,d);self.result(r);self.produi.key=None;}
            if let Some((id,c))=save{if id<0{self.produi.code_edit=None;}else{self.click(&format!("Code pot {id}: {c}"));match self.db.set_pot_barcode(id,&c){Ok(())=>{self.produi.code_edit=None;self.produi.key=None;}Err(e)=>self.message=e}}}
            hint(ui,"Volgorde: hoogste prioriteit eerst, dan de volgorde hier. Gescande code moet een wachtende pot zijn; een gebruikte of afgeronde code wordt nooit opnieuw gedoseerd.");
        });
    }
    fn history_card(&mut self,ui:&mut egui::Ui){
        let hist=self.produi.hist.clone();
        card(ui,"Productiehistorie",|ui|{
            ui.horizontal_wrapped(|ui|{
                ui.label("Zoek");if ui.add(egui::TextEdit::singleline(&mut self.produi.filter.text).desired_width(140.).hint_text("code, order, recept")).changed(){self.produi.key=None;}
                ui.label("Periode");egui::ComboBox::from_id_salt("ph-days").width(110.).selected_text(match self.produi.days{1=>"vandaag",7=>"7 dagen",30=>"30 dagen",_=>"alles"}).show_ui(ui,|ui|{
                    for (d,t) in [(1,"vandaag"),(7,"7 dagen"),(30,"30 dagen"),(0,"alles")]{if ui.selectable_value(&mut self.produi.days,d,t).clicked(){self.produi.key=None;}}});
                ui.label("Status");let cur=self.produi.filter.status.clone().unwrap_or("alle".into());
                egui::ComboBox::from_id_salt("ph-st").width(130.).selected_text(cur).show_ui(ui,|ui|{
                    if ui.selectable_label(self.produi.filter.status.is_none(),"alle").clicked(){self.produi.filter.status=None;self.produi.key=None;}
                    for s in [production::DONE,production::FAILED,production::NOT_DOSED,production::ABORTED,production::INTERRUPTED,production::ON_LINE,production::DOSING,production::CANCELLED]{
                        if ui.selectable_label(self.produi.filter.status.as_deref()==Some(s),s).clicked(){self.produi.filter.status=Some(s.into());self.produi.key=None;}}});
                ui.label("Order");let ord=self.produi.filter.order;
                egui::ComboBox::from_id_salt("ph-ord").width(150.).selected_text(ord.map(|o|o.to_string()).unwrap_or("alle".into())).show_ui(ui,|ui|{
                    if ui.selectable_label(ord.is_none(),"alle").clicked(){self.produi.filter.order=None;self.produi.key=None;}
                    for o in self.produi.orders.clone(){if ui.selectable_label(ord==Some(o.id),format!("{} {}",o.id,o.name)).clicked(){self.produi.filter.order=Some(o.id);self.produi.key=None;}}});
            });
            if hist.is_empty(){empty(ui,"Geen potten in deze selectie.",40.);return;}
            egui::ScrollArea::vertical().id_salt("ph").max_height(240.).show(ui,|ui|egui::Grid::new("ph-g").striped(true).spacing([10.,3.]).show(ui,|ui|{
                for t in ["Code","Datum","Order","Recept","Soort","Status","Koppen",""]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                for p in &hist{
                    ui.label(RichText::new(&p.barcode).monospace().color(if Some(p.id)==self.produi.shown{ACCENT}else{INK}));ui.label(applog::local_time(p.started_ms));ui.label(&p.order);
                    ui.label(format!("{} v{}",p.recipe,p.version));ui.label(RichText::new(if p.simulation{"simulatie"}else{"hardware"}).color(if p.simulation{WARN}else{INK}));
                    ui.label(RichText::new(&p.status).color(if p.status==production::DONE{OK}else{DANGER})).on_hover_text(&p.note);
                    ui.label(p.heads.iter().map(|(k,v)|format!("{k}: {v}")).collect::<Vec<_>>().join(", "));
                    if ui.small_button("Toon").clicked(){self.produi.shown=Some(p.id);}ui.end_row();
                }
            }));
            if let Some(id)=self.produi.shown{
                let items=self.db.pot_items(id).unwrap_or_default();let ev=self.db.events(Some(id),50).unwrap_or_default();
                section(ui,&format!("POT {}",production::barcode_of(id)));
                egui::Grid::new("ph-items").striped(true).spacing([10.,2.]).show(ui,|ui|{
                    for t in ["Stap","Ingrediënt","Kop","Gevraagd g","Gedoseerd g","Afw. %","Oordeel","Toegerekend"]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                    for i in &items{ui.label((i.step_idx+1).to_string());ui.label(&i.ingredient);ui.label(&i.head);ui.label(pump_studio::pumptest::nl(i.requested_g,3));
                        ui.label(i.dosed_g.map(|g|pump_studio::pumptest::nl(g,4)).unwrap_or("—".into()));ui.label(i.deviation_pct.map(|d|pump_studio::pumptest::nls(d,2)).unwrap_or("—".into()));
                        ui.label(pump_studio::recipe::status_label(&i.status));ui.label(if i.attributed{"ja"}else{""});ui.end_row();}
                });
                for (ms,_,k,t) in ev.iter().rev(){hint(ui,&format!("{} {k}: {t}",applog::local_time(*ms)));}
            }
        });
    }
    pub fn scanner_card(&mut self,ui:&mut egui::Ui){
        let s=self.dev.status(DevId::Scanner);
        card(ui,"Barcodescanner",|ui|{
            ui.horizontal(|ui|{dot(ui,link_color(s.link,s.simulation));ui.label(format!("{} · {}",link_text(s.link,s.simulation),if s.endpoint.is_empty(){"—"}else{&s.endpoint}));if s.busy{ui.spinner();}});
            if ui.add_enabled(s.link==Link::On && !s.busy,egui::Button::new("Scanner testen (trigger)")).clicked(){self.click("Scanner testen");let r=self.dev.scan_test();self.result(r);}
            if !s.scan.is_empty(){ui.label(RichText::new(&s.scan).monospace().color(if s.ok{INK}else{DANGER}));}
            hint(ui,"Generiek protocol (zoals Camelot): trigger aan (BA), antwoord <STX>code<ETX><CR><LF> of <STX><NAK> = geen code, trigger uit (B). Instellingen → Scanner.");
        });
    }
}
