//! Recepten: list, editor (steps up/down/copy/remove, items with grams, tolerance and head; top-up per step),
//! validation, save with versioning, version history; run on one pot (plan per head, Start pot with a confirm on a
//! real machine, live progress, "Volgende kop", results per item) and the pot history with the recipe version.
use super::*;
use pump_studio::{devices::parse_decimal,machine::Head,pumptest,recipe::{self,ItemDraft,PotItemRow,PotPlan,PotRunRow,RecipeDraft,RecipeRow,StepDraft}};

#[derive(Clone,Default,PartialEq)]
pub struct EdItem{pub ingredient:String,pub grams:String,pub tol:String,pub head:Option<Head>}
#[derive(Clone,Default,PartialEq)]
pub struct EdStep{pub topup:bool,pub items:Vec<EdItem>}
#[derive(Clone,Default,PartialEq)]
pub struct Editor{pub recipe:Option<i64>,pub name:String,pub description:String,pub pot_g:String,pub tol:String,pub steps:Vec<EdStep>,pub height:String}
/// Dutch number without trailing zeros: 0,25 / 2 / 1,5.
fn nlt(v:f64,d:usize)->String{let t=pumptest::nl(v,d);if t.contains(','){t.trim_end_matches('0').trim_end_matches(',').to_string()}else{t}}
impl Editor{
    pub fn from_draft(recipe:Option<i64>,d:&RecipeDraft)->Self{
        Self{recipe,name:d.name.clone(),description:d.description.clone(),pot_g:d.pot_g.map(|g|nlt(g,3)).unwrap_or_default(),tol:nlt(d.tolerance_pct,2),height:d.pot_height_cm.map(|h|nlt(h,1)).unwrap_or_default(),
            steps:d.steps.iter().map(|s|EdStep{topup:s.topup,items:s.items.iter().map(|i|EdItem{ingredient:i.ingredient.clone(),grams:nlt(i.grams,4),tol:i.tolerance_pct.map(|t|nlt(t,2)).unwrap_or_default(),head:i.head}).collect()}).collect()}
    }
    /// Draft from the form; number errors name the step and item.
    pub fn draft(&self)->Result<RecipeDraft,Vec<String>>{
        let mut err=vec![];
        let dec=|t:&str,what:String,err:&mut Vec<String>|->Option<f64>{if t.trim().is_empty(){return None;}match parse_decimal(t){Ok(v)=>Some(v),Err(e)=>{err.push(format!("{what}: {e}"));None}}};
        let tol=dec(&self.tol,"Tolerantie".into(),&mut err).unwrap_or(recipe::DEFAULT_TOL);
        let pot_g=dec(&self.pot_g,"Potgewicht".into(),&mut err);
        let pot_height_cm=dec(&self.height,"Pothoogte".into(),&mut err);
        let steps=self.steps.iter().enumerate().map(|(s,st)|StepDraft{topup:st.topup,items:st.items.iter().map(|i|ItemDraft{ingredient:i.ingredient.trim().into(),
            grams:if i.grams.trim().is_empty(){err.push(format!("Stap {} {}: vul een gewicht in",s+1,i.ingredient));0.}else{dec(&i.grams,format!("Stap {} {}",s+1,i.ingredient),&mut err).unwrap_or(0.)},
            tolerance_pct:dec(&i.tol,format!("Stap {} {} tolerantie",s+1,i.ingredient),&mut err),head:i.head}).collect()}).collect();
        if self.name.trim().is_empty(){err.push("Geef het recept een naam".into());}
        if err.is_empty(){Ok(RecipeDraft{name:self.name.trim().into(),description:self.description.clone(),pot_g,tolerance_pct:tol,steps,pot_height_cm})}else{Err(err)}
    }
}

#[derive(Default)]
pub struct RecUi{
    list:Vec<RecipeRow>,key:Option<(ProfileKind,u64)>,pub show_archived:bool,
    pub ed:Option<Editor>,saved:Option<Editor>,versions:Vec<(i64,i64,i64,usize)>,
    /// Version to run and its plan (refreshed when devices, calibrations or data change).
    pub run_version:Option<i64>,plan:Option<Result<PotPlan,Vec<String>>>,plan_key:Option<(i64,u64,(u64,u64),bool)>,
    pub confirm_real:bool,pub settle_s:f64,
    runs:Vec<PotRunRow>,items:Vec<PotItemRow>,pub shown:Option<i64>,runs_key:Option<(Option<i64>,u64,Option<i64>)>,
}

impl App{
    fn rec_refresh(&mut self){
        let key=(self.profile(),self.data_ver);
        if self.recui.key.as_ref()!=Some(&key){
            self.recui.key=Some(key);
            self.recui.list=self.db.recipes(self.profile()).unwrap_or_else(|e|{self.message=e;vec![]});
            if let Some(r)=self.recui.ed.as_ref().and_then(|e|e.recipe){self.recui.versions=self.db.recipe_versions(r).unwrap_or_default();}
        }
        let rid=self.recui.ed.as_ref().and_then(|e|e.recipe);
        // The version that runs: the current version of the open recipe (saved state).
        self.recui.run_version=rid.and_then(|r|self.recui.list.iter().find(|x|x.id==r)).map(|x|x.version_id);
        let hw=self.hardware_mode();
        if let Some(v)=self.recui.run_version{
            let pk=(v,self.data_ver,self.cal_seen,hw);
            if self.recui.plan_key!=Some(pk){self.recui.plan_key=Some(pk);self.recui.plan=Some(self.db.resolve_plan(ProfileKind::Vsd,v,hw));}
        }else{self.recui.plan=None;self.recui.plan_key=None;}
        // Pot history of the open recipe; follows a running pot.
        let pot=self.dev.pot.lock().unwrap().clone();
        if pot.run.is_some(){self.recui.shown=pot.run;}
        let rk=(rid,pot.version,self.recui.shown);
        if self.recui.runs_key!=Some(rk){
            self.recui.runs_key=Some(rk);
            self.recui.runs=rid.and_then(|r|self.db.pot_runs(r).ok()).unwrap_or_default();
            if self.recui.shown.is_none_or(|id|!self.recui.runs.iter().any(|r|r.id==id)){self.recui.shown=self.recui.runs.first().map(|r|r.id);}
            self.recui.items=self.recui.shown.and_then(|id|self.db.pot_items(id).ok()).unwrap_or_default();
        }
    }
    pub fn recipes_page(&mut self,ui:&mut egui::Ui){
        page_title(ui,"Recepten","Geordende stappen: één ingrediënt per stap doseert na elkaar, meerdere ingrediënten in een stap doseren tegelijk. Elke stap wordt gewogen; de kop volgt uit de pomp van het ingrediënt.");
        if self.profile()!=ProfileKind::Vsd{card(ui,"",|ui|{empty(ui,"Recepten draaien op het VSD-profiel (LH1/LH2). Kies rechtsboven het profiel VSD.",60.);});return;}
        self.rec_refresh();
        let wide=ui.available_width()>=1000.;
        if wide{ui.horizontal_top(|ui|{
            ui.vertical(|ui|{ui.set_width(300.);self.rec_list(ui);});
            ui.vertical(|ui|self.rec_main(ui));
        });}else{self.rec_list(ui);ui.add_space(8.);self.rec_main(ui);}
    }
    fn rec_list(&mut self,ui:&mut egui::Ui){
        let list=self.recui.list.clone();let open=self.recui.ed.as_ref().and_then(|e|e.recipe);
        card(ui,"Recepten",|ui|{
            ui.horizontal(|ui|{if ui.button("Nieuw recept").clicked(){self.click("Nieuw recept");self.rec_open_new();}ui.checkbox(&mut self.recui.show_archived,"archief");});
            if list.is_empty(){empty(ui,"Nog geen recepten.",40.);return;}
            let arch=self.recui.show_archived;
            for r in list.iter().filter(|r|arch || !r.archived){
                let sel=open==Some(r.id);
                let t=format!("{}  v{}\n{} stappen · {} g{}{}",r.name,r.version,r.steps,pumptest::nl(r.total_g,3),if r.used>0{format!(" · {} pot(ten)",r.used)}else{String::new()},if r.archived{" · archief"}else{""});
                if ui.selectable_label(sel,RichText::new(t).color(if r.archived{MUTED}else{INK})).clicked() && !sel{self.click(&format!("Recept {} openen",r.name));self.rec_open(r.id,r.version_id);}
            }
        });
    }
    fn rec_open_new(&mut self){let e=Editor{tol:pumptest::nl(recipe::DEFAULT_TOL,2),steps:vec![EdStep::default()],..Default::default()};self.recui.saved=None;self.recui.ed=Some(e);self.recui.versions.clear();self.recui.shown=None;self.recui.runs_key=None;}
    /// Open a recipe version in the editor (also used by the smoke test).
    pub fn open_recipe(&mut self,rid:i64,vid:i64){self.data_ver+=1;self.rec_open(rid,vid);}
    fn rec_open(&mut self,rid:i64,vid:i64){
        match self.db.recipe_version(vid){
            Ok((_,_,d))=>{let e=Editor::from_draft(Some(rid),&d);self.recui.saved=Some(e.clone());self.recui.ed=Some(e);self.recui.versions=self.db.recipe_versions(rid).unwrap_or_default();self.recui.shown=None;self.recui.runs_key=None;}
            Err(e)=>self.message=e,
        }
    }
    fn rec_main(&mut self,ui:&mut egui::Ui){
        if self.recui.ed.is_none(){card(ui,"",|ui|empty(ui,"Kies links een recept of maak een nieuw recept.",60.));return;}
        self.rec_editor(ui);ui.add_space(8.);
        if self.recui.ed.as_ref().is_some_and(|e|e.recipe.is_some()){self.rec_run(ui);ui.add_space(8.);self.rec_results(ui);}
    }
    fn rec_editor(&mut self,ui:&mut egui::Ui){
        let names=self.ingredients.clone();let running=self.dev.pot_running();
        let dirty=self.recui.ed!=self.recui.saved;
        let mut save=false;let mut revert=false;let mut archive=None;
        let rid=self.recui.ed.as_ref().and_then(|e|e.recipe);
        let row=rid.and_then(|r|self.recui.list.iter().find(|x|x.id==r)).cloned();
        let title=match &row{Some(r)=>format!("{} · versie {}{}",r.name,r.version,if dirty{" · gewijzigd"}else{""}),None=>"Nieuw recept".into()};
        let pumps=self.pumps.clone();
        card(ui,&title,|ui|{
            let ed=self.recui.ed.as_mut().unwrap();
            egui::Grid::new("rec-head").num_columns(4).spacing([10.,5.]).show(ui,|ui|{
                ui.label("Naam");ui.add(egui::TextEdit::singleline(&mut ed.name).desired_width(180.));
                ui.label("Tolerantie ±%");ui.add(egui::TextEdit::singleline(&mut ed.tol).desired_width(60.));ui.end_row();
                ui.label("Omschrijving");ui.add(egui::TextEdit::singleline(&mut ed.description).desired_width(180.));
                ui.label("Potgewicht (g)");ui.add(egui::TextEdit::singleline(&mut ed.pot_g).desired_width(60.).hint_text("optioneel"));ui.end_row();
                ui.label("");ui.label("");
                ui.label("Pothoogte (cm)");ui.add(egui::TextEdit::singleline(&mut ed.height).desired_width(60.).hint_text("Instellingen")).on_hover_text("Hoogte voor de lift (PLC P1 = hoogte × 10). Leeg = lifthoogte per kop uit Instellingen (VSD).");ui.end_row();
            });
            ui.separator();
            let n=ed.steps.len();let mut op:Option<(usize,u8)>=None;
            for (s,st) in ed.steps.iter_mut().enumerate(){
                Frame::new().stroke(Stroke::new(W1,BORDER)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::same(8)).show(ui,|ui|{
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui|{
                        ui.label(RichText::new(format!("Stap {}",s+1)).strong());
                        ui.label(RichText::new(if st.items.len()>1{"parallel"}else{"sequentieel"}).size(12.).color(MUTED));
                        ui.checkbox(&mut st.topup,"bijdoseren").on_hover_text("Eén keer bijdoseren als een ingrediënt onder de ondergrens blijft (te veel wordt gemeld, nooit gecorrigeerd)");
                        if ui.add_enabled(s>0,egui::Button::new("↑").small()).on_hover_text("Stap omhoog").clicked(){op=Some((s,0));}
                        if ui.add_enabled(s+1<n,egui::Button::new("↓").small()).on_hover_text("Stap omlaag").clicked(){op=Some((s,1));}
                        if ui.small_button("Kopiëren").clicked(){op=Some((s,2));}
                        if ui.small_button("Verwijderen").clicked(){op=Some((s,3));}
                        if ui.small_button("+ ingrediënt").clicked(){st.items.push(EdItem::default());}
                    });
                    let mut del=None;
                    egui::Grid::new(("rec-step",s)).spacing([8.,3.]).show(ui,|ui|{
                        if !st.items.is_empty(){for t in ["Ingrediënt","Gram","Tol. ±%","Kop",""]{ui.label(RichText::new(t).size(11.).color(MUTED));}ui.end_row();}
                        for (k,it) in st.items.iter_mut().enumerate(){
                            egui::ComboBox::from_id_salt(("rec-ing",s,k)).width(140.).height(320.).selected_text(if it.ingredient.is_empty(){"Kies"}else{&it.ingredient}).show_ui(ui,|ui|{
                                for nm in &names{if ui.selectable_label(*nm==it.ingredient,nm).clicked(){it.ingredient=nm.clone();}}});
                            ui.add(egui::TextEdit::singleline(&mut it.grams).desired_width(70.));
                            ui.add(egui::TextEdit::singleline(&mut it.tol).desired_width(50.).hint_text("recept"));
                            // Head: from the pump (shown), optionally forced.
                            let auto=pumps.iter().filter(|p|p.enabled && p.ingredient==it.ingredient).filter_map(|p|p.head).next();
                            egui::ComboBox::from_id_salt(("rec-head",s,k)).width(90.).selected_text(match it.head{Some(h)=>h.name().to_string(),None=>format!("auto ({})",auto.map(|h|h.name()).unwrap_or("?"))}).show_ui(ui,|ui|{
                                ui.selectable_value(&mut it.head,None,"auto");for h in Head::ALL{ui.selectable_value(&mut it.head,Some(h),h.name());}});
                            if ui.small_button("✕").on_hover_text("Ingrediënt uit de stap halen").clicked(){del=Some(k);}
                            ui.end_row();
                        }
                    });
                    if let Some(k)=del{st.items.remove(k);}
                });
                ui.add_space(4.);
            }
            if let Some((s,o))=op{match o{0=>ed.steps.swap(s,s-1),1=>ed.steps.swap(s,s+1),2=>{let c=ed.steps[s].clone();ed.steps.insert(s+1,c);}_=>{ed.steps.remove(s);}}}
            if ui.button("+ stap").clicked(){ed.steps.push(EdStep{items:vec![EdItem::default()],..Default::default()});}
            // Validation as you type (structure + numbers); pump/calibration checks are in "Uitvoeren".
            let draft=ed.draft();
            let errs=match &draft{Ok(d)=>{let known=|n:&str|names.iter().any(|x|x==n);d.structural_errors(&known)}Err(e)=>e.clone()};
            if let Ok(d)=&draft{hint(ui,&format!("{} stappen, totaal {} g",d.steps.len(),pumptest::nl(d.total_g(),3)));}
            for e in &errs{ui.label(RichText::new(e).color(DANGER));}
            ui.horizontal_wrapped(|ui|{
                save=primary(ui,dirty && errs.is_empty() && !running,"Opslaan");
                if ui.add_enabled(dirty && rid.is_some(),egui::Button::new("Wijzigingen terugdraaien")).clicked(){revert=true;}
                if let Some(r)=&row{if ui.button(if r.archived{"Uit archief halen"}else{"Archiveren"}).clicked(){archive=Some(!r.archived);}}
            });
            if row.as_ref().is_some_and(|r|r.used>0){hint(ui,"Deze versie is al gebruikt: opslaan maakt een nieuwe versie; eerdere potten blijven naar hun versie wijzen.");}
            let versions=&self.recui.versions;
            if versions.len()>1 || versions.first().is_some_and(|v|v.3>0){
                section(ui,"VERSIES");
                let mut load=None;
                egui::Grid::new("rec-ver").striped(true).spacing([12.,2.]).show(ui,|ui|{
                    for (vid,v,ms,used) in versions{ui.label(format!("v{v}"));ui.label(applog::local_time(*ms));ui.label(format!("{used} pot(ten)"));
                        if ui.small_button("Laden in editor").on_hover_text("Inhoud van deze versie laden; opslaan maakt er de nieuwste versie van").clicked(){load=Some(*vid);}ui.end_row();}
                });
                if let Some(vid)=load{match self.db.recipe_version(vid){Ok((_,_,d))=>{let e=Editor::from_draft(rid,&d);self.recui.ed=Some(e);}Err(e)=>self.message=e}}
            }
        });
        if revert{self.recui.ed=self.recui.saved.clone();}
        if let (Some(a),Some(r))=(archive,rid){self.click(&format!("Recept {r} {}",if a{"archiveren"}else{"uit archief"}));match self.db.set_recipe_archived(r,a){Ok(())=>self.data_ver+=1,Err(e)=>self.message=e}}
        if save{self.rec_save();}
    }
    fn rec_save(&mut self){
        let Some(ed)=self.recui.ed.clone() else{return};
        let d=match ed.draft(){Ok(d)=>d,Err(e)=>{self.message=e.join("; ");return}};
        self.click(&format!("Recept {} opslaan",d.name));
        match self.db.save_recipe(self.profile(),ed.recipe,&d){
            Ok(o)=>{applog::info("RECEPT",&format!("{} opgeslagen als versie {}{}",d.name,o.version,if o.new_version{" (nieuwe versie)"}else{""}));
                self.message=format!("{} opgeslagen (versie {})",d.name,o.version);self.data_ver+=1;
                let e=Editor::from_draft(Some(o.recipe_id),&d);self.recui.saved=Some(e.clone());self.recui.ed=Some(e);self.recui.versions=self.db.recipe_versions(o.recipe_id).unwrap_or_default();self.recui.runs_key=None;}
            Err(e)=>self.message=e,
        }
    }
    fn rec_run(&mut self,ui:&mut egui::Ui){
        let pot=self.dev.pot.lock().unwrap().clone();let dirty=self.recui.ed!=self.recui.saved;
        let hw=self.hardware_mode();
        card(ui,"Uitvoeren op één pot",|ui|{
            match &self.recui.plan{
                None=>hint(ui,"Sla het recept eerst op."),
                Some(Err(errs))=>{ui.label(RichText::new("Niet uitvoerbaar:").strong().color(DANGER));for e in errs{ui.label(RichText::new(e).color(DANGER));}}
                Some(Ok(p))=>{
                    ui.label(RichText::new(format!("{} v{} · {}",p.recipe,p.version,if hw{"HARDWARE"}else{"simulatie"})).strong().color(if hw{DANGER}else{WARN}));
                    for h in p.heads(){
                        section(ui,&format!("KOP {}",h.name()));
                        for s in p.part(h){ui.label(format!("Stap {}{}: {}",s.idx+1,if s.topup{" (bijdoseren)"}else{""},
                            s.items.iter().map(|i|format!("{} {} g op {}",i.ingredient,pumptest::nl(i.grams,3),i.pump)).collect::<Vec<_>>().join(" ∥ ")));}
                    }
                    if p.heads().len()>1{hint(ui,"Handmatig, één pot: eerst kop LH1, dan zet u de pot onder LH2 en klikt u Volgende kop.");}
                    let lift=p.heads().iter().map(|h|match p.pot_height_cm.or(self.dev.settings.machine.lift_height_cm[h.idx()]).filter(|v|*v>0.){
                        Some(v)=>format!("{} {} cm (P1 {}){}",h.name(),pumptest::nl(v,1),pump_studio::machine::height_p1(v),if p.pot_height_cm.is_some(){" recept"}else{""}),
                        None=>format!("{} niet ingesteld{}",h.name(),if hw{""}else{" (simulatie: 10 cm)"})}).collect::<Vec<_>>().join(" · ");
                    ui.label(RichText::new(format!("Lifthoogte: {lift}")).color(if hw && lift.contains("niet ingesteld"){DANGER}else{INK}));
                }
            }
            if dirty{ui.label(RichText::new("Niet opgeslagen wijzigingen: uitgevoerd wordt de opgeslagen versie.").color(WARN));}
            if pot.run.is_none(){
                ui.horizontal(|ui|{ui.label("Wachttijd stabiel");ui.add(egui::DragValue::new(&mut self.recui.settle_s).speed(0.1).range(0. ..=60.).suffix(" s"));});
                if hw{ui.checkbox(&mut self.recui.confirm_real,RichText::new("Echte machine: lift en unit mogen bewegen").color(DANGER));}
                let ok=matches!(self.recui.plan,Some(Ok(_))) && (!hw || self.recui.confirm_real);
                if primary(ui,ok,"Start pot"){
                    if let Some(v)=self.recui.run_version{self.click(&format!("Start pot versie {v}"));
                        match self.dev.start_pot(&mut self.db,v,self.recui.confirm_real,self.recui.settle_s){Ok(id)=>{self.recui.shown=Some(id);self.recui.runs_key=None;self.recui.confirm_real=false;}Err(e)=>self.message=e}}
                }
                hint(ui,"Nodig: CAN, PLC en de weegschaal van elke kop in het recept verbonden (Monitors). De lift gaat pas omhoog na Start pot en gaat omlaag aan het eind, bij STOP en bij een fout.");
                if let Some((id,st,note))=&pot.last{hint(ui,&format!("Laatste pot {id}: {st}{}",if note.is_empty(){String::new()}else{format!(" — {note}")}));}
            }else{
                ui.label(RichText::new(format!("Pot {} loopt: {}",pot.run.unwrap(),pot.recipe)).strong().color(ACCENT));
                if let Some(w)=&pot.waiting{ui.label(RichText::new(w).strong().color(WARN));if primary(ui,true,"Volgende kop"){self.click("Volgende kop");self.dev.pot_resume();}}
                hint(ui,"STOP / Esc stopt de pot binnen 1 s; wat gedoseerd is blijft bewaard en de lift gaat omlaag.");
            }
            if !pot.lines.is_empty(){
                egui::ScrollArea::vertical().id_salt("pot-lines").max_height(140.).stick_to_bottom(true).show(ui,|ui|{for l in pot.lines.iter().rev().take(40).rev(){ui.label(RichText::new(l).size(12.).monospace());}});
            }
        });
    }
    fn rec_results(&mut self,ui:&mut egui::Ui){
        let runs=self.recui.runs.clone();let items=self.recui.items.clone();
        let run=self.recui.shown.and_then(|id|runs.iter().find(|r|r.id==id)).cloned();
        card(ui,&match &run{Some(r)=>format!("Pot {} · {} v{} · {} · {}",r.id,r.recipe,r.version,applog::local_time(r.started_ms),r.status),None=>"Resultaat pot".into()},|ui|{
            let Some(r)=run else{empty(ui,"Nog geen pot gedraaid met dit recept.",40.);return};
            if r.simulation{ui.label(RichText::new("SIMULATIE").strong().color(WARN));}
            if !r.note.is_empty(){ui.label(RichText::new(&r.note).color(if r.status=="klaar"{INK}else{DANGER}));}
            if items.is_empty(){hint(ui,"Nog geen ingrediënten gewogen.");return;}
            egui::ScrollArea::horizontal().id_salt("pot-items-h").show(ui,|ui|egui::Grid::new("pot-items").striped(true).spacing([10.,3.]).show(ui,|ui|{
                for t in ["Stap","Ingrediënt","Pomp","Kop","Gevraagd g","Gedoseerd g","Afw. %","Tol.","Oordeel","Bijgedoseerd","Toegerekend"]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                for i in &items{
                    ui.label((i.step_idx+1).to_string());ui.label(&i.ingredient);ui.label(&i.pump);ui.label(&i.head);
                    ui.label(RichText::new(pumptest::nl(i.requested_g,3)).monospace());ui.label(RichText::new(i.dosed_g.map(|g|pumptest::nl(g,4)).unwrap_or("—".into())).monospace());
                    ui.label(i.deviation_pct.map(|d|pumptest::nls(d,2)).unwrap_or("—".into()));ui.label(format!("±{}",pumptest::nl(i.tolerance_pct,2)));
                    let c=match i.status.as_str(){"ok"=>OK,"te_weinig"|"te_veel"=>WARN,_=>DANGER};
                    ui.label(RichText::new(recipe::status_label(&i.status)).color(c)).on_hover_text(&i.note);
                    ui.label(if i.topped_up{"ja"}else{""});
                    ui.label(if i.attributed{RichText::new("ja").color(WARN)}else{RichText::new("")}).on_hover_text("Parallel gedoseerd: aandeel in het nettogewicht van de kop naar verhouding van de gevraagde grammen");
                    ui.end_row();
                }
            }));
            hint(ui,"Gemeten waarden worden nooit aangepast. Te veel wordt gemeld, nooit gecorrigeerd. 'Toegerekend' = parallel gedoseerd, gewicht naar verhouding verdeeld.");
        });
        ui.add_space(8.);
        card(ui,"Pottenhistorie",|ui|{
            if runs.is_empty(){empty(ui,"Nog geen potten.",30.);return;}
            egui::ScrollArea::vertical().id_salt("pot-hist").max_height(160.).show(ui,|ui|egui::Grid::new("pot-hist-g").striped(true).spacing([12.,3.]).show(ui,|ui|{
                for t in ["Pot","Datum","Versie","Soort","Status",""]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                for r in &runs{
                    ui.label(RichText::new(r.id.to_string()).strong().color(if Some(r.id)==self.recui.shown{ACCENT}else{INK}));ui.label(applog::local_time(r.started_ms));
                    ui.label(format!("v{}",r.version));ui.label(RichText::new(if r.simulation{"simulatie"}else{"hardware"}).color(if r.simulation{WARN}else{INK}));ui.label(&r.status);
                    if ui.small_button("Toon").clicked(){self.recui.shown=Some(r.id);}ui.end_row();
                }
            }));
        });
    }
}

#[cfg(test)] mod tests{
    use super::*;
    #[test]fn editor_roundtrip_and_number_errors(){
        let d=RecipeDraft{name:"X".into(),tolerance_pct:1.5,steps:vec![StepDraft{topup:true,items:vec![ItemDraft{ingredient:"A".into(),grams:0.25,tolerance_pct:Some(2.),head:Some(Head::LH2)}]}],..Default::default()};
        let e=Editor::from_draft(Some(3),&d);assert_eq!((e.steps[0].items[0].grams.as_str(),e.tol.as_str(),nlt(2.,3).as_str()),("0,25","1,5","2"));assert_eq!(e.draft().unwrap(),d);
        let mut bad=e.clone();bad.steps[0].items[0].grams="1,2,3".into();bad.name.clear();
        let err=bad.draft().unwrap_err();assert!(err.iter().any(|x|x.contains("Stap 1 A")) && err.iter().any(|x|x.contains("naam")),"{err:?}");
    }
}
