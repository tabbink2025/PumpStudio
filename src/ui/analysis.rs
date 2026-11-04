//! Analyse: dashboard (today / 7 days), production deviation per ingredient and pump, pots per recipe, history per pump
//! (calibration g/stroke trend, test deviation per weight over time); filters period / head / simulation; export of the
//! filtered selection to CSV (NL) and Excel. Queries and exports run on a background thread.
use super::*;
use egui_plot::{Legend,Line,Plot,Points};
use pump_studio::analysis::{self,Analysis,Filter};
use std::sync::mpsc;

pub struct AnaUi{
    pub days:i64,pub head:usize,pub include_sim:bool,pub pump:String,
    result:Option<Analysis>,rx:Option<mpsc::Receiver<Result<Analysis,String>>>,export_rx:Option<mpsc::Receiver<Result<Vec<PathBuf>,String>>>,
    key:Option<(i64,usize,bool,u64)>,pub error:String,
}
impl Default for AnaUi{fn default()->Self{Self{days:30,head:0,include_sim:false,pump:String::new(),result:None,rx:None,export_rx:None,key:None,error:String::new()}}}
fn n2(v:f64)->String{pump_studio::pumptest::nl(v,2)}
fn s2(v:f64)->String{pump_studio::pumptest::nls(v,2)}

impl App{
    /// A result is there and no calculation is running (smoke test).
    pub fn anaui_ready(&self)->bool{self.anaui.result.is_some() && self.anaui.rx.is_none()}
    fn ana_filter(&self)->Filter{
        let now=analysis::now_ms();
        Filter{profile:Some(self.profile()),from_ms:if self.anaui.days>0{Some(now-self.anaui.days*86_400_000)}else{None},to_ms:None,
            head:match self.anaui.head{1=>Some("LH1".into()),2=>Some("LH2".into()),_=>None},include_sim:self.anaui.include_sim}
    }
    /// Start a computation on a background thread when the filter or the data changed.
    fn ana_refresh(&mut self,force:bool){
        if let Some(rx)=&self.anaui.rx{match rx.try_recv(){
            Ok(r)=>{self.anaui.rx=None;match r{Ok(a)=>{applog::info("ANALYSE",&format!("Analyse berekend in {} ms ({} potten, {} items, {} testdoses, {} kalibraties)",a.took_ms,a.pots.len(),a.items.len(),a.tests.len(),a.cals.len()));self.anaui.error.clear();self.anaui.result=Some(a);}Err(e)=>self.anaui.error=e}}
            Err(mpsc::TryRecvError::Empty)=>return,Err(_)=>{self.anaui.rx=None;}
        }}
        let key=(self.anaui.days,self.anaui.head,self.anaui.include_sim,self.data_ver+self.cal_seen.0+self.cal_seen.1+self.dev.prod.lock().unwrap().version+self.dev.pot.lock().unwrap().version);
        if !force && self.anaui.key==Some(key){return;}
        self.anaui.key=Some(key);
        let f=self.ana_filter();let path=self.db.path().to_path_buf();let (tx,rx)=mpsc::channel();
        let _=std::thread::Builder::new().name("analyse".into()).spawn(move||{
            let r=Db::open(&path).and_then(|d|d.set_busy_timeout(Duration::from_secs(5)).map(|_|d)).and_then(|d|d.analysis(&f));let _=tx.send(r);});
        self.anaui.rx=Some(rx);
    }
    pub fn analysis_page(&mut self,ui:&mut egui::Ui){
        page_title(ui,"Analyse","Historie per pomp, productie per ingrediënt en recept, en een overzicht van vandaag en de laatste 7 dagen. Uitgesloten testdoses tellen nooit mee.");
        self.ana_refresh(false);
        if let Some(rx)=&self.anaui.export_rx{if let Ok(r)=rx.try_recv(){self.anaui.export_rx=None;
            match r{Ok(p)=>{let t=format!("Export opgeslagen: {}",p.iter().map(|x|x.display().to_string()).collect::<Vec<_>>().join(", "));applog::info("ANALYSE",&t);self.message=t;
                if let Some(d)=p.first().and_then(|x|x.parent()){let _=std::process::Command::new("explorer.exe").arg(d).spawn();}}
                Err(e)=>self.message=format!("Export mislukt: {e}")}}}
        card(ui,"Selectie",|ui|{
            ui.horizontal_wrapped(|ui|{
                ui.label("Periode");egui::ComboBox::from_id_salt("an-days").width(110.).selected_text(match self.anaui.days{1=>"vandaag (24 u)",7=>"7 dagen",30=>"30 dagen",90=>"90 dagen",365=>"1 jaar",_=>"alles"}).show_ui(ui,|ui|{
                    for (d,t) in [(1,"vandaag (24 u)"),(7,"7 dagen"),(30,"30 dagen"),(90,"90 dagen"),(365,"1 jaar"),(0,"alles")]{ui.selectable_value(&mut self.anaui.days,d,t);}});
                ui.label("Kop");for (k,t) in ["beide","LH1","LH2"].iter().enumerate(){ui.selectable_value(&mut self.anaui.head,k,*t);}
                ui.checkbox(&mut self.anaui.include_sim,"simulatie meetellen");
                if ui.button("Vernieuwen").clicked(){self.ana_refresh(true);}
                if self.anaui.rx.is_some(){ui.spinner();ui.label("berekenen…");}
            });
            if self.anaui.include_sim{ui.label(RichText::new("LET OP: simulatie telt mee in deze cijfers en in de export (kolom Soort).").strong().color(Color32::WHITE).background_color(DANGER));}
            else{hint(ui,"Alleen hardware (simulatie staat standaard uit).");}
            ui.horizontal_wrapped(|ui|{
                let can=self.anaui.result.is_some() && self.anaui.export_rx.is_none();
                for (xlsx,t) in [(true,"Export Excel (.xlsx)"),(false,"Export CSV")]{
                    if ui.add_enabled(can,egui::Button::new(t)).clicked(){
                        self.click(&format!("Analyse {t}"));
                        let a=self.anaui.result.clone().unwrap();let dir=self.dev.settings.output.join("analyse");let (tx,rx)=mpsc::channel();
                        let _=std::thread::Builder::new().name("export".into()).spawn(move||{let _=tx.send(analysis::export(&a,&dir,xlsx));});
                        self.anaui.export_rx=Some(rx);
                    }
                }
                if self.anaui.export_rx.is_some(){ui.spinner();}
                hint(ui,"Export van de selectie: tabbladen Potten, Ingrediënten, Testen, Kalibraties (CSV: één bestand per tabel, puntkomma en decimale komma). Map results\\analyse.");
            });
            if !self.anaui.error.is_empty(){ui.label(RichText::new(&self.anaui.error).color(DANGER));}
        });
        let Some(a)=self.anaui.result.clone() else{ui.add_space(8.);card(ui,"",|ui|empty(ui,"Berekenen…",60.));return};
        ui.add_space(8.);self.dashboard_card(ui,&a);ui.add_space(8.);
        ui.columns(2,|c|{
            card(&mut c[0],"Productie per ingrediënt",|ui|dev_table(ui,"an-ing",&a.ingredients,true));
            card(&mut c[1],"Potten per recept",|ui|{
                if a.recipes.is_empty(){empty(ui,"Geen potten in deze selectie.",40.);return;}
                egui::Grid::new("an-rec").striped(true).spacing([12.,3.]).show(ui,|ui|{
                    for t in ["Recept","Potten","Binnen tolerantie","%"]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                    for r in &a.recipes{ui.label(&r.recipe);ui.label(r.pots.to_string());ui.label(r.within.to_string());ui.label(RichText::new(format!("{} %",pump_studio::pumptest::nl(r.within_pct,1))).color(if r.within_pct>=95.{OK}else{WARN}));ui.end_row();}
                });
                hint(ui,"Pot binnen tolerantie = klaar en alle gewogen ingrediënten binnen hun tolerantie.");
            });
        });
        ui.add_space(8.);
        card(ui,"Productie per pomp",|ui|dev_table(ui,"an-pump",&a.pumps,false));
        ui.add_space(8.);self.pump_history(ui,&a);
    }
    fn dashboard_card(&mut self,ui:&mut egui::Ui,a:&Analysis){
        let d=&a.dash;
        card(ui,"Overzicht",|ui|{
            ui.horizontal_wrapped(|ui|{
                for (t,n,p) in [("Vandaag",d.today,d.today_within_pct),("Laatste 7 dagen",d.week,d.week_within_pct)]{
                    Frame::new().fill(BG).corner_radius(CornerRadius::same(6)).inner_margin(Margin::same(10)).show(ui,|ui|{ui.set_min_width(180.);
                        ui.label(RichText::new(t).color(MUTED));ui.label(RichText::new(format!("{n} pot(ten)")).size(22.).strong());
                        ui.label(RichText::new(p.map(|p|format!("{} % binnen tolerantie",pump_studio::pumptest::nl(p,1))).unwrap_or("—".into())).color(if p.is_some_and(|p|p>=95.){OK}else{WARN}));});
                }
                for (t,v) in [("Slechtste ingrediënten (7 dagen)",&d.worst_ingredients),("Slechtste pompen (7 dagen)",&d.worst_pumps)]{
                    Frame::new().fill(BG).corner_radius(CornerRadius::same(6)).inner_margin(Margin::same(10)).show(ui,|ui|{ui.set_min_width(220.);
                        ui.label(RichText::new(t).color(MUTED));
                        if v.is_empty(){ui.label("—");}
                        for x in v.iter(){ui.label(format!("{}  gem. |afw.| {} % (n {})",x.name,n2(x.mean_abs_pct),x.n));}});
                }
            });
            hint(ui,&format!("Vandaag = sinds 0:00; klaar-potten. Kop- en simulatiefilter gelden ook hier.{}",if a.filter.include_sim{" SIMULATIE TELT MEE."}else{""}));
        });
    }
    fn pump_history(&mut self,ui:&mut egui::Ui,a:&Analysis){
        let mut pumps:Vec<String>=a.cals.iter().map(|c|c.pump.clone()).chain(a.tests.iter().map(|t|t.pump.clone())).collect();pumps.sort();pumps.dedup();
        if (self.anaui.pump.is_empty() || !pumps.contains(&self.anaui.pump)) && !pumps.is_empty(){self.anaui.pump=pumps[0].clone();}
        card(ui,"Historie per pomp",|ui|{
            if pumps.is_empty(){empty(ui,"Geen kalibraties of testen in deze selectie.",40.);return;}
            ui.horizontal(|ui|{ui.label("Pomp");egui::ComboBox::from_id_salt("an-pump").width(120.).height(300.).selected_text(&self.anaui.pump).show_ui(ui,|ui|{for p in &pumps{ui.selectable_value(&mut self.anaui.pump,p.clone(),p);}});});
            let p=self.anaui.pump.clone();
            let cals:Vec<&analysis::CalA>=a.cals.iter().filter(|c|c.pump==p).collect();
            let tests:Vec<&analysis::TestA>=a.tests.iter().filter(|t|t.pump==p).collect();
            let t0=cals.iter().map(|c|c.date_ms).chain(tests.iter().map(|t|t.time_ms)).min().unwrap_or(0);
            let day=|ms:i64|(ms-t0) as f64/86_400_000.;
            ui.columns(2,|c|{
                section(&mut c[0],"KALIBRATIE g/slag (of g/omw) DOOR DE TIJD");
                Plot::new("an-cal").height(180.).x_axis_label("dagen").legend(Legend::default()).allow_scroll(false).show(&mut c[0],|pl|{
                    let pts:Vec<[f64;2]>=cals.iter().filter_map(|c|c.g_per_stroke.or(c.g_per_rev).map(|g|[day(c.date_ms),g])).collect();
                    if !pts.is_empty(){pl.line(Line::new("g/slag",pts.clone()).color(ACCENT));pl.points(Points::new("kalibraties",pts).radius(3.5_f32).color(ACCENT));}
                });
                section(&mut c[1],"POMPTEST: AFWIJKING % PER GEWICHT DOOR DE TIJD");
                Plot::new("an-test").height(180.).x_axis_label("dagen").y_axis_label("%").legend(Legend::default()).allow_scroll(false).show(&mut c[1],|pl|{
                    let mut ws:Vec<f64>=tests.iter().map(|t|t.requested_g).collect();ws.sort_by(f64::total_cmp);ws.dedup();
                    for w in ws{let pts:Vec<[f64;2]>=tests.iter().filter(|t|t.requested_g==w).map(|t|[day(t.time_ms),t.err_pct]).collect();pl.points(Points::new(format!("{} g",pump_studio::pumptest::nl(w,3)),pts).radius(3.0_f32));}
                });
            });
            let stats:Vec<&analysis::TestStat>=a.test_stats.iter().filter(|s|s.pump==p).collect();
            if !stats.is_empty(){egui::Grid::new("an-ts").striped(true).spacing([12.,3.]).show(ui,|ui|{
                for t in ["Gewicht g","n","Gem. afw. %","Spreiding %","Van","Tot"]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                for s in stats{ui.label(pump_studio::pumptest::nl(s.requested_g,3));ui.label(s.n.to_string());ui.label(s2(s.mean_pct));ui.label(n2(s.sd_pct));ui.label(applog::local_time(s.first_ms));ui.label(applog::local_time(s.last_ms));ui.end_row();}
            });}
            hint(ui,&format!("{} kalibratie(s) (ook geïmporteerde en archief), {} testdosis/doses (uitgesloten doses tellen niet).",cals.len(),tests.len()));
        });
    }
}
fn dev_table(ui:&mut egui::Ui,id:&str,v:&[analysis::DevStat],attr:bool){
    if v.is_empty(){empty(ui,"Geen gewogen ingrediënten in deze selectie.",40.);return;}
    egui::ScrollArea::vertical().id_salt(id).max_height(260.).show(ui,|ui|egui::Grid::new(id).striped(true).spacing([10.,3.]).show(ui,|ui|{
        let mut h=vec!["Naam","n","Gem. afw. %","Gem. |afw.| %","Spreiding %","Binnen tol."];if attr{h.push("Toegerekend");}
        for t in h{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
        for x in v{ui.label(&x.name);ui.label(x.n.to_string());ui.label(s2(x.mean_pct));ui.label(n2(x.mean_abs_pct));ui.label(n2(x.sd_pct));
            ui.label(RichText::new(format!("{}/{} ({} %)",x.within,x.n,pump_studio::pumptest::nl(x.within_pct,0))).color(if x.within==x.n{OK}else{WARN}));
            if attr{ui.label(if x.attributed>0{format!("{} van {}",x.attributed,x.n)}else{String::new()});}ui.end_row();}
    }));
    if attr{hint(ui,"Toegerekend = parallel gedoseerd: het nettogewicht van de kop is naar verhouding verdeeld. Deze waarden tellen mee; het aandeel staat erbij.");}
}
