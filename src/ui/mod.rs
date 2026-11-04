//! Page-based UI: sidebar with all pages, top bar with profile, device status and STOP, status bar.
//! Pages read the device manager's status every frame and send commands that return at once;
//! hardware I/O only happens on the device worker threads.
mod shell;mod pumps;mod monitors;mod settings;mod log;mod calibration;mod testpage;mod ingredients;mod recipes;mod production;mod machine;mod analysis;
use eframe::egui::{self,Color32,CornerRadius,Frame,Margin,RichText,Stroke};
use pump_studio::{applog,db::{Db,ProfileKind,PumpRow},devices::{DevId,Devices,Link,Settings},machine::MachineConfig,model::Config};
use std::{collections::{HashMap,HashSet},path::PathBuf,time::{Duration,Instant}};

// Palette: light industrial theme; status colours carry meaning only (ok / simulatie-waarschuwing / fout).
pub const BG:Color32=Color32::from_rgb(242,244,248);
pub const CARD:Color32=Color32::WHITE;
pub const BORDER:Color32=Color32::from_rgb(222,227,234);
pub const INK:Color32=Color32::from_rgb(28,36,48);
pub const MUTED:Color32=Color32::from_rgb(108,118,132);
pub const ACCENT:Color32=Color32::from_rgb(20,90,170);
pub const OK:Color32=Color32::from_rgb(28,135,80);
pub const WARN:Color32=Color32::from_rgb(196,128,18);
pub const DANGER:Color32=Color32::from_rgb(185,40,40);
pub const HEADER:Color32=Color32::from_rgb(21,33,51);
pub const W1:f32=1.0;

#[derive(Clone,Copy,PartialEq,Eq,Debug)]
pub enum Page{Pompen,Kalibratie,Pomptest,Recepten,Ingredienten,Productie,Machine,Monitors,Analyse,Logboek,Instellingen}
impl Page{
    pub const ALL:[Page;11]=[Page::Pompen,Page::Kalibratie,Page::Pomptest,Page::Recepten,Page::Ingredienten,Page::Productie,Page::Machine,Page::Monitors,Page::Analyse,Page::Logboek,Page::Instellingen];
    pub fn label(self)->&'static str{match self{Page::Pompen=>"Pompen",Page::Kalibratie=>"Kalibratie",Page::Pomptest=>"Pomptest",Page::Recepten=>"Recepten",Page::Ingredienten=>"Ingrediënten",
        Page::Productie=>"Productie",Page::Machine=>"Machine/lijn",Page::Monitors=>"Monitors",Page::Analyse=>"Analyse",Page::Logboek=>"Logboek",Page::Instellingen=>"Instellingen"}}
}

pub struct App{
    pub db:Db,pub dev:Devices,pub page:Page,
    /// Pumps of the active profile (DB rows), ingredient names, date of the active calibration per pump (DB).
    pub pumps:Vec<PumpRow>,pub ingredients:Vec<String>,pub cal_dates:HashMap<String,(i64,bool)>,
    /// Kalibratie page.
    pub calui:calibration::CalUi,cal_seen:(u64,u64),
    /// Pomptest page.
    pub ptui:testpage::TestUi,
    /// Ingrediënten and Recepten pages; `data_ver` changes whenever pumps/ingredients/recipes change.
    pub ingui:ingredients::IngUi,pub recui:recipes::RecUi,pub data_ver:u64,
    /// Productie and Machine/lijn pages.
    pub produi:production::ProdUi,pub mach:machine::MachUi,
    /// Analyse page; log retention in days (Instellingen).
    pub anaui:analysis::AnaUi,pub log_days:i64,
    /// Settings forms (applied with Toepassen).
    pub f_machine:MachineConfig,pub f_bench:Config,pub f_python:String,pub f_repo:String,pub inv36_steps:i64,
    /// Simulatie (true) or Hardware per device in the connection manager.
    pub mode:HashMap<DevId,bool>,
    // Pompen page
    pub filter:String,pub fhead:usize,pub factive:usize,pub sel:HashSet<i64>,pub ing_buf:HashMap<i64,String>,
    /// VSD manual move: unit 0 stappen / 1 omwentelingen / 2 graden; typed text per unit (own parsers, no
    /// drag input); confirmation of a move back, valid only for the target/unit/pumps it was given for.
    pub step_unit:usize,pub step_text:String,pub rev_text:String,pub deg_text:String,pub back_ok:bool,pub back_key:(usize,String,Vec<i64>),
    /// Handbediening VSD: speed, acceleration, deceleration as typed (steps/s, steps/s²).
    pub motion_text:[String;3],
    pub jog_mode:usize,pub jog_unit:usize,pub jog_amount:f64,pub jog_target:i64,pub jog_phase:f64,pub jog_weigh:bool,
    // Monitors page
    pub scan_s:f64,pub plc_process:u32,pub plc_cmd:u32,pub plc_param:u32,pub plc_confirm:bool,
    // Logboek page
    pub log_seq:u64,pub log_lines:Vec<applog::Line>,pub log_filter:String,pub log_level:usize,pub log_area:String,
    /// Last message for the operator (status bar); each new one is logged once.
    pub message:String,last_msg:String,
    can_version:u64,bench_pos:Option<i64>,closing:Option<Instant>,
    /// Whether Esc means STOP, decided at the end of the previous frame (egui drops focus on Esc
    /// before the next frame is drawn, so the current frame can no longer tell).
    esc_ok:bool,
}

impl App{
    pub fn new(cc:&eframe::CreationContext<'_>)->Self{
        style(&cc.egui_ctx);
        let base=std::env::current_exe().ok().and_then(|p|p.parent().map(PathBuf::from)).unwrap_or_default();
        let root=if base.join("bridge/hardware_bridge.py").exists(){base}else{PathBuf::from(env!("CARGO_MANIFEST_DIR"))};
        applog::init(root.join("logs"));
        applog::info("APP",&format!("Pump Studio gestart vanuit {}",root.display()));
        match Self::open(root.clone()){Ok(a)=>a,Err(e)=>{
            // Without a database nothing can be stored: say so and stop instead of running half.
            applog::error("DATABASE",&e);
            #[cfg(windows)] {let _=std::process::Command::new("msg").args(["*",&format!("Pump Studio: {e}")]).spawn();}
            std::process::exit(2);
        }}
    }
    /// Database, log index, device manager and forms for `root` (also used by the smoke test).
    pub fn open(root:PathBuf)->Result<Self,String>{
        let path=root.join("data").join("pumpstudio.db");
        let mut db=Db::open(&path)?;
        if db.created{applog::info("DATABASE",&format!("Nieuwe database {} met profielen VSD (192 pompen) en Keramisch (1 pomp)",path.display()));}
        match db.import_legacy(&root){Ok(Some(n))=>applog::info("DATABASE",&n),Ok(None)=>{},Err(e)=>applog::warn("DATABASE",&format!("Oude machine.json niet overgenomen: {e}"))}
        match db.import_calibrations(&root){Ok(Some(n))=>applog::info("DATABASE",&n),Ok(None)=>{},Err(e)=>applog::warn("DATABASE",&format!("Oude kalibraties niet overgenomen: {e}"))}
        match db.import_tests(&root){Ok(Some(n))=>applog::info("DATABASE",&n),Ok(None)=>{},Err(e)=>applog::warn("DATABASE",&format!("Oude doseertesten niet overgenomen: {e}"))}
        match db.close_stale_runs(){Ok(n) if n>0=>applog::warn("POMPTEST",&format!("{n} test(en) waren nog bezig bij afsluiten: status onderbroken")),Ok(_)=>{},Err(e)=>applog::warn("DATABASE",&e)}
        match db.close_stale_pots(){Ok(n) if n>0=>applog::warn("RECEPT",&format!("{n} pot(ten) waren nog bezig bij afsluiten: status onderbroken")),Ok(_)=>{},Err(e)=>applog::warn("DATABASE",&e)}
        let restored=match db.restore_after_restart(){Ok(m)=>m,Err(e)=>{applog::warn("DATABASE",&e);None}};
        if let Some(m)=&restored{applog::warn("PRODUCTIE",m);}
        // Log retention (Instellingen, default 90 days): old index rows removed off the UI thread.
        let log_days=db.meta("log_retention_days")?.and_then(|v|v.parse::<i64>().ok()).unwrap_or(90);
        {let p=path.clone();let _=std::thread::Builder::new().name("log-retentie".into()).spawn(move||{
            match Db::open(&p).and_then(|d|d.set_busy_timeout(Duration::from_secs(5)).map(|_|d)).and_then(|d|d.prune_log(log_days,pump_studio::analysis::now_ms())){
                Ok(n) if n>0=>applog::info("LOGBOEK",&format!("{n} logregel(s) ouder dan {log_days} dagen uit de database verwijderd")),Ok(_)=>{},Err(e)=>applog::warn("LOGBOEK",&format!("Opruimen logindex: {e}"))}});}
        let tx=pump_studio::db::spawn_log_writer(path.clone())?;
        applog::set_sink(Some(Box::new(move|l|{let _=tx.send(l.clone());})));
        let profile=db.active_profile()?;
        let machine=db.machine_config()?;let bench=db.bench_config()?;
        let mut s=Settings::new(machine.clone(),bench.clone(),root.join("results"));
        s.bridge_dir=root.join("bridge");
        s.python=db.setting::<String>(profile,"python")?.unwrap_or_else(pump_studio::db::default_python).into();
        s.repo=db.setting::<String>(ProfileKind::Keramisch,"repo")?.unwrap_or_else(pump_studio::db::default_repo).into();
        s.mix_allowed=db.meta("mix_allowed")?.as_deref()==Some("1");
        s.db_path=Some(path.clone());
        let dev=Devices::new(profile,s);
        let mut f_machine=machine;f_machine.pumps.clear();
        let mut app=Self{dev,page:Page::Pompen,pumps:vec![],ingredients:vec![],cal_dates:HashMap::new(),calui:calibration::CalUi::default(),cal_seen:(0,0),ptui:testpage::TestUi::default(),ingui:Default::default(),recui:Default::default(),data_ver:0,produi:Default::default(),mach:Default::default(),anaui:Default::default(),log_days:90,
            f_machine,f_bench:bench,f_python:String::new(),f_repo:String::new(),inv36_steps:51200,mode:HashMap::new(),
            filter:String::new(),fhead:0,factive:0,sel:HashSet::new(),ing_buf:HashMap::new(),
            step_unit:0,step_text:"25 600".into(),rev_text:"0,5".into(),deg_text:"180".into(),back_ok:false,back_key:(0,String::new(),vec![]),motion_text:Default::default(),
            jog_mode:0,jog_unit:1,jog_amount:1.,jog_target:0,jog_phase:0.,jog_weigh:false,
            scan_s:12.,plc_process:1300,plc_cmd:pump_studio::plc::CMD_RESET,plc_param:0,plc_confirm:false,
            log_seq:0,log_lines:vec![],log_filter:String::new(),log_level:0,log_area:String::new(),
            message:String::new(),last_msg:String::new(),can_version:0,bench_pos:None,closing:None,esc_ok:true,db};
        app.log_days=log_days;
        app.load_profile()?;
        // Reopen on the page used last.
        if let Some(p)=app.db.meta("page")?.and_then(|l|Page::ALL.into_iter().find(|p|p.label()==l)){app.page=p;}
        // Production was running when the app closed: show the restored line, the operator decides.
        if let Some(m)=restored{app.message=m;app.page=Page::Productie;}
        Ok(app)
    }
    pub fn profile(&self)->ProfileKind{self.dev.profile}
    /// (Re)load pumps, ingredients and forms of the active profile from the database.
    pub fn load_profile(&mut self)->Result<(),String>{
        let p=self.db.profile(self.profile())?;
        self.pumps=self.db.pumps(p.id)?;self.ingredients=self.db.ingredients(p.id)?;
        self.cal_dates=self.db.active_dates(self.profile())?;
        self.sel.retain(|id|self.pumps.iter().any(|r|r.id==*id));self.ing_buf.clear();self.data_ver+=1;
        self.f_python=self.db.setting::<String>(self.profile(),"python")?.unwrap_or_else(pump_studio::db::default_python);
        self.f_repo=self.db.setting::<String>(ProfileKind::Keramisch,"repo")?.unwrap_or_else(pump_studio::db::default_repo);
        self.dev.settings.python=self.f_python.clone().into();
        if self.profile()==ProfileKind::Vsd{let pumps:Vec<_>=self.pumps.iter().map(PumpRow::to_vsd).collect();self.dev.set_pumps(pumps);}
        Ok(())
    }
    /// Profile switch without restart: devices of the old profile are disconnected first.
    pub fn switch_profile(&mut self,p:ProfileKind){
        if p==self.profile(){return;}
        self.click(&format!("Profiel wisselen naar {}",p.name()));
        // Returns at once (K8); `tick` finishes the switch when the old devices are stopped and closed.
        match self.dev.switch_profile(p){Err(e)=>self.message=e,Ok(())=>{if self.dev.switching.is_some(){self.message=format!("Profiel wisselen naar {}…",p.name());}}}
        self.finish_switch();
    }
    fn finish_switch(&mut self){
        match self.dev.take_switch_result(){
            None=>{}
            Some(Err(e))=>self.message=e,
            Some(Ok(p))=>{if let Err(e)=self.db.set_active_profile(p).and_then(|_|self.load_profile()){self.message=format!("Profiel laden mislukt: {e}");}else{self.message=format!("Profiel {} actief",p.name());}}
        }
    }
    /// Per frame, before drawing: worker updates, profile switch, positions and calibration dates.
    pub fn tick(&mut self){
        self.dev.poll();self.finish_switch();self.persist_positions();
        let seen=(self.dev.status(DevId::Can).cal_version,self.dev.bench_cal_version());
        if seen!=self.cal_seen{self.cal_seen=seen;if let Ok(d)=self.db.active_dates(self.profile()){self.cal_dates=d;}}
    }
    pub fn click(&self,what:&str){applog::info("KLIK",what);}
    /// Result of a manager call: refusals become the operator message (and are logged once).
    pub fn result(&mut self,r:Result<(),String>){if let Err(e)=r{self.message=e;}}
    pub fn sim_mode(&self,id:DevId)->bool{*self.mode.get(&id).unwrap_or(&true)}

    /// Store edited pump fields at once; VSD pump list goes to the CAN group and the simulation.
    pub fn save_pump(&mut self,row:PumpRow){
        match self.db.update_pump(&row){
            Ok(())=>{
                applog::info("POMPEN",&format!("{}: kop {}, ingrediënt '{}', {}",row.name,row.head.map(|h|h.name()).unwrap_or("—"),row.ingredient,if row.enabled{"actief"}else{"uit"}));
                if let Some(r)=self.pumps.iter_mut().find(|r|r.id==row.id){*r=row.clone();}self.data_ver+=1;
                if !row.ingredient.trim().is_empty() && !self.ingredients.contains(&row.ingredient.trim().to_string()){self.ingredients.push(row.ingredient.trim().into());self.ingredients.sort();}
                if self.profile()==ProfileKind::Vsd{let pumps:Vec<_>=self.pumps.iter().map(PumpRow::to_vsd).collect();self.dev.set_pumps(pumps);}
            }
            Err(e)=>self.message=format!("Opslaan mislukt: {e}"),
        }
    }
    /// Positions reported by the devices are kept in the database (last known position).
    pub fn persist_positions(&mut self){
        let st=self.dev.status(DevId::Can);
        if st.version!=self.can_version{
            self.can_version=st.version;
            if self.profile()==ProfileKind::Vsd{
                let mut changed=vec![];
                for r in self.pumps.iter_mut(){if let Some(p)=st.positions.get(&r.name).map(|p|*p as f64){if r.position!=Some(p){r.position=Some(p);changed.push((r.id,Some(p)));}}}
                self.store_positions(&changed);
            }
        }
        if self.profile()==ProfileKind::Keramisch && self.dev.bench.connected && self.bench_pos!=Some(self.dev.bench.position){
            self.bench_pos=Some(self.dev.bench.position);
            let p=self.dev.bench.position as f64;
            let changed:Vec<(i64,Option<f64>)>=self.pumps.first_mut().filter(|r|r.position!=Some(p)).map(|r|{r.position=Some(p);(r.id,Some(p))}).into_iter().collect();
            self.store_positions(&changed);
        }
    }
    /// P6: one transaction per command; a failure (e.g. database locked) is logged and shown, never dropped.
    fn store_positions(&mut self,rows:&[(i64,Option<f64>)]){
        if let Err(e)=self.db.set_pump_positions(rows){
            applog::error("DATABASE",&format!("Posities van {} pomp(en) niet opgeslagen: {e}",rows.len()));
            self.message=format!("Posities niet opgeslagen: {e}");
        }
    }
    pub fn stop(&mut self){self.click("STOP");self.dev.stop_all();self.message="STOP: alle lopende opdrachten onderbroken".into();}
}

pub fn style(ctx:&egui::Context){
    let mut v=egui::Visuals::light();
    v.panel_fill=BG;v.window_fill=CARD;v.extreme_bg_color=Color32::from_rgb(249,250,252);v.faint_bg_color=Color32::from_rgb(246,248,251);
    v.selection.bg_fill=Color32::from_rgb(210,226,246);v.selection.stroke=Stroke::new(W1,ACCENT);
    for w in [&mut v.widgets.inactive,&mut v.widgets.hovered,&mut v.widgets.active,&mut v.widgets.open,&mut v.widgets.noninteractive]{w.corner_radius=CornerRadius::same(4);}
    v.widgets.inactive.bg_stroke=Stroke::new(W1,BORDER);v.widgets.noninteractive.bg_stroke=Stroke::new(W1,BORDER);
    ctx.set_visuals(v);
    ctx.style_mut(|s|{
        s.spacing.item_spacing=egui::vec2(8.,6.);s.spacing.button_padding=egui::vec2(10.,4.);s.spacing.interact_size.y=24.;
        for (st,size) in [(egui::TextStyle::Heading,20.),(egui::TextStyle::Body,14.),(egui::TextStyle::Button,14.),(egui::TextStyle::Small,12.),(egui::TextStyle::Monospace,13.)]{
            let mono=st==egui::TextStyle::Monospace;s.text_styles.insert(st,if mono{egui::FontId::monospace(size)}else{egui::FontId::proportional(size)});}
    });
}

// ---------- shared widgets ----------
pub fn card<R>(ui:&mut egui::Ui,title:&str,add:impl FnOnce(&mut egui::Ui)->R)->R{
    Frame::new().fill(CARD).stroke(Stroke::new(W1,BORDER)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(12)).show(ui,|ui|{
        ui.set_width(ui.available_width());
        if !title.is_empty(){ui.label(RichText::new(title).strong().size(15.).color(INK));ui.add_space(2.);}
        add(ui)}).inner
}
pub fn page_title(ui:&mut egui::Ui,title:&str,sub:&str){ui.label(RichText::new(title).size(20.).strong().color(INK));if !sub.is_empty(){ui.label(RichText::new(sub).color(MUTED));}ui.add_space(8.);}
pub fn hint(ui:&mut egui::Ui,text:&str){ui.label(RichText::new(text).size(12.).color(MUTED));}
pub fn section(ui:&mut egui::Ui,text:&str){ui.add_space(4.);ui.label(RichText::new(text).size(11.).strong().color(MUTED));}
pub fn dot(ui:&mut egui::Ui,color:Color32){let (rect,_)=ui.allocate_exact_size(egui::vec2(10.,10.),egui::Sense::hover());ui.painter().circle_filled(rect.center(),4.5,color);}
pub fn empty(ui:&mut egui::Ui,text:&str,height:f32){
    let (rect,_)=ui.allocate_exact_size(egui::vec2(ui.available_width(),height),egui::Sense::hover());
    ui.painter().rect(rect.shrink(2.),CornerRadius::same(6),BG,Stroke::new(W1,BORDER),egui::StrokeKind::Inside);
    ui.painter().text(rect.center(),egui::Align2::CENTER_CENTER,text,egui::FontId::proportional(13.),MUTED);
}
pub fn primary(ui:&mut egui::Ui,enabled:bool,text:&str)->bool{
    ui.add_enabled(enabled,egui::Button::new(RichText::new(text).strong().color(Color32::WHITE)).fill(ACCENT)).clicked()
}
pub fn result_line(ui:&mut egui::Ui,text:&str,ok:bool){if !text.is_empty(){ui.label(RichText::new(text).color(if ok{INK}else{DANGER}));}}
/// Colour of a device state: hardware on = green, simulation on = orange, failed = red, off = grey.
pub fn link_color(link:Link,sim:bool)->Color32{match link{Link::On=>if sim{WARN}else{OK},Link::Connecting=>ACCENT,Link::Failed=>DANGER,Link::Off=>MUTED}}
pub fn link_text(link:Link,sim:bool)->&'static str{match link{Link::On=>if sim{"simulatie"}else{"hardware"},Link::Connecting=>"verbinden…",Link::Failed=>"fout",Link::Off=>"uit"}}
/// Esc means STOP unless a text field has keyboard focus or a popup/list is open. Call at the end of a frame.
pub fn esc_is_stop(ctx:&egui::Context)->bool{
    let editing=ctx.memory(|m|m.focused()).is_some_and(|id|egui::text_edit::TextEditState::load(ctx,id).is_some());
    !editing && !ctx.memory(|m|m.any_popup_open())
}
/// Inline field: save on leaving the field (Enter/click elsewhere), never on Esc (that cancels the edit).
pub fn field_commit(lost_focus:bool,esc:bool,buf:&str,current:&str)->Option<String>{
    if lost_focus && !esc && buf.trim()!=current{Some(buf.trim().to_string())}else{None}
}
pub fn grams(v:Option<f64>)->String{v.map(|g|format!("{g:.4} g")).unwrap_or("—".into())}

impl eframe::App for App{
    fn update(&mut self,ctx:&egui::Context,_frame:&mut eframe::Frame){
        let frame_start=Instant::now();
        self.tick();
        if self.message!=self.last_msg{if !self.message.is_empty(){applog::warn("MELDING",&self.message);}self.last_msg=self.message.clone();}
        if let Some((seq,lines))=applog::snapshot(self.log_seq){self.log_seq=seq;self.log_lines=lines;}
        // R6: Esc = STOP, unless a text field is being edited or a list is open (then Esc only cancels that).
        if ctx.input(|i|i.key_pressed(egui::Key::Escape)) && self.esc_ok{self.stop();}
        // Closing: stop and disconnect every group first so ports and bridge processes are released.
        if ctx.input(|i|i.viewport().close_requested()){
            match self.closing{
                None=>{ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);applog::info("APP","Afsluiten: stoppen en verbindingen sluiten");self.dev.shutdown();self.closing=Some(Instant::now());}
                Some(_)=>{}
            }
        }
        if let Some(t)=self.closing{
            self.dev.poll();
            let open=DevId::VSD.iter().chain(DevId::KERAMISCH.iter()).any(|d|self.dev.status(*d).link==Link::On);
            if !open || t.elapsed()>Duration::from_secs(4){applog::info("APP","Afgesloten");ctx.send_viewport_cmd(egui::ViewportCommand::Close);}
        }
        self.frame(ctx);
        self.esc_ok=esc_is_stop(ctx);
        // A frame of more than 100 ms would make the UI feel stuck: log it (K8).
        let ft=frame_start.elapsed();if ft>Duration::from_millis(100){applog::warn("UI",&format!("Trage frame: {} ms op pagina {}",ft.as_millis(),self.page.label()));}
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}
impl App{
    /// All panels of one frame (also driven headless by the tests and the smoke test).
    pub fn frame(&mut self,ctx:&egui::Context){
        egui::TopBottomPanel::top("top").frame(Frame::new().fill(HEADER).inner_margin(Margin::symmetric(14,8))).show(ctx,|ui|self.top_bar(ui));
        egui::TopBottomPanel::bottom("status").frame(Frame::new().fill(CARD).stroke(Stroke::new(W1,BORDER)).inner_margin(Margin::symmetric(12,5))).show(ctx,|ui|self.status_bar(ui));
        egui::SidePanel::left("nav").resizable(false).exact_width(196.).frame(Frame::new().fill(CARD).stroke(Stroke::new(W1,BORDER)).inner_margin(Margin::same(10)))
            .show(ctx,|ui|{egui::ScrollArea::vertical().show(ui,|ui|self.sidebar(ui));});
        egui::CentralPanel::default().frame(Frame::new().fill(BG).inner_margin(Margin::same(14))).show(ctx,|ui|{
            egui::ScrollArea::vertical().auto_shrink(false).show(ui,|ui|match self.page{
                Page::Pompen=>self.pumps_page(ui),Page::Kalibratie=>self.calibration_page(ui),Page::Pomptest=>self.pumptest_page(ui),Page::Monitors=>self.monitors_page(ui),Page::Logboek=>self.log_page(ui),Page::Instellingen=>self.settings_page(ui),
                Page::Ingredienten=>self.ingredients_page(ui),Page::Recepten=>self.recipes_page(ui),
                Page::Productie=>self.production_page(ui),Page::Machine=>self.machine_page(ui),Page::Analyse=>self.analysis_page(ui),
            });
        });
    }
}

/// Render every page in both profiles at 1366×768 without any device connected (no panic, nothing blank).
pub fn render_all_pages(app:&mut App)->Result<usize,String>{
    let ctx=egui::Context::default();style(&ctx);let mut frames=0;
    let input=||egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(1366.,768.))),..Default::default()};
    for p in [ProfileKind::Vsd,ProfileKind::Keramisch,ProfileKind::Vsd]{
        app.switch_profile(p);
        let t=Instant::now();while app.profile()!=p || app.dev.switching.is_some(){app.tick();if t.elapsed()>Duration::from_secs(5){return Err(format!("Profiel {} niet actief: {}",p.name(),app.message));}std::thread::sleep(Duration::from_millis(5));}
        for page in Page::ALL{
            app.page=page;
            for _ in 0..2{let out=ctx.run(input(),|ctx|app.frame(ctx));frames+=1;
                if out.shapes.len()<10{return Err(format!("Pagina {} ({}) lijkt leeg: {} vormen",page.label(),p.name(),out.shapes.len()));}}
        }
    }
    Ok(frames)
}

/// K8: frames while the profile switches (VSD devices connected, a dose running) stay short.
pub fn switch_frames(app:&mut App,ctx:&egui::Context)->Result<(Duration,Duration,usize),String>{
    let input=||egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(1366.,768.))),..Default::default()};
    let t=Instant::now();app.switch_profile(ProfileKind::Keramisch);
    let mut max=Duration::ZERO;let mut n=0;
    while app.profile()!=ProfileKind::Keramisch || app.dev.switching.is_some(){
        let f=Instant::now();let _=ctx.run(input(),|ctx|{app.tick();app.frame(ctx);});max=max.max(f.elapsed());n+=1;
        if t.elapsed()>Duration::from_secs(5){return Err(format!("wissel niet klaar: {}",app.message));}
    }
    Ok((max,t.elapsed(),n))
}

#[cfg(test)] mod tests{
    use super::*;
    fn esc()->egui::Event{egui::Event::Key{key:egui::Key::Escape,physical_key:None,pressed:true,repeat:false,modifiers:Default::default()}}
    #[test]fn esc_stops_unless_a_text_field_is_edited(){
        let ctx=egui::Context::default();let mut text=String::from("Water");let mut seen=vec![];let mut esc_ok=true;
        // Frame 0: field gets focus. Frame 1: Esc while editing -> no STOP, field loses focus, edit not saved.
        // Frame 2: Esc with nothing focused -> STOP. Frame 3: a focused button does not swallow Esc.
        for k in 0..4{
            let mut input=egui::RawInput::default();if k>0{input.events.push(esc());}
            let mut commit=None;
            let _=ctx.run(input,|ctx|{
                if k>0{seen.push(esc_ok);}
                egui::CentralPanel::default().show(ctx,|ui|{
                    let r=ui.add(egui::TextEdit::singleline(&mut text).id(egui::Id::new("ing")));
                    if k==0{r.request_focus();}
                    if k==1{text.push_str("XYZ");}
                    commit=field_commit(r.lost_focus(),ui.input(|i|i.key_pressed(egui::Key::Escape)),&text,"Water");
                    let b=ui.button("knop");if k==2{b.request_focus();}
                });
                esc_ok=esc_is_stop(ctx);
            });
            if k==1{assert_eq!(commit,None,"Esc slaat niet op");}
        }
        assert_eq!(seen,vec![false,true,true]);
        // An open list (combo box popup, e.g. "Kies" ingredient) also keeps Esc for itself.
        let ctx2=egui::Context::default();let mut ok=true;
        let _=ctx2.run(egui::RawInput::default(),|ctx|{ctx.memory_mut(|m|m.open_popup(egui::Id::new("ing-popup")));ok=esc_is_stop(ctx);});assert!(!ok);
        assert_eq!(field_commit(true,false," Melk ","Water"),Some("Melk".into()));assert_eq!(field_commit(true,true,"Melk","Water"),None);assert_eq!(field_commit(false,false,"Melk","Water"),None);
    }
    /// N4: no page announces a later iteration any more (source of every UI file).
    #[test]fn n4_no_page_follows_later(){
        for (n,src) in [("mod",include_str!("mod.rs")),("shell",include_str!("shell.rs")),("pumps",include_str!("pumps.rs")),("calibration",include_str!("calibration.rs")),("testpage",include_str!("testpage.rs")),
            ("recipes",include_str!("recipes.rs")),("ingredients",include_str!("ingredients.rs")),("production",include_str!("production.rs")),("machine",include_str!("machine.rs")),("analysis",include_str!("analysis.rs")),
            ("monitors",include_str!("monitors.rs")),("log",include_str!("log.rs")),("settings",include_str!("settings.rs"))]{
            assert!(!src.contains(&["Volgt"," in iteratie"].concat()) && !src.contains(&["later","_page"].concat()),"{n}");
        }
        assert_eq!(Page::ALL.len(),11);
    }
    #[test]fn every_page_renders_in_both_profiles_without_devices(){
        let root=std::env::temp_dir().join(format!("pumpstudio-ui-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let mut app=App::open(root.clone()).unwrap();
        assert_eq!(app.pumps.len(),192);let n=render_all_pages(&mut app).unwrap();assert_eq!(n,66);
        assert_eq!(app.profile(),ProfileKind::Vsd);assert_eq!(app.pumps.len(),192);
        app.switch_profile(ProfileKind::Keramisch);assert_eq!(app.pumps.len(),1);
        applog::set_sink(None);drop(app);
        for _ in 0..100{if std::fs::remove_dir_all(&root).is_ok() || !root.exists(){break;}std::thread::sleep(Duration::from_millis(20));}
        assert!(!root.exists(),"testmap opgeruimd");
    }
    #[test]fn k8_profile_switch_keeps_frames_short(){
        let root=std::env::temp_dir().join(format!("pumpstudio-ui-k8-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let mut app=App::open(root.clone()).unwrap();app.page=Page::Instellingen;
        for id in DevId::VSD{app.dev.connect(id,true).unwrap();}
        let t=Instant::now();while !DevId::VSD.iter().all(|d|app.dev.status(*d).link==Link::On){app.tick();assert!(t.elapsed()<Duration::from_secs(5));std::thread::sleep(Duration::from_millis(5));}
        app.dev.pumps(vec!["P_C01".into()],pump_studio::machine::PumpOp::Dispense,40*51200).unwrap();
        let ctx=egui::Context::default();style(&ctx);
        let (max,total,n)=super::switch_frames(&mut app,&ctx).unwrap();
        eprintln!("K8 (debug): wissel in {total:?} over {n} frames, langste frame {max:?}");
        // Debug build under parallel test load: generous; the release smoke checks the 100 ms of K8.
        assert!(max<Duration::from_millis(250),"{max:?}");assert_eq!(app.pumps.len(),1);
        app.dev.shutdown();applog::set_sink(None);drop(app);
        for _ in 0..100{if std::fs::remove_dir_all(&root).is_ok() || !root.exists(){break;}std::thread::sleep(Duration::from_millis(20));}
    }
}
