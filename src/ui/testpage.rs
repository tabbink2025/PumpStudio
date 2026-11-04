//! Pomptest: test setup (one weight, series, random with seed), run with live table and chart, correction in
//! 0,1 % steps (also between doses), pump settings between doses, results per weight, manual exclusion
//! with reason, history, Word report and CSV export.
use super::*;
use egui_plot::{Corner,Legend,Line,MarkerShape,Plot,Points};
use pump_studio::{calib,devices::{parse_decimal,DevId,Motion},pumptest::{self,DoseRow,Mode,TestPlan},testdb::RunRow};

pub struct TestUi{
    pub pump:String,pub plan:TestPlan,pub weight_text:String,pub series_text:String,pub min_text:String,pub max_text:String,pub tol_text:String,pub seed_text:String,
    pub corr_text:String,pub last_n:usize,pub speed_text:String,pub accel_text:String,pub decel_text:String,
    runs:Vec<RunRow>,doses:Vec<DoseRow>,pub shown:Option<i64>,pub chosen:std::collections::BTreeSet<i64>,key:Option<(ProfileKind,String,u64,Option<i64>)>,
    excl:Option<(i64,String)>,cal_info:Option<(String,Result<String,String>)>,
}
impl Default for TestUi{
    fn default()->Self{
        let plan=TestPlan{seed:seed(),..Default::default()};
        Self{pump:String::new(),weight_text:"5".into(),series_text:"0,5; 1; 2; 3; 4; 5; 10; 15; 20".into(),min_text:"0,5".into(),max_text:"5".into(),tol_text:"1".into(),
            seed_text:plan.seed.to_string(),plan,corr_text:"0".into(),last_n:5,speed_text:"1".into(),accel_text:"1".into(),decel_text:"1".into(),
            runs:vec![],doses:vec![],shown:None,chosen:Default::default(),key:None,excl:None,cal_info:None}
    }
}
fn seed()->u64{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_nanos() as u64%1_000_000_007).unwrap_or(1)|1}
/// "0,5; 1; 2" or one per line; comma is the decimal sign, so separate with ';', spaces or new lines.
pub fn parse_weights(text:&str)->Result<Vec<f64>,String>{
    let v:Vec<f64>=text.split([';','\n','\t',' ']).filter(|t|!t.trim().is_empty()).map(|t|parse_decimal(t).and_then(|g|if g>0.{Ok(g)}else{Err(format!("Gewicht moet positief zijn: {t}"))})).collect::<Result<_,_>>()?;
    if v.is_empty(){return Err("Vul minimaal één gewicht in".into());}Ok(v)
}
fn n3(v:f64)->String{pumptest::nl(v,3)}
fn n4(v:f64)->String{pumptest::nl(v,4)}

impl App{
    fn test_refresh(&mut self){
        if self.ptui.pump.is_empty() || !self.pumps.iter().any(|p|p.name==self.ptui.pump){
            // Chosen on Pompen, else the first calibrated active pump, else the first active one.
            self.ptui.pump=self.pumps.iter().find(|p|self.sel.contains(&p.id) && p.enabled).or(self.pumps.iter().find(|p|p.enabled && self.cal_dates.contains_key(&p.name)))
                .or(self.pumps.iter().find(|p|p.enabled)).map(|p|p.name.clone()).unwrap_or_default();}
        // While a test runs the table follows it.
        if let Some(t)=self.dev.running_test(){if t.pump==self.ptui.pump{self.ptui.shown=Some(t.run);}}
        let key=(self.profile(),self.ptui.pump.clone(),self.dev.test_version(),self.ptui.shown);
        if self.ptui.key.as_ref()==Some(&key){return;}
        self.ptui.key=Some(key);
        self.ptui.runs=self.db.test_runs(self.profile(),&self.ptui.pump).unwrap_or_default();
        if self.ptui.shown.is_none_or(|id|!self.ptui.runs.iter().any(|r|r.id==id)){self.ptui.shown=self.ptui.runs.first().map(|r|r.id);}
        self.ptui.doses=self.ptui.shown.and_then(|id|self.db.test_doses(id).ok()).unwrap_or_default();
        self.ptui.chosen.retain(|id|key_has(&self.ptui.runs,*id));
        // Which calibration a test would use now (mode rule of iteration 2).
        let hardware=match self.profile(){ProfileKind::Vsd=>{let s=self.dev.status(DevId::Can);s.link==Link::On && !s.simulation},ProfileKind::Keramisch=>self.dev.bench.connected && !self.dev.bench.simulation};
        let kind=if self.profile()==ProfileKind::Vsd{calib::KIND_VSD}else{calib::KIND_BENCH_CURVE};
        let info=self.db.calibration_for_dosing(self.profile(),&self.ptui.pump,kind,hardware)
            .map(|c|format!("nr {} · {} · {}{}",c.id,applog::local_time(c.date_ms),if c.simulation{"simulatie"}else{"hardware"},c.g_per_stroke.or(c.g_per_rev).map(|g|format!(" · {} g/{}",n4(g),if c.g_per_stroke.is_some(){"slag"}else{"omw"})).unwrap_or_default()));
        self.ptui.cal_info=Some((self.ptui.pump.clone(),info));
    }
    /// Plan from the form; every refusal is shown.
    fn test_plan(&self)->Result<TestPlan,String>{
        let u=&self.ptui;let mut p=u.plan.clone();
        p.tolerance_pct=parse_decimal(&u.tol_text).map_err(|e|format!("Tolerantie: {e}"))?;
        p.seed=u.seed_text.trim().parse::<u64>().map_err(|_|"Zaad: geheel getal".to_string())?;
        match p.mode{
            Mode::Enkel=>p.weights=vec![parse_decimal(&u.weight_text).map_err(|e|format!("Gewicht: {e}"))?],
            Mode::Reeks=>p.weights=parse_weights(&u.series_text)?,
            Mode::Willekeurig=>{p.min=parse_decimal(&u.min_text).map_err(|e|format!("Minimum: {e}"))?;p.max=parse_decimal(&u.max_text).map_err(|e|format!("Maximum: {e}"))?;}
        }
        p.doses()?;Ok(p)
    }
    pub fn pumptest_page(&mut self,ui:&mut egui::Ui){
        self.test_refresh();
        page_title(ui,"Pomptest","Doseertest met de actieve kalibratie in de juiste modus (hardware alleen met een hardware-kalibratie). Elke dosis wordt direct opgeslagen; STOP / Esc stopt binnen 1 s.");
        let wide=ui.available_width()>=1000.;
        if wide{ui.horizontal_top(|ui|{
            ui.vertical(|ui|{ui.set_width(370.);self.test_setup(ui);});
            ui.vertical(|ui|self.test_results(ui));
        });}else{self.test_setup(ui);ui.add_space(8.);self.test_results(ui);}
    }
    fn test_setup(&mut self,ui:&mut egui::Ui){
        let running=self.dev.running_test();let vsd=self.profile()==ProfileKind::Vsd;
        card(ui,"Testopzet",|ui|{
            let names:Vec<String>=self.pumps.iter().filter(|p|p.enabled).map(|p|p.name.clone()).collect();
            ui.horizontal(|ui|{ui.label("Pomp");ui.add_enabled_ui(running.is_none(),|ui|{
                egui::ComboBox::from_id_salt("pt-pump").width(120.).height(320.).selected_text(&self.ptui.pump).show_ui(ui,|ui|{
                    for n in &names{if ui.selectable_label(*n==self.ptui.pump,n).clicked(){self.ptui.pump=n.clone();self.ptui.shown=None;}}});});});
            match &self.ptui.cal_info{Some((_,Ok(t)))=>hint(ui,&format!("Kalibratie: {t}")),Some((_,Err(e)))=>{ui.label(RichText::new(e).color(DANGER));},None=>{}}
            ui.add_enabled_ui(running.is_none(),|ui|{
                ui.horizontal(|ui|{for m in [Mode::Enkel,Mode::Reeks,Mode::Willekeurig]{ui.selectable_value(&mut self.ptui.plan.mode,m,m.label());}});
                egui::Grid::new("pt-plan").num_columns(2).spacing([12.,5.]).show(ui,|ui|{
                    let u=&mut self.ptui;
                    match u.plan.mode{
                        Mode::Enkel=>{ui.label("Gewicht");ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut u.weight_text).desired_width(80.));ui.label("g");});ui.end_row();
                            ui.label("Herhalingen");ui.add(egui::DragValue::new(&mut u.plan.repeats).range(1..=pumptest::MAX_DOSES));ui.end_row();}
                        Mode::Reeks=>{ui.label("Gewichten (g)");ui.add(egui::TextEdit::singleline(&mut u.series_text).desired_width(200.)).on_hover_text("Gescheiden door ; of spatie; komma is decimaal (0,5; 1; 2)");ui.end_row();
                            ui.label("Herhalingen per gewicht");ui.add(egui::DragValue::new(&mut u.plan.repeats).range(1..=pumptest::MAX_DOSES));ui.end_row();
                            ui.label("Volgorde");ui.horizontal(|ui|{ui.selectable_value(&mut u.plan.shuffle,false,"oplopend");ui.selectable_value(&mut u.plan.shuffle,true,"door elkaar");});ui.end_row();}
                        Mode::Willekeurig=>{ui.label("Aantal doses");ui.add(egui::DragValue::new(&mut u.plan.n).range(1..=pumptest::MAX_DOSES));ui.end_row();
                            ui.label("Tussen (g)");ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut u.min_text).desired_width(60.));ui.label("en");ui.add(egui::TextEdit::singleline(&mut u.max_text).desired_width(60.));});ui.end_row();}
                    }
                    if u.plan.mode==Mode::Willekeurig || u.plan.shuffle && u.plan.mode==Mode::Reeks{
                        ui.label("Zaad");ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut u.seed_text).desired_width(110.));if ui.small_button("Nieuw").clicked(){u.seed_text=seed().to_string();}});ui.end_row();}
                    ui.label("Tolerantie ±");ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut u.tol_text).desired_width(50.));ui.label("%");});ui.end_row();
                    ui.label("Wachttijd stabiel");ui.add(egui::DragValue::new(&mut u.plan.settle_s).speed(0.1).range(0. ..=60.).suffix(" s"));ui.end_row();
                    ui.label("Tarra per dosis");ui.add_enabled(vsd,egui::Checkbox::new(&mut u.plan.tare_each,"")).on_hover_text(if vsd{"Uit: de weging na de vorige dosis is het nulpunt (sneller)"}else{"Pompbank: altijd wegen voor en na"});ui.end_row();
                    ui.label("Pauze na elke dosis");ui.checkbox(&mut u.plan.pause_each,"");ui.end_row();
                });
            });
            let plan=self.test_plan();
            match &plan{
                Ok(p)=>{let d=p.doses().unwrap_or_default();let (lo,hi)=d.iter().fold((f64::MAX,f64::MIN),|(a,b),g|(a.min(*g),b.max(*g)));
                    hint(ui,&format!("{} doses, {} – {} g, totaal {} g{}",d.len(),n3(lo),n3(hi),n3(d.iter().sum()),if d.len()<=12{format!(": {}",d.iter().map(|g|n3(*g)).collect::<Vec<_>>().join("; "))}else{String::new()}));}
                Err(e)=>{ui.label(RichText::new(e).color(DANGER));}
            }
            if running.is_none(){
                if primary(ui,plan.is_ok() && self.ptui.cal_info.as_ref().is_some_and(|c|c.1.is_ok()),"Test starten"){
                    if let Ok(p)=plan{self.ptui.plan=p.clone();self.click(&format!("Pomptest starten {} ({}, {} doses)",self.ptui.pump,p.mode.label(),p.doses().map(|d|d.len()).unwrap_or(0)));
                        match self.dev.start_test(&mut self.db,&self.ptui.pump.clone(),&p){Ok(id)=>{self.ptui.shown=Some(id);self.ptui.key=None;},Err(e)=>self.message=e}}
                }
            }else if let Some(t)=&running{
                ui.add(egui::ProgressBar::new(t.done as f32/t.n.max(1) as f32).text(format!("test {} · {} / {} doses",t.run,t.done,t.n)).fill(ACCENT));
                if !t.last.is_empty(){hint(ui,&t.last);}
                if t.waiting{if primary(ui,true,"Volgende dosis"){self.click("Volgende dosis");self.dev.resume_test();}}
                hint(ui,"STOP / Esc stopt de test; de doses tot dan blijven bewaard (status gestopt).");
            }
        });
        ui.add_space(8.);
        card(ui,"Correctie",|ui|{
            let cur=self.dev.correction();
            ui.label(RichText::new(format!("{} %",pumptest::nls(cur,1))).size(20.).strong().monospace());
            ui.horizontal_wrapped(|ui|{
                for d in [-1.,-0.5,-0.1,0.1,0.5,1.]{if ui.button(format!("{}",pumptest::nls(d,1))).clicked(){self.set_corr(cur+d);}}
                if ui.button("0").clicked(){self.set_corr(0.);}
            });
            ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut self.ptui.corr_text).desired_width(60.));ui.label("%");
                if ui.button("Toepassen").clicked(){match parse_decimal(&self.ptui.corr_text){Ok(v)=>self.set_corr(v),Err(e)=>self.message=e}}});
            let rec=pumptest::recommended_correction(&self.ptui.doses,self.ptui.last_n);
            ui.horizontal(|ui|{
                ui.label("Aanbevolen");ui.add(egui::DragValue::new(&mut self.ptui.last_n).range(1..=100).prefix("laatste ").suffix(" doses"));
                match rec{Some((r,a,b,_))=>{ui.label(RichText::new(format!("{} %",pumptest::nls(r,1))).strong()).on_hover_text(format!("op basis van dosis {a}–{b}"));if ui.button("Overnemen").clicked(){self.set_corr(r);}}None=>{hint(ui,"—");}}
            });
            if let Some((_,a,b,sk))=rec{hint(ui,&format!("Aanbevolen op basis van dosis {a}–{b} (uitgesloten doses tellen niet){}.",if sk>0{format!("; {sk} dosis/doses van 0 g of minder overgeslagen")}else{String::new()}));}
            hint(ui,"−20 … +20 % in stappen van 0,1 %; geldt vanaf de volgende dosis en wordt per dosis vastgelegd. Aanbevolen = per dosis de correctie die precies het gevraagde gewicht had gegeven, gemiddeld over de laatste doses; alleen met een klik overnemen.");
        });
        ui.add_space(8.);
        card(ui,"Pompinstellingen",|ui|{
            if vsd{
                egui::Grid::new("pt-motion").num_columns(2).spacing([12.,5.]).show(ui,|ui|{
                    for (l,t) in [("Snelheid (omw/s)",&mut self.ptui.speed_text),("Acceleratie (omw/s²)",&mut self.ptui.accel_text),("Deceleratie (omw/s²)",&mut self.ptui.decel_text)]{ui.label(l);ui.add(egui::TextEdit::singleline(t).desired_width(60.));ui.end_row();}
                });
                if ui.button("Toepassen (vanaf volgende dosis)").clicked(){
                    let m=(||Ok::<_,String>(Motion{speed_rps:parse_decimal(&self.ptui.speed_text)?,accel_rps2:parse_decimal(&self.ptui.accel_text)?,decel_rps2:parse_decimal(&self.ptui.decel_text)?}))();
                    self.click("Pompinstellingen toepassen");let r=m.and_then(|m|self.dev.set_motion(m));self.result(r);
                }
                hint(ui,"Zonder opnieuw verbinden; de instellingen staan bij elke dosis in de tabel en het CSV.");
            }else{hint(ui,"Pomptype, stappen/omw en snelheid: Instellingen → Toepassen (werkt live tussen doses); per dosis vastgelegd.");}
        });
    }
    fn set_corr(&mut self,v:f64){self.click(&format!("Correctie {}",pumptest::nls(v,1)));match self.dev.set_correction(v){Ok(c)=>self.ptui.corr_text=pumptest::nl(c,1),Err(e)=>self.message=e}}
    fn test_results(&mut self,ui:&mut egui::Ui){
        let run=self.ptui.shown.and_then(|id|self.ptui.runs.iter().find(|r|r.id==id).cloned());
        let tol=run.as_ref().map(|r|r.tolerance_pct).unwrap_or(1.);
        let doses=self.ptui.doses.clone();
        card(ui,&match &run{Some(r)=>format!("Test {} · {} · {} · {}",r.id,applog::local_time(r.started_ms),r.mode_text(),r.status),None=>"Resultaat".into()},|ui|{
            if run.is_none(){empty(ui,"Nog geen test voor deze pomp.",60.);return;}
            if run.as_ref().is_some_and(|r|r.simulation){ui.label(RichText::new("SIMULATIE").strong().color(WARN));}
            Plot::new("pt-plot").height(190.).x_axis_label("dosis").y_axis_label("afwijking %").legend(Legend::default().position(Corner::LeftTop)).allow_scroll(false).show(ui,|pl|{
                let n=doses.len().max(1) as f64+0.5;
                for s in [tol,-tol]{pl.line(Line::new("tolerantie",vec![[0.5,s],[n,s]]).color(MUTED).style(egui_plot::LineStyle::dashed_loose()));}
                let pts=|ex:bool|doses.iter().filter(|d|d.excluded==ex).filter_map(|d|d.err_pct().map(|e|[d.nr as f64,e])).collect::<Vec<_>>();
                pl.points(Points::new("afwijking",pts(false)).radius(3.5_f32).color(ACCENT));
                let ex=pts(true);if !ex.is_empty(){pl.points(Points::new("uitgesloten",ex).radius(3.5_f32).shape(MarkerShape::Cross).color(MUTED));}
            });
            // Results per weight (excluded doses do not count, but are counted).
            let stats=pumptest::summary(&doses,tol);
            egui::ScrollArea::horizontal().id_salt("pt-stats-h").show(ui,|ui|egui::Grid::new("pt-stats").striped(true).spacing([12.,3.]).show(ui,|ui|{
                for t in ["Gevraagd","n","Gem. afw. %","Gem. afw. g","Spreiding %","Kleinste / grootste %",&format!("Binnen ±{} %",pumptest::nl(tol,1)),"g/min","Uitgesloten"]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                for w in stats.iter().chain(pumptest::overall(&doses,tol).iter()){
                    ui.label(&w.label);ui.label(w.n.to_string());ui.label(pumptest::nls(w.mean_err_pct,2));ui.label(pumptest::nls(w.mean_err_g,4));ui.label(pumptest::nl(w.sd_pct,2));
                    ui.label(if w.n>0{format!("{} / {}",pumptest::nls(w.min_pct,2),pumptest::nls(w.max_pct,2))}else{"—".into()});
                    ui.label(RichText::new(format!("{}/{} ({} %)",w.within,w.n,pumptest::nl(w.within_pct,0))).color(if w.within==w.n && w.n>0{OK}else{WARN}));
                    ui.label(w.g_per_min.map(|g|pumptest::nl(g,1)).unwrap_or("—".into()));ui.label(if w.excluded>0{w.excluded.to_string()}else{String::new()});ui.end_row();
                }
            }));
            if self.profile()==ProfileKind::Keramisch{
                let small:Vec<&pumptest::WeightStats>=stats.iter().filter(|w|w.requested<=5. && w.n>=2).collect();
                if !small.is_empty(){hint(ui,&format!("Druppelgrootte (indicatie): spreiding {}.{}",small.iter().map(|w|format!("{} → {} g ({} %)",w.label,n4(w.sd_g),pumptest::nl(w.sd_pct,2))).collect::<Vec<_>>().join(" · "),
                    pumptest::smallest_within(&stats,tol).map(|g|format!(" Kleinste geteste dosis met spreiding binnen ±{} %: {} g.",pumptest::nl(tol,1),n3(g))).unwrap_or_default()));}
            }
        });
        ui.add_space(8.);
        card(ui,"Doses",|ui|{
            if doses.is_empty(){empty(ui,"Nog geen doses.",40.);return;}
            let mut action:Option<(i64,bool,String)>=None;
            egui::ScrollArea::both().id_salt("pt-doses").max_height(260.).show(ui,|ui|{
                egui::Grid::new("pt-dose-g").striped(true).spacing([10.,3.]).show(ui,|ui|{
                    for t in ["Nr","Gevraagd g","Gedoseerd g","Afw. g","Afw. %","Binnen","Corr. %","Duur s","Status",""]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                    for d in &doses{
                        let txt=|s:String|{let r=RichText::new(s).monospace();if d.excluded{r.strikethrough().color(MUTED)}else{r}};
                        ui.label(txt(d.nr.to_string()));ui.label(txt(n3(d.requested_g)));ui.label(txt(d.dosed_g.map(n4).unwrap_or("—".into())));
                        ui.label(txt(d.err_g().map(|e|pumptest::nls(e,4)).unwrap_or("—".into())));ui.label(txt(d.err_pct().map(|e|pumptest::nls(e,2)).unwrap_or("—".into())));
                        match d.within(tol){Some(true)=>{ui.label(RichText::new("ja").color(OK));}Some(false)=>{ui.label(RichText::new("nee").color(DANGER));}None=>{ui.label("—");}}
                        ui.label(txt(pumptest::nls(d.correction_pct,1)));ui.label(txt(d.duration_s.map(|s|pumptest::nl(s,1)).unwrap_or("—".into())));
                        ui.label(if d.status=="ok"{RichText::new("ok")}else{RichText::new(&d.status).color(DANGER)}).on_hover_text(format!("{}{}",d.error,if d.settings.is_empty(){String::new()}else{format!("\nInstellingen: {}",d.settings)}));
                        ui.horizontal(|ui|{
                            if d.excluded{ui.label(RichText::new(format!("uitgesloten: {}",d.exclude_reason)).size(12.).color(MUTED));if ui.small_button("Meetellen").clicked(){action=Some((d.id,false,String::new()));}}
                            else if d.dosed_g.is_some(){
                                match &mut self.ptui.excl{
                                    Some((id,reason)) if *id==d.id=>{
                                        ui.add(egui::TextEdit::singleline(reason).hint_text("reden (verplicht)").desired_width(130.));
                                        if ui.add_enabled(!reason.trim().is_empty(),egui::Button::new("Uitsluiten")).clicked(){action=Some((d.id,true,reason.clone()));}
                                        if ui.small_button("Annuleren").clicked(){action=Some((-1,false,String::new()));}
                                    }
                                    _=>{if ui.small_button("Uitsluiten…").clicked(){self.ptui.excl=Some((d.id,String::new()));}}
                                }
                            }
                        });
                        ui.end_row();
                    }
                });
            });
            if let Some((id,ex,reason))=action{
                if id<0{self.ptui.excl=None;}
                else{self.click(&format!("Dosis {id} {}",if ex{"uitsluiten"}else{"weer meetellen"}));
                    match self.db.set_excluded(id,ex,&reason){Ok(())=>{self.ptui.excl=None;self.ptui.key=None;}Err(e)=>self.message=e}}
            }
            hint(ui,"Uitschieters worden nooit automatisch verwijderd. Uitsluiten vraagt een reden; de dosis blijft zichtbaar en het verslag noemt het aantal en de reden.");
        });
        ui.add_space(8.);
        let runs=self.ptui.runs.clone();
        card(ui,"Historie",|ui|{
            if runs.is_empty(){empty(ui,"Nog geen testen.",40.);return;}
            egui::ScrollArea::vertical().id_salt("pt-hist").max_height(180.).show(ui,|ui|{
                egui::Grid::new("pt-hist-g").striped(true).spacing([12.,3.]).show(ui,|ui|{
                    for t in ["","Test","Datum","Modus","Soort","Status","Tolerantie",""]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                    for r in &runs{
                        let mut c=self.ptui.chosen.contains(&r.id);if ui.checkbox(&mut c,"").changed(){if c{self.ptui.chosen.insert(r.id);}else{self.ptui.chosen.remove(&r.id);}}
                        ui.label(RichText::new(r.id.to_string()).strong().color(if Some(r.id)==self.ptui.shown{ACCENT}else{INK}));ui.label(applog::local_time(r.started_ms));
                        ui.label(RichText::new(r.mode_text()).color(if r.simulation{WARN}else{INK}));ui.label(r.mode.label());ui.label(&r.status);ui.label(format!("±{} %",pumptest::nl(r.tolerance_pct,1)));
                        if ui.small_button("Toon").clicked(){self.ptui.shown=Some(r.id);}
                        ui.end_row();
                    }
                });
            });
            ui.horizontal_wrapped(|ui|{
                let chosen:Vec<i64>=if self.ptui.chosen.is_empty(){self.ptui.shown.into_iter().collect()}else{self.ptui.chosen.iter().copied().collect()};
                if ui.add_enabled(!chosen.is_empty(),egui::Button::new(format!("Word-verslag ({} test{})",chosen.len(),if chosen.len()==1{""}else{"en"}))).clicked(){self.test_report(&chosen);}
                if ui.add_enabled(self.ptui.shown.is_some(),egui::Button::new("CSV van getoonde test")).clicked(){self.test_csv();}
                if ui.button("Open map").clicked(){let _=std::process::Command::new("explorer.exe").arg(&self.dev.settings.output).spawn();}
            });
            hint(ui,"Vink testen van dezelfde pomp aan voor één verslag; simulatie en hardware nooit samen.");
        });
    }
    fn test_report(&mut self,ids:&[i64]){
        self.click(&format!("Word-verslag testen {ids:?}"));
        let r=(||{
            let mut runs=vec![];for id in ids{runs.push((self.db.test_run(*id)?,self.db.test_doses(*id)?));}
            let cal=runs[0].0.calibration_id.and_then(|c|self.db.calibration(c).ok()).map(|c|format!("nr {} van {} ({})",c.id,applog::local_time(c.date_ms),if c.simulation{"simulatie"}else{"hardware"}));
            let bytes=pump_studio::report::test_report(&runs,cal.as_deref())?;
            pump_studio::report::save_test_report(&self.dev.settings.output,&runs[0].0.pump,ids,&bytes)
        })();
        match r{Ok(p)=>{applog::info("POMPTEST",&format!("Verslag opgeslagen: {}",p.display()));self.message=format!("Verslag opgeslagen: {}",p.display());let _=std::process::Command::new("explorer.exe").arg(&p).spawn();}
            Err(e)=>self.message=format!("Verslag niet gemaakt: {e}")}
    }
    fn test_csv(&mut self){
        let Some(id)=self.ptui.shown else{return};self.click(&format!("CSV test {id}"));
        let r=(||{let run=self.db.test_run(id)?;let d=self.db.test_doses(id)?;
            let title=format!("Pomptest {} · test {} · {} · {} · {}",run.pump,run.id,applog::local_time(run.started_ms),run.mode_text(),run.status);
            std::fs::create_dir_all(&self.dev.settings.output).map_err(|e|e.to_string())?;
            let p=self.dev.settings.output.join(format!("pomptest_{}_{}.csv",run.pump.replace(['/','\\',':',' '],"_"),run.id));
            std::fs::write(&p,pumptest::csv(&title,&d,run.tolerance_pct)).map_err(|e|e.to_string())?;Ok::<_,String>(p)})();
        match r{Ok(p)=>{applog::info("POMPTEST",&format!("CSV opgeslagen: {}",p.display()));self.message=format!("CSV opgeslagen: {}",p.display());}Err(e)=>self.message=format!("CSV niet opgeslagen: {e}")}
    }
}
fn key_has(runs:&[RunRow],id:i64)->bool{runs.iter().any(|r|r.id==id)}

#[cfg(test)] mod tests{
    use super::*;
    #[test]fn weights_list_parses_dutch(){
        assert_eq!(parse_weights("0,5; 1; 2 5\n10").unwrap(),vec![0.5,1.,2.,5.,10.]);
        assert!(parse_weights("").is_err());assert!(parse_weights("1; x").is_err());assert!(parse_weights("1; -2").is_err());
    }
}
