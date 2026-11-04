//! Ingrediënten: list with linked pumps/heads, edit (name, density, tolerance, head preference, preferred pump,
//! notes), add, delete (refused with the recipes that use it), warnings and CSV import/export (Dutch notation).
//! The link ingredient ↔ pump itself is set per pump on Pompen (one truth: the pump's ingredient field).
use super::*;
use pump_studio::{devices::parse_decimal,machine::Head,pumptest,recipe::{self,IngredientRow}};

#[derive(Default)]
pub struct IngUi{
    rows:Vec<IngredientRow>,warnings:Vec<String>,key:Option<(ProfileKind,u64,(u64,u64),bool)>,
    pub filter:String,pub new_name:String,pub csv_path:String,
    /// Edit form: (id, name, density, tolerance, notes, head preference).
    edit:Option<(i64,String,String,String,String,Option<Head>)>,
    confirm_delete:Option<i64>,
}
fn opt_dec(t:&str)->Result<Option<f64>,String>{if t.trim().is_empty(){Ok(None)}else{parse_decimal(t).map(Some)}}
fn head_text(h:Option<Head>)->&'static str{h.map(|h|h.name()).unwrap_or("geen")}

impl App{
    /// Hardware calibration needed when CAN is on as hardware (mode rule); else any active calibration.
    pub fn hardware_mode(&self)->bool{let s=self.dev.status(DevId::Can);s.link==Link::On && !s.simulation}
    pub fn is_calibrated(&self,pump:&str,hardware:bool)->bool{self.cal_dates.get(pump).is_some_and(|(_,sim)|!hardware || !*sim)}
    fn ing_refresh(&mut self){
        let hw=self.hardware_mode();
        let key=(self.profile(),self.data_ver,self.cal_seen,hw);
        if self.ingui.key.as_ref()==Some(&key){return;}
        self.ingui.key=Some(key);
        match self.db.ingredient_rows(self.profile()){
            Ok(r)=>self.ingui.rows=r,Err(e)=>{self.message=e;self.ingui.rows.clear();}
        }
        let cal=|p:&str|self.is_calibrated(p,hw);
        self.ingui.warnings=recipe::ingredient_warnings(&self.ingui.rows,&self.pumps,&cal,if hw{"hardware"}else{"simulatie"});
        if self.ingui.csv_path.is_empty(){self.ingui.csv_path=self.dev.settings.output.join(format!("ingredienten_{}.csv",self.profile().name())).display().to_string();}
    }
    /// After an ingredient change: reload pumps and names (Pompen shows the same link).
    fn ing_changed(&mut self){self.data_ver+=1;if let Err(e)=self.load_profile(){self.message=e;}}
    pub fn ingredients_page(&mut self,ui:&mut egui::Ui){
        self.ing_refresh();
        page_title(ui,"Ingrediënten","Ingrediënt ↔ pomp/kop. De koppeling zelf staat per pomp op Pompen; hier naam, dichtheid, tolerantie, voorkeurskop en voorkeurspomp.");
        card(ui,"Nieuw en CSV",|ui|{
            ui.horizontal_wrapped(|ui|{
                ui.label("Naam");ui.add(egui::TextEdit::singleline(&mut self.ingui.new_name).desired_width(160.));
                if ui.add_enabled(!self.ingui.new_name.trim().is_empty(),egui::Button::new("Toevoegen")).clicked(){
                    let n=self.ingui.new_name.trim().to_string();self.click(&format!("Ingrediënt toevoegen {n}"));
                    match self.db.add_ingredient(self.profile(),&n){Ok(_)=>{applog::info("INGREDIENT",&format!("{n} toegevoegd"));self.ingui.new_name.clear();self.ing_changed();}Err(e)=>self.message=e}
                }
            });
            ui.horizontal_wrapped(|ui|{
                ui.label("CSV-bestand");ui.add(egui::TextEdit::singleline(&mut self.ingui.csv_path).desired_width(360.));
                if ui.button("Exporteren").clicked(){self.ing_export();}
                if ui.button("Importeren").clicked(){self.ing_import();}
            });
            hint(ui,"CSV met ; en decimale komma: Naam;Dichtheid;Tolerantie;Voorkeurkop;Opmerking;Pompen (Pompen alleen ter info). Importeren voegt toe of werkt bij op naam; een foute regel weigert het hele bestand met het regelnummer.");
        });
        ui.add_space(8.);
        if !self.ingui.warnings.is_empty(){
            card(ui,"Waarschuwingen",|ui|{for w in self.ingui.warnings.iter().take(30){ui.label(RichText::new(w).color(WARN));}
                if self.ingui.warnings.len()>30{hint(ui,&format!("… en {} meer",self.ingui.warnings.len()-30));}});
            ui.add_space(8.);
        }
        let rows=self.ingui.rows.clone();
        card(ui,&format!("Ingrediënten ({})",rows.len()),|ui|{
            ui.horizontal(|ui|{ui.label("Zoek");ui.add(egui::TextEdit::singleline(&mut self.ingui.filter).desired_width(160.));});
            if rows.is_empty(){empty(ui,"Nog geen ingrediënten. Voeg er een toe of vul een ingrediënt in bij een pomp op Pompen.",50.);return;}
            let f=self.ingui.filter.trim().to_lowercase();
            let mut act:Option<Box<dyn FnOnce(&mut App)>>=None;
            egui::ScrollArea::both().id_salt("ing-list").max_height(420.).show(ui,|ui|{
                egui::Grid::new("ing-g").striped(true).spacing([12.,4.]).show(ui,|ui|{
                    for t in ["Naam","Pompen (kop)","Voorkeur","Dichtheid","Tolerantie","Voorkeurskop","Opmerkingen",""]{ui.label(RichText::new(t).size(12.).strong().color(MUTED));}ui.end_row();
                    for r in rows.iter().filter(|r|f.is_empty() || r.name.to_lowercase().contains(&f)){
                        ui.label(RichText::new(&r.name).strong());
                        ui.label(if r.pumps.is_empty(){RichText::new("geen").color(WARN)}else{RichText::new(r.pumps.iter().map(|p|format!("{} ({}){}",p.pump,head_text(p.head),if p.enabled{""}else{" uit"})).collect::<Vec<_>>().join(", "))});
                        // Preferred pump among the linked ones.
                        let cur=r.pumps.iter().find(|p|p.preferred).map(|p|p.pump.clone());
                        ui.add_enabled_ui(r.pumps.len()>1,|ui|{egui::ComboBox::from_id_salt(("ing-pref",r.id)).width(100.).selected_text(cur.clone().unwrap_or("—".into())).show_ui(ui,|ui|{
                            if ui.selectable_label(cur.is_none(),"—").clicked(){let id=r.id;act=Some(Box::new(move|a:&mut App|a.ing_pref(id,None)));}
                            for p in &r.pumps{if ui.selectable_label(cur.as_deref()==Some(p.pump.as_str()),&p.pump).clicked(){let (id,pid)=(r.id,p.pump_id);act=Some(Box::new(move|a:&mut App|a.ing_pref(id,Some(pid))));}}
                        });});
                        ui.label(r.density.map(|d|pumptest::nl(d,3)).unwrap_or("—".into()));
                        ui.label(r.tolerance_pct.map(|t|format!("±{} %",pumptest::nl(t,2))).unwrap_or_else(||"recept".into()));
                        ui.label(head_text(r.head_pref));ui.label(RichText::new(&r.notes).size(12.));
                        ui.horizontal(|ui|{
                            if ui.small_button("Bewerken").clicked(){self.ingui.edit=Some((r.id,r.name.clone(),r.density.map(|d|pumptest::nl(d,3)).unwrap_or_default(),r.tolerance_pct.map(|d|pumptest::nl(d,2)).unwrap_or_default(),r.notes.clone(),r.head_pref));}
                            if self.ingui.confirm_delete==Some(r.id){
                                if ui.add(egui::Button::new(RichText::new("Echt verwijderen").color(Color32::WHITE)).fill(DANGER)).clicked(){let id=r.id;act=Some(Box::new(move|a:&mut App|a.ing_delete(id)));}
                                if ui.small_button("Annuleren").clicked(){self.ingui.confirm_delete=None;}
                            }else if ui.small_button("Verwijderen").clicked(){self.ingui.confirm_delete=Some(r.id);}
                        });
                        ui.end_row();
                    }
                });
            });
            if let Some(a)=act{a(self);}
            hint(ui,"Verwijderen kan alleen als geen recept het ingrediënt gebruikt; de pompen worden dan ontkoppeld.");
        });
        if self.ingui.edit.is_some(){ui.add_space(8.);self.ing_form(ui);}
    }
    fn ing_form(&mut self,ui:&mut egui::Ui){
        let mut save=false;let mut cancel=false;
        card(ui,"Ingrediënt bewerken",|ui|{
            let Some((_,name,dens,tol,notes,head))=self.ingui.edit.as_mut() else{return};
            egui::Grid::new("ing-form").num_columns(2).spacing([12.,5.]).show(ui,|ui|{
                ui.label("Naam");ui.add(egui::TextEdit::singleline(name).desired_width(200.));ui.end_row();
                ui.label("Dichtheid (g/ml)");ui.add(egui::TextEdit::singleline(dens).desired_width(80.).hint_text("leeg = onbekend"));ui.end_row();
                ui.label("Tolerantie ±%");ui.add(egui::TextEdit::singleline(tol).desired_width(80.).hint_text("leeg = recept"));ui.end_row();
                ui.label("Voorkeurskop");ui.horizontal(|ui|{ui.selectable_value(head,None,"geen");for h in Head::ALL{ui.selectable_value(head,Some(h),h.name());}});ui.end_row();
                ui.label("Opmerkingen");ui.add(egui::TextEdit::multiline(notes).desired_rows(2).desired_width(300.));ui.end_row();
            });
            ui.horizontal(|ui|{save=primary(ui,true,"Opslaan");cancel=ui.button("Annuleren").clicked();});
            hint(ui,"Een nieuwe naam geldt meteen voor de gekoppelde pompen en alle recepten (ze verwijzen naar het ingrediënt, niet naar de naam).");
        });
        if cancel{self.ingui.edit=None;}
        if save{
            let Some((id,name,dens,tol,notes,head))=self.ingui.edit.clone() else{return};
            let Some(mut row)=self.ingui.rows.iter().find(|r|r.id==id).cloned() else{self.ingui.edit=None;return};
            let r=(||{row.name=name.trim().to_string();row.density=opt_dec(&dens).map_err(|e|format!("Dichtheid: {e}"))?;row.tolerance_pct=opt_dec(&tol).map_err(|e|format!("Tolerantie: {e}"))?;
                row.notes=notes;row.head_pref=head;self.db.save_ingredient(&row)})();
            self.click(&format!("Ingrediënt {id} opslaan"));
            match r{Ok(())=>{applog::info("INGREDIENT",&format!("{} opgeslagen",name.trim()));self.ingui.edit=None;self.ing_changed();}Err(e)=>self.message=e}
        }
    }
    fn ing_pref(&mut self,id:i64,pump:Option<i64>){self.click(&format!("Voorkeurspomp ingrediënt {id}"));match self.db.set_preferred_pump(id,pump){Ok(())=>self.ing_changed(),Err(e)=>self.message=e}}
    fn ing_delete(&mut self,id:i64){
        self.ingui.confirm_delete=None;self.click(&format!("Ingrediënt {id} verwijderen"));
        match self.db.delete_ingredient(id){Ok(())=>{applog::info("INGREDIENT",&format!("Ingrediënt {id} verwijderd"));self.ing_changed();}Err(e)=>self.message=e}
    }
    fn ing_export(&mut self){
        self.click("Ingrediënten exporteren");let p=PathBuf::from(self.ingui.csv_path.trim());
        let r=(||{let b=self.db.ingredients_csv(self.profile())?;if let Some(d)=p.parent(){std::fs::create_dir_all(d).map_err(|e|e.to_string())?;}std::fs::write(&p,b).map_err(|e|e.to_string())})();
        match r{Ok(())=>{applog::info("INGREDIENT",&format!("CSV opgeslagen: {}",p.display()));self.message=format!("CSV opgeslagen: {}",p.display());}Err(e)=>self.message=format!("CSV niet opgeslagen: {e}")}
    }
    fn ing_import(&mut self){
        self.click("Ingrediënten importeren");let p=PathBuf::from(self.ingui.csv_path.trim());
        let r=std::fs::read(&p).map_err(|e|format!("{}: {e}",p.display())).and_then(|b|self.db.import_ingredients_csv(self.profile(),&b));
        match r{Ok(m)=>{applog::info("INGREDIENT",&m);self.message=m;self.ing_changed();}Err(e)=>self.message=format!("Import geweigerd: {e}")}
    }
}
