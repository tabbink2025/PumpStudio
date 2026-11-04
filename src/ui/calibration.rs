//! Kalibratie: VSD (Camelot method on the scale of the pump's own head, queue, progress, STOP, control
//! dose) and ceramic (forward-only calibration and 360° curve). Results, plots and history come from the
//! database; the newest successful calibration is active, older ones are archived.
use super::*;
use egui_plot::{Corner,Legend,Line,MarkerShape,Plot,Points};
use pump_studio::{calib::{self,CalPoint,CalRow},devices::{group,DevId},machine::Head,model::{Curve,Interval},vsdcal::{CalSettings,VsdCalibration},workflow::Action};

const RUNS:[Color32;4]=[Color32::from_rgb(96,145,200),Color32::from_rgb(230,140,70),Color32::from_rgb(90,170,120),Color32::from_rgb(200,95,110)];

/// State of the Kalibratie page.
pub struct CalUi{
    pub pump:String,pub set:CalSettings,pub strokes:String,pub check_g:f64,
    pub k_revs:u32,pub k_repeats:usize,pub k_prime:u32,pub k_deg:f64,pub k_by_steps:bool,pub k_step:i64,pub k_curve_repeats:usize,
    hist:Vec<CalRow>,key:Option<(ProfileKind,String,u64,u64)>,pub selected:Option<i64>,view:Option<(i64,View)>,
}
enum View{Vsd(Box<VsdCalibration>,Vec<CalPoint>),Bench(Option<Curve>,Vec<CalPoint>)}
impl Default for CalUi{
    fn default()->Self{Self{pump:String::new(),set:CalSettings::default(),strokes:"1, 2, 3, 5, 7, 10, 13, 16".into(),check_g:1.,
        k_revs:1,k_repeats:3,k_prime:0,k_deg:10.,k_by_steps:false,k_step:1000,k_curve_repeats:5,hist:vec![],key:None,selected:None,view:None}}
}
fn pct(a:Option<f64>,b:Option<f64>)->Option<f64>{match(a,b){(Some(a),Some(b)) if b.abs()>1e-12=>Some(100.*(a-b)/b),_=>None}}
fn num(v:Option<f64>,d:usize)->String{v.map(|x|format!("{x:.d$}").replace('.',",")).unwrap_or("—".into())}
fn status_color(s:&str)->Color32{match s{"actief"=>OK,"afgebroken"=>DANGER,_=>MUTED}}

impl App{
    /// History of the shown pump, reloaded when a calibration was stored or the pump changed.
    fn cal_refresh(&mut self){
        // Default pump: chosen on Pompen, else the first with an active calibration, else the first one.
        if self.calui.pump.is_empty() || !self.pumps.iter().any(|p|p.name==self.calui.pump){
            self.calui.pump=self.pumps.iter().find(|p|self.sel.contains(&p.id)).or(self.pumps.iter().find(|p|self.cal_dates.contains_key(&p.name))).or(self.pumps.first()).map(|p|p.name.clone()).unwrap_or_default();}
        let key=(self.profile(),self.calui.pump.clone(),self.dev.status(DevId::Can).cal_version,self.dev.bench_cal_version());
        if self.calui.key.as_ref()==Some(&key){return;}
        self.calui.key=Some(key);
        match self.db.calibrations(self.profile(),&self.calui.pump){
            Ok(h)=>{self.calui.selected=h.iter().find(|r|r.status==calib::ACTIVE).or(h.first()).map(|r|r.id);self.calui.hist=h;}
            Err(e)=>{self.message=format!("Kalibraties laden: {e}");self.calui.hist.clear();}
        }
        self.calui.view=None;
    }
    fn cal_view(&mut self){
        let Some(id)=self.calui.selected else{self.calui.view=None;return};
        if self.calui.view.as_ref().is_some_and(|v|v.0==id){return;}
        let pts=self.db.calibration_points(id).unwrap_or_default();
        let kind=self.calui.hist.iter().find(|r|r.id==id).map(|r|r.kind.clone()).unwrap_or_default();
        let v=if kind==calib::KIND_VSD{self.db.vsd_calibration(id).ok().map(|c|View::Vsd(Box::new(c),pts))}else{Some(View::Bench(self.db.bench_curve(id).ok().flatten(),pts))};
        self.calui.view=v.map(|v|(id,v));
    }
    pub fn calibration_page(&mut self,ui:&mut egui::Ui){
        self.cal_refresh();self.cal_view();
        let vsd=self.profile()==ProfileKind::Vsd;
        page_title(ui,"Kalibratie",if vsd{"Camelot-methode: fase 1 (deel van een slag, cosinusverdeeld) en fase 2 (hele omwentelingen), gewogen op de weegschaal van de eigen kop. Resultaat en historie in de database."}
            else{"Keramische pomp: forward-only kalibratie (g/omw, optioneel prime) en 360°-meetcurve. Alleen vooruit; tellerbeveiliging blijft actief."});
        let wide=ui.available_width()>=1000.;
        let left=|a:&mut Self,ui:&mut egui::Ui|{if vsd{a.vsd_cal_controls(ui)}else{a.bench_cal_controls(ui)}};
        if wide{ui.horizontal_top(|ui|{
            ui.vertical(|ui|{ui.set_width(360.);left(self,ui);});
            ui.vertical(|ui|{self.cal_results(ui);});
        });}else{left(self,ui);ui.add_space(8.);self.cal_results(ui);}
    }
    fn pump_picker(&mut self,ui:&mut egui::Ui){
        let names:Vec<String>=self.pumps.iter().map(|p|p.name.clone()).collect();
        ui.horizontal(|ui|{ui.label("Pomp");
            egui::ComboBox::from_id_salt("cal-pump").width(120.).selected_text(&self.calui.pump).height(320.).show_ui(ui,|ui|{
                for n in &names{if ui.selectable_label(*n==self.calui.pump,n).clicked() && *n!=self.calui.pump{self.calui.pump=n.clone();}}});
            if let Some(p)=self.pumps.iter().find(|p|p.name==self.calui.pump){
                hint(ui,&format!("{} · node {} motor {}{}",p.head.map(|h|h.name()).unwrap_or("—"),p.node,p.motor,if p.enabled{""}else{" · staat uit"}));}
        });
    }
    fn vsd_cal_controls(&mut self,ui:&mut egui::Ui){
        let st=self.dev.status(DevId::Can);let ready=st.link==Link::On && !st.busy;
        card(ui,"Kalibreren",|ui|{
            self.pump_picker(ui);
            let head=self.pumps.iter().find(|p|p.name==self.calui.pump).and_then(|p|p.head);
            if let Some(h)=head{let s=self.dev.status(if h==Head::LH1{DevId::Lh1}else{DevId::Lh2});
                ui.horizontal(|ui|{dot(ui,link_color(s.link,s.simulation));ui.label(format!("Weegschaal {}: {}",h.name(),link_text(s.link,s.simulation)));});}
            egui::CollapsingHeader::new("Instellingen (standaard Camelot)").default_open(false).show(ui,|ui|{
                let s=&mut self.calui.set;
                egui::Grid::new("cal-set").num_columns(2).spacing([12.,5.]).show(ui,|ui|{
                    ui.label("Fase 1 meetpunten");ui.add(egui::DragValue::new(&mut s.phase1_samples).range(3..=200));ui.end_row();
                    ui.label("Fase 1 tot");ui.add(egui::DragValue::new(&mut s.phase1_max_deg).range(30. ..=360.).suffix(" °"));ui.end_row();
                    ui.label("Fase 1 runs");ui.add(egui::DragValue::new(&mut s.phase1_runs).range(1..=10));ui.end_row();
                    ui.label("Fase 2");ui.checkbox(&mut s.phase2,"hele omwentelingen");ui.end_row();
                    ui.label("Omwentelingen");ui.add(egui::TextEdit::singleline(&mut self.calui.strokes).desired_width(150.));ui.end_row();
                    ui.label("Fase 2 runs");ui.add(egui::DragValue::new(&mut s.phase2_runs).range(1..=10));ui.end_row();
                    ui.label("Wachten voor weging");ui.add(egui::DragValue::new(&mut s.settle_s).speed(0.1).range(0. ..=60.).suffix(" s"));ui.end_row();
                });
                if ui.small_button("Camelot-standaard").clicked(){*s=CalSettings::default();self.calui.strokes="1, 2, 3, 5, 7, 10, 13, 16".into();}
            });
            let strokes:Result<Vec<u32>,String>=self.calui.strokes.split([',',';',' ']).filter(|t|!t.trim().is_empty()).map(|t|t.trim().parse::<u32>().map_err(|_|format!("Ongeldig aantal omwentelingen: {t}"))).collect();
            let mut set=self.calui.set.clone();
            let valid=strokes.and_then(|v|{set.stroke_samples=v;set.validate().map(|_|set.clone())});
            if let Err(e)=&valid{ui.label(RichText::new(e).color(DANGER));}
            let queue:Vec<String>=self.pumps.iter().filter(|p|self.sel.contains(&p.id)).map(|p|p.name.clone()).collect();
            ui.horizontal_wrapped(|ui|{
                if primary(ui,ready && valid.is_ok(),&format!("{} kalibreren",self.calui.pump)){if let Ok(s)=valid.clone(){self.start_cal(vec![self.calui.pump.clone()],s);}}
                if ui.add_enabled(ready && valid.is_ok() && !queue.is_empty(),egui::Button::new(format!("Wachtrij: {} gekozen pomp(en)",queue.len())))
                    .on_hover_text(if queue.is_empty(){"Kies pompen op de pagina Pompen".to_string()}else{queue.join(", ")}).clicked(){if let Ok(s)=valid.clone(){self.start_cal(queue.clone(),s);}}
            });
            if let Some(p)=&st.progress{
                ui.add(egui::ProgressBar::new(p.step as f32/p.steps.max(1) as f32).text(format!("pomp {}/{} {} · stap {}/{}",p.k,p.n,p.pump,p.step,p.steps)).fill(ACCENT));
                hint(ui,&p.text);
            }
            if st.link!=Link::On{hint(ui,"Eerst CAN en de weegschaal van de kop verbinden (Instellingen > Verbindingen).");}
            result_line(ui,if st.text.starts_with("Kalibratie") || st.text.starts_with("Controle"){&st.text}else{""},st.ok);
            hint(ui,"STOP / Esc onderbreekt binnen 1 s; de gemeten punten worden opgeslagen als afgebroken, de actieve kalibratie blijft.");
        });
        ui.add_space(8.);
        card(ui,"Controle doseren",|ui|{
            let active=self.calui.hist.iter().find(|r|r.status==calib::ACTIVE).cloned();
            ui.horizontal(|ui|{
                ui.label("Gewicht");ui.add(egui::DragValue::new(&mut self.calui.check_g).speed(0.01).range(0.001..=1000.).max_decimals(4).suffix(" g"));
                if ui.add_enabled(ready && active.is_some(),egui::Button::new("Controle doseren")).clicked(){
                    self.click(&format!("Controle doseren {} {} g",self.calui.pump,self.calui.check_g));let r=self.dev.check_dose(&self.calui.pump.clone(),self.calui.check_g);self.result(r);
                }
            });
            match &active{
                None=>hint(ui,"Geen actieve kalibratie voor deze pomp."),
                Some(a)=>hint(ui,&match (a.check_requested,a.check_measured){(Some(r),Some(m))=>format!("Laatste controle: gevraagd {} g, gedoseerd {} g, afwijking {} %",num(Some(r),4),num(Some(m),4),num(a.check_error_pct,2)),_=>"Nog geen controle bij de actieve kalibratie.".into()}),
            }
            hint(ui,"Doelpositie vanaf de actuele nokfase met de actieve kalibratie; tarra ervoor, stabiele weging erna op de eigen kop. Op hardware alleen een hardware-kalibratie; in simulatie de simulatie-kalibratie, anders de hardware-kalibratie.");
        });
    }
    fn start_cal(&mut self,names:Vec<String>,s:CalSettings){
        self.click(&format!("Kalibratie starten: {}",names.join(", ")));
        let r=self.dev.calibrate(names,s);self.result(r);
    }
    fn bench_cal_controls(&mut self,ui:&mut egui::Ui){
        let b=self.dev.bench.clone();let st=self.dev.status(DevId::Bench);let ready=b.connected && !st.busy && b.reference_valid;
        card(ui,"Pompbank",|ui|{
            ui.horizontal(|ui|{dot(ui,link_color(st.link,st.simulation));ui.label(format!("{} · teller {} st",link_text(st.link,st.simulation),group(b.position)));});
            ui.label(RichText::new(if b.reference_valid{"Referentie actief"}else{"Referentie ongeldig: eerst startpositie 0 of home (Pompen)"}).color(if b.reference_valid{OK}else{DANGER}));
            if !b.scale_ok && b.connected{ui.label(RichText::new("Geen weegschaal verbonden: kalibreren kan niet").color(WARN));}
        });
        ui.add_space(8.);
        card(ui,"Kalibratie (g/omw)",|ui|{
            egui::Grid::new("k-cal").num_columns(2).spacing([12.,5.]).show(ui,|ui|{
                ui.label("Omwentelingen per meting");ui.add(egui::DragValue::new(&mut self.calui.k_revs).range(1..=390));ui.end_row();
                ui.label("Herhalingen");ui.add(egui::DragValue::new(&mut self.calui.k_repeats).range(1..=100));ui.end_row();
                ui.label("Prime omwentelingen");ui.add(egui::DragValue::new(&mut self.calui.k_prime).range(0..=100)).on_hover_text("0 = overslaan");ui.end_row();
            });
            if primary(ui,ready && b.scale_ok,"Kalibratie starten"){
                let a=Action::Calibrate{revolutions:self.calui.k_revs,repeats:self.calui.k_repeats,prime:self.calui.k_prime};
                self.click(&format!("Keramische kalibratie {} omw × {}, prime {}",self.calui.k_revs,self.calui.k_repeats,self.calui.k_prime));let r=self.dev.bench_action(a);self.result(r);}
        });
        ui.add_space(8.);
        card(ui,"360°-meetcurve",|ui|{
            ui.horizontal(|ui|{ui.label("Meetpunt per");ui.selectable_value(&mut self.calui.k_by_steps,false,"hoek");ui.selectable_value(&mut self.calui.k_by_steps,true,"stappen");});
            let n=if b.connected{b.config.steps_rev}else{self.dev.settings.bench.steps_rev};
            egui::Grid::new("k-curve").num_columns(2).spacing([12.,5.]).show(ui,|ui|{
                if self.calui.k_by_steps{ui.label("Positiestap");ui.add(egui::DragValue::new(&mut self.calui.k_step).speed(10.).range(1..=n.max(1)).suffix(" st"));}
                else{ui.label("Hoekinterval");ui.add(egui::DragValue::new(&mut self.calui.k_deg).speed(0.5).range(0.1..=360.).suffix(" °"));}
                ui.end_row();
                ui.label("Herhalingen");ui.add(egui::DragValue::new(&mut self.calui.k_curve_repeats).range(1..=100));ui.end_row();
            });
            let interval=if self.calui.k_by_steps{Interval::Steps(self.calui.k_step)}else{Interval::Degrees(self.calui.k_deg)};
            let grid=interval.targets(n);
            match &grid{Ok(t)=>hint(ui,&format!("{} punten per omwenteling ({} st/omw)",t.len(),group(n))),Err(e)=>{ui.label(RichText::new(e).color(DANGER));}}
            if primary(ui,ready && b.scale_ok && grid.is_ok(),"Meetcurve starten"){
                self.click(&format!("Meetcurve {interval:?} × {}",self.calui.k_curve_repeats));let r=self.dev.bench_action(Action::Characterize{interval,repeats:self.calui.k_curve_repeats});self.result(r);}
        });
        ui.add_space(6.);
        if st.busy{ui.horizontal(|ui|{ui.spinner();ui.label(&b.status);});}else{result_line(ui,&b.status,!b.status.starts_with("Afgebroken"));}
        hint(ui,"Resultaat wordt direct opgeslagen; gestopt of mislukt = afgebroken (nooit actief). STOP / Esc onderbreekt.");
    }
    fn cal_results(&mut self,ui:&mut egui::Ui){
        let sel=self.calui.selected.and_then(|id|self.calui.hist.iter().find(|r|r.id==id).cloned());
        card(ui,&format!("Resultaat {}",self.calui.pump),|ui|{
            let Some(r)=&sel else{empty(ui,"Nog geen kalibratie voor deze pomp.",80.);return;};
            // The previous successful calibration of the same kind, for comparison.
            let prev=self.calui.hist.iter().filter(|h|h.kind==r.kind && h.simulation==r.simulation && h.id!=r.id && h.status!=calib::ABORTED && h.date_ms<=r.date_ms).max_by_key(|h|h.date_ms).cloned();
            egui::Grid::new("cal-kpi").num_columns(4).spacing([18.,4.]).show(ui,|ui|{
                ui.label(RichText::new("Status").color(MUTED));ui.label(RichText::new(&r.status).strong().color(status_color(&r.status)));
                ui.label(RichText::new("Datum").color(MUTED));ui.label(format!("{} · {}",applog::local_time(r.date_ms),if r.simulation{"simulatie"}else{"hardware"}));ui.end_row();
                ui.label(RichText::new("Soort").color(MUTED));ui.label(calib::kind_label(&r.kind));
                ui.label(RichText::new("g per slag").color(MUTED));ui.label(num(r.g_per_stroke,4));ui.end_row();
                ui.label(RichText::new("g per omwenteling").color(MUTED));ui.label(num(r.g_per_rev,4));
                ui.label(RichText::new("Spreiding tussen runs").color(MUTED));ui.label(format!("{} g",num(r.spread,5)));ui.end_row();
                ui.label(RichText::new("Gerepareerde punten").color(MUTED));ui.label(r.repaired.to_string());
                ui.label(RichText::new("Controle").color(MUTED));ui.label(match r.check_measured{Some(m)=>format!("{} → {} g ({} %)",num(r.check_requested,3),num(Some(m),4),num(r.check_error_pct,2)),None=>"—".into()});ui.end_row();
                if let Some(p)=&prev{
                    ui.label(RichText::new("t.o.v. vorige").color(MUTED));
                    ui.label(format!("g/slag {} % · g/omw {} % ({})",num(pct(r.g_per_stroke,p.g_per_stroke),2),num(pct(r.g_per_rev,p.g_per_rev),2),applog::local_time(p.date_ms)));ui.end_row();
                }
            });
            if !r.note.is_empty(){hint(ui,&r.note);}
            if !r.source.is_empty(){hint(ui,&format!("Bron: {}",r.source));}
        });
        ui.add_space(8.);
        match &self.calui.view{
            Some((_,View::Vsd(c,pts)))=>{
                let spr=c.steps_per_rev();
                ui.columns(2,|cols|{
                    card(&mut cols[0],"Fase 1: gewicht tegen positie",|ui|{
                        Plot::new("cal-p1").height(220.).x_axis_label("positie (stappen)").y_axis_label("g").legend(Legend::default().position(Corner::LeftTop)).allow_scroll(false).show(ui,|pl|{
                            let runs=pts.iter().filter(|p|p.phase==1).map(|p|p.run).max().unwrap_or(0);
                            for k in 1..=runs{pl.points(Points::new(format!("run {k}"),pts.iter().filter(|p|p.phase==1 && p.run==k).map(|p|[p.position_steps as f64,p.grams]).collect::<Vec<_>>()).radius(2.5_f32).color(RUNS[(k as usize-1)%4]));}
                            pl.line(Line::new("gemiddeld",pts.iter().filter(|p|p.phase==1 && p.run==0).map(|p|[p.position_steps as f64,p.grams]).collect::<Vec<_>>()).color(ACCENT).width(2.0_f32));
                            let rep:Vec<[f64;2]>=pts.iter().filter(|p|p.phase==1 && p.run==0 && p.repaired).map(|p|[p.position_steps as f64,p.grams]).collect();
                            if !rep.is_empty(){pl.points(Points::new("gerepareerd",rep).shape(MarkerShape::Diamond).radius(5.0_f32).color(WARN));}
                        });
                        hint(ui,&format!("{} st per omwenteling; max = {} g per slag",group(spr),num(Some(c.g_per_stroke),4)));
                    });
                    card(&mut cols[1],"Fase 2: gewicht tegen omwentelingen",|ui|{
                        Plot::new("cal-p2").height(220.).x_axis_label("omwentelingen").y_axis_label("g").legend(Legend::default().position(Corner::LeftTop)).allow_scroll(false).show(ui,|pl|{
                            let runs=pts.iter().filter(|p|p.phase==2).map(|p|p.run).max().unwrap_or(0);
                            for k in 1..=runs{pl.points(Points::new(format!("run {k}"),pts.iter().filter(|p|p.phase==2 && p.run==k).map(|p|[p.position_steps as f64/spr as f64,p.grams]).collect::<Vec<_>>()).radius(3.0_f32).color(RUNS[(k as usize-1)%4]));}
                            pl.line(Line::new("gemiddeld",pts.iter().filter(|p|p.phase==2 && p.run==0).map(|p|[p.position_steps as f64/spr as f64,p.grams]).collect::<Vec<_>>()).color(ACCENT).width(2.0_f32));
                        });
                    });
                });
            }
            Some((_,View::Bench(curve,pts)))=>{
                card(ui,if curve.is_some(){"Meetcurve: cumulatieve massa"}else{"Kalibratie: g per omwenteling per herhaling"},|ui|{
                    let n=self.dev.settings.bench.steps_rev.max(1) as f64;
                    Plot::new("cal-bench").height(240.).legend(Legend::default().position(Corner::LeftTop)).allow_scroll(false)
                        .x_axis_label(if curve.is_some(){"hoek °"}else{"herhaling"}).y_axis_label(if curve.is_some(){"g"}else{"g/omw"}).show(ui,|pl|{
                        if let Some(c)=curve{
                            let ang:Vec<f64>=c.steps.iter().map(|s|*s as f64*360./c.steps_rev as f64).collect();
                            for (k,run) in c.runs.iter().enumerate(){pl.line(Line::new(format!("herhaling {}",k+1),ang.iter().zip(run).map(|(a,m)|[*a,*m]).collect::<Vec<_>>()).color(RUNS[k%4]).width(1.0_f32));}
                            pl.line(Line::new("doseercurve",ang.iter().zip(&c.masses).map(|(a,m)|[*a,*m]).collect::<Vec<_>>()).color(ACCENT).width(3.0_f32));
                        }else{
                            pl.points(Points::new("g/omw",pts.iter().filter(|p|p.position_steps>0).map(|p|[p.run as f64,p.grams*n/p.position_steps as f64]).collect::<Vec<_>>()).radius(4.0_f32).color(ACCENT));
                        }
                    });
                });
            }
            None=>{}
        }
        ui.add_space(8.);
        let hist=self.calui.hist.clone();
        card(ui,"Historie",|ui|{
            if hist.is_empty(){empty(ui,"Nog geen kalibraties.",50.);return;}
            egui::ScrollArea::vertical().id_salt("cal-hist").max_height(220.).show(ui,|ui|{
                egui::Grid::new("cal-hist-g").striped(true).spacing([14.,4.]).show(ui,|ui|{
                    for t in ["Datum","Modus","Soort","Status","g/slag","g/omw","Spreiding","Δ vorige %","Controle %",""]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                    for (k,r) in hist.iter().enumerate(){
                        let prev=hist[k+1..].iter().find(|h|h.kind==r.kind && h.simulation==r.simulation && h.status!=calib::ABORTED);
                        let mine=Some(r.id)==self.calui.selected;
                        ui.label(RichText::new(applog::local_time(r.date_ms)).strong().color(if mine{ACCENT}else{INK}));
                        ui.label(RichText::new(if r.simulation{"simulatie"}else{"hardware"}).color(if r.simulation{WARN}else{INK}));
                        ui.label(calib::kind_label(&r.kind));ui.label(RichText::new(&r.status).color(status_color(&r.status)));
                        ui.label(num(r.g_per_stroke,4));ui.label(num(r.g_per_rev,4));ui.label(num(r.spread,5));
                        ui.label(num(prev.and_then(|p|pct(r.g_per_rev,p.g_per_rev)),2));ui.label(num(r.check_error_pct,2));
                        if ui.small_button("Toon").clicked(){self.calui.selected=Some(r.id);}
                        ui.end_row();
                    }
                });
            });
        });
    }
}
