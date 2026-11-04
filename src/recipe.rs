//! Ingredients ↔ pumps, recipes with versions (ordered steps; a step with one item doses sequentially, with
//! several items in parallel), validation and pump choice for a run, weight attribution, and pot-run storage.
//! Receptmodel (PLAN.md, binding): parallel items on one head share the head's net weight in proportion to the
//! requested grams (flagged as such); too little and too much are both reported; measured values are never altered.
use crate::{calib,db::{Db,ProfileKind},machine::Head,model::Result,pumptest::nl};
use rusqlite::{params,OptionalExtension};
use serde::{Deserialize,Serialize};

fn e(x:rusqlite::Error)->String{format!("Database: {x}")}
fn now_ms()->i64{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis() as i64).unwrap_or(0)}
fn head_from(s:Option<String>)->Option<Head>{match s.as_deref(){Some("LH1")=>Some(Head::LH1),Some("LH2")=>Some(Head::LH2),_=>None}}
pub const DEFAULT_TOL:f64=1.;

// ---------- ingredients ----------
#[derive(Clone,Debug,PartialEq)]
pub struct PumpLink{pub pump_id:i64,pub pump:String,pub head:Option<Head>,pub enabled:bool,pub preferred:bool}
#[derive(Clone,Debug,PartialEq,Default)]
pub struct IngredientRow{pub id:i64,pub profile_id:i64,pub name:String,pub density:Option<f64>,pub tolerance_pct:Option<f64>,pub notes:String,pub head_pref:Option<Head>,pub pumps:Vec<PumpLink>}

impl Db{
    pub fn ingredient_rows(&self,profile:ProfileKind)->Result<Vec<IngredientRow>>{
        let pid=self.profile(profile)?.id;
        let mut st=self.conn.prepare("SELECT id,profile_id,name,density,tolerance_pct,coalesce(notes,''),head_pref FROM ingredient WHERE profile_id=?1 ORDER BY name").map_err(e)?;
        let mut rows:Vec<IngredientRow>=st.query_map([pid],|r|Ok(IngredientRow{id:r.get(0)?,profile_id:r.get(1)?,name:r.get(2)?,density:r.get(3)?,tolerance_pct:r.get(4)?,notes:r.get(5)?,head_pref:head_from(r.get(6)?),pumps:vec![]}))
            .map_err(e)?.collect::<std::result::Result<_,_>>().map_err(e)?;
        let mut st=self.conn.prepare("SELECT ip.ingredient_id,p.id,p.name,p.head,p.enabled,ip.preferred FROM ingredient_pump ip JOIN pump p ON p.id=ip.pump_id WHERE p.profile_id=?1 ORDER BY p.sort").map_err(e)?;
        let links=st.query_map([pid],|r|Ok((r.get::<_,i64>(0)?,PumpLink{pump_id:r.get(1)?,pump:r.get(2)?,head:head_from(r.get(3)?),enabled:r.get::<_,i64>(4)?!=0,preferred:r.get::<_,i64>(5)?!=0}))).map_err(e)?;
        for l in links{let (iid,link)=l.map_err(e)?;if let Some(row)=rows.iter_mut().find(|r|r.id==iid){row.pumps.push(link);}}
        Ok(rows)
    }
    pub fn ingredient_by_name(&self,profile:ProfileKind,name:&str)->Result<Option<i64>>{
        let pid=self.profile(profile)?.id;
        self.conn.query_row("SELECT id FROM ingredient WHERE profile_id=?1 AND name=?2",params![pid,name.trim()],|r|r.get(0)).optional().map_err(e)
    }
    pub fn add_ingredient(&self,profile:ProfileKind,name:&str)->Result<i64>{
        let name=name.trim();if name.is_empty(){return Err("Naam invullen".into());}
        if self.ingredient_by_name(profile,name)?.is_some(){return Err(format!("Ingrediënt {name} bestaat al"));}
        let pid=self.profile(profile)?.id;
        self.conn.execute("INSERT INTO ingredient(profile_id,name) VALUES(?1,?2)",params![pid,name]).map_err(e)?;Ok(self.conn.last_insert_rowid())
    }
    /// Name (unique), density, tolerance, notes and head preference.
    pub fn save_ingredient(&self,row:&IngredientRow)->Result<()>{
        let name=row.name.trim();if name.is_empty(){return Err("Naam invullen".into());}
        if row.density.is_some_and(|d|!d.is_finite() || d<=0. || d>30.){return Err("Dichtheid 0 tot 30 g/ml".into());}
        if row.tolerance_pct.is_some_and(|t|!t.is_finite() || !(0.01..=50.).contains(&t)){return Err("Tolerantie 0,01 tot 50 %".into());}
        let clash:Option<i64>=self.conn.query_row("SELECT id FROM ingredient WHERE profile_id=?1 AND name=?2 AND id<>?3",params![row.profile_id,name,row.id],|r|r.get(0)).optional().map_err(e)?;
        if clash.is_some(){return Err(format!("Ingrediënt {name} bestaat al"));}
        self.conn.execute("UPDATE ingredient SET name=?2,density=?3,tolerance_pct=?4,notes=?5,head_pref=?6 WHERE id=?1",
            params![row.id,name,row.density,row.tolerance_pct,row.notes.trim(),row.head_pref.map(|h|h.name())]).map(|_|()).map_err(e)
    }
    /// The preferred pump of an ingredient (when it has several); `None` clears the preference.
    pub fn set_preferred_pump(&self,ingredient:i64,pump:Option<i64>)->Result<()>{
        self.conn.execute("UPDATE ingredient_pump SET preferred=CASE WHEN pump_id=?2 THEN 1 ELSE 0 END WHERE ingredient_id=?1",params![ingredient,pump]).map(|_|()).map_err(e)
    }
    /// Recipes (with version) that use this ingredient.
    pub fn ingredient_usage(&self,ingredient:i64)->Result<Vec<String>>{
        let mut st=self.conn.prepare("SELECT DISTINCT r.name,v.version FROM recipe_item i JOIN recipe_step s ON s.id=i.step_id JOIN recipe_version v ON v.id=s.version_id JOIN recipe r ON r.id=v.recipe_id WHERE i.ingredient_id=?1 ORDER BY r.name,v.version").map_err(e)?;
        let rows=st.query_map([ingredient],|r|Ok(format!("{} v{}",r.get::<_,String>(0)?,r.get::<_,i64>(1)?))).map_err(e)?;
        rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    /// Delete only when no recipe (any version) uses it; its pumps are unlinked (pump keeps working without ingredient).
    pub fn delete_ingredient(&mut self,ingredient:i64)->Result<()>{
        let used=self.ingredient_usage(ingredient)?;
        let name:String=self.conn.query_row("SELECT name FROM ingredient WHERE id=?1",[ingredient],|r|r.get(0)).map_err(e)?;
        if !used.is_empty(){return Err(format!("{name} wordt gebruikt in: {}. Eerst uit die recepten halen.",used.join(", ")));}
        let tx=self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
        tx.execute("UPDATE pump SET ingredient_id=NULL WHERE ingredient_id=?1",[ingredient]).map_err(e)?;
        tx.execute("UPDATE pot_item SET ingredient_id=NULL WHERE ingredient_id=?1",[ingredient]).map_err(e)?;
        tx.execute("DELETE FROM ingredient WHERE id=?1",[ingredient]).map_err(e)?;
        tx.commit().map_err(e)?;crate::applog::info("INGREDIENT",&format!("{name} verwijderd"));Ok(())
    }
    /// CSV for Excel (Dutch: semicolon, decimal comma, UTF-8 with BOM).
    pub fn ingredients_csv(&self,profile:ProfileKind)->Result<Vec<u8>>{
        let mut out=String::from("\u{feff}Naam;Dichtheid (g/ml);Tolerantie (%);Voorkeurkop;Opmerking;Pompen\n");
        for r in self.ingredient_rows(profile)?{
            out+=&format!("{};{};{};{};{};{}\n",csv_field(&r.name),r.density.map(|d|nl(d,4)).unwrap_or_default(),r.tolerance_pct.map(|t|nl(t,2)).unwrap_or_default(),
                r.head_pref.map(|h|h.name()).unwrap_or(""),csv_field(&r.notes),csv_field(&r.pumps.iter().map(|p|format!("{} ({})",p.pump,p.head.map(|h|h.name()).unwrap_or("—"))).collect::<Vec<_>>().join(", ")));
        }
        Ok(out.into_bytes())
    }
    /// Import: new names are added, existing ones get density/tolerance/head/notes from the file. Pump links stay
    /// as they are (they are set on Pompen). Returns a summary; refuses the whole file on the first bad line.
    pub fn import_ingredients_csv(&mut self,profile:ProfileKind,bytes:&[u8])->Result<String>{
        let text=String::from_utf8(bytes.to_vec()).map_err(|_|"Bestand is geen UTF-8".to_string())?;
        let text=text.trim_start_matches('\u{feff}');
        let mut lines=text.lines();
        let head=lines.next().ok_or("Leeg bestand")?;
        if !head.to_lowercase().starts_with("naam"){return Err("Eerste regel moet de kop zijn (Naam;Dichtheid;Tolerantie;Voorkeurkop;Opmerking)".into());}
        let mut rows=vec![];
        for (k,l) in lines.enumerate(){
            if l.trim().is_empty(){continue;}
            let f=split_csv(l);let get=|i:usize|f.get(i).map(|s|s.trim().to_string()).unwrap_or_default();
            let num=|s:String,what:&str|->Result<Option<f64>>{if s.is_empty(){Ok(None)}else{crate::devices::parse_decimal(&s).map(Some).map_err(|x|format!("Regel {}: {what}: {x}",k+2))}};
            let name=get(0);if name.is_empty(){return Err(format!("Regel {}: naam ontbreekt",k+2));}
            let head=match get(3).to_uppercase().as_str(){""=>None,"LH1"=>Some(Head::LH1),"LH2"=>Some(Head::LH2),x=>return Err(format!("Regel {}: voorkeurkop {x} (LH1, LH2 of leeg)",k+2))};
            rows.push((name,num(get(1),"dichtheid")?,num(get(2),"tolerantie")?,head,get(4)));
        }
        let (mut added,mut updated)=(0,0);
        for (name,density,tol,head,notes) in rows{
            let id=match self.ingredient_by_name(profile,&name)?{Some(id)=>{updated+=1;id},None=>{added+=1;self.add_ingredient(profile,&name)?}};
            let pid=self.profile(profile)?.id;
            self.save_ingredient(&IngredientRow{id,profile_id:pid,name,density,tolerance_pct:tol,notes,head_pref:head,pumps:vec![]})?;
        }
        Ok(format!("{added} toegevoegd, {updated} bijgewerkt"))
    }
}
fn csv_field(s:&str)->String{if s.contains([';','"','\n']){format!("\"{}\"",s.replace('"',"\"\""))}else{s.to_string()}}
/// Split one CSV line on ';' with "…" quoting.
pub fn split_csv(line:&str)->Vec<String>{
    let mut out=vec![];let mut cur=String::new();let mut q=false;let mut it=line.chars().peekable();
    while let Some(c)=it.next(){
        match c{'"' if q && it.peek()==Some(&'"')=>{cur.push('"');it.next();}'"'=>q=!q,';' if !q=>{out.push(std::mem::take(&mut cur));}_=>cur.push(c)}
    }
    out.push(cur);out
}

/// Warnings of the Ingrediënten page: ingredient without pump, pump without ingredient, pump off or not
/// calibrated in the mode of the connection (`calibrated(pump)`).
pub fn ingredient_warnings(rows:&[IngredientRow],pumps:&[crate::db::PumpRow],calibrated:&dyn Fn(&str)->bool,mode:&str)->Vec<String>{
    let mut w=vec![];
    for r in rows{
        if r.pumps.is_empty(){w.push(format!("{}: geen pomp gekoppeld",r.name));continue;}
        for p in &r.pumps{
            if !p.enabled{w.push(format!("{}: pomp {} staat uit",r.name,p.pump));}
            else if !calibrated(&p.pump){w.push(format!("{}: pomp {} niet gekalibreerd ({mode})",r.name,p.pump));}
        }
    }
    let unlinked=pumps.iter().filter(|p|p.ingredient.trim().is_empty()).count();
    if unlinked>0{w.push(format!("{unlinked} pomp(en) zonder ingrediënt: {}",pumps.iter().filter(|p|p.ingredient.trim().is_empty()).take(10).map(|p|p.name.as_str()).collect::<Vec<_>>().join(", ")));}
    w
}

// ---------- recipes ----------
#[derive(Clone,Debug,PartialEq,Serialize,Deserialize,Default)]
pub struct ItemDraft{pub ingredient:String,pub grams:f64,pub tolerance_pct:Option<f64>,pub head:Option<Head>}
#[derive(Clone,Debug,PartialEq,Serialize,Deserialize,Default)]
pub struct StepDraft{pub topup:bool,pub items:Vec<ItemDraft>}
#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]
pub struct RecipeDraft{pub name:String,pub description:String,pub pot_g:Option<f64>,pub tolerance_pct:f64,pub steps:Vec<StepDraft>,
    /// Pot height in cm for the lift (overrides the head's height from Instellingen); None = use that.
    #[serde(default)] pub pot_height_cm:Option<f64>}
impl Default for RecipeDraft{fn default()->Self{Self{name:String::new(),description:String::new(),pot_g:None,tolerance_pct:DEFAULT_TOL,steps:vec![],pot_height_cm:None}}}
impl RecipeDraft{
    pub fn total_g(&self)->f64{self.steps.iter().flat_map(|s|s.items.iter()).map(|i|i.grams).sum()}
    /// Structure of the Receptmodel, without pumps: refuses with a message per problem (R2).
    pub fn structural_errors(&self,known:&dyn Fn(&str)->bool)->Vec<String>{
        let mut w=vec![];
        if self.name.trim().is_empty(){w.push("Recept zonder naam".into());}
        if !self.tolerance_pct.is_finite() || !(0.01..=50.).contains(&self.tolerance_pct){w.push("Tolerantie recept 0,01 tot 50 %".into());}
        if self.pot_g.is_some_and(|g|!g.is_finite() || g<=0.){w.push("Potgewicht moet positief zijn".into());}
        if let Some(Err(e))=self.pot_height_cm.map(crate::machine::check_height){w.push(format!("Pothoogte: {e}"));}
        if self.steps.is_empty(){w.push("Leeg recept: voeg minimaal één stap toe".into());}
        for (k,s) in self.steps.iter().enumerate(){
            let n=k+1;
            if s.items.is_empty(){w.push(format!("Stap {n}: leeg"));}
            for (j,i) in s.items.iter().enumerate(){
                let what=if i.ingredient.trim().is_empty(){format!("stap {n} regel {}",j+1)}else{format!("stap {n} {}",i.ingredient.trim())};
                if i.ingredient.trim().is_empty(){w.push(format!("Stap {n}: regel {} zonder ingrediënt",j+1));}
                else if !known(i.ingredient.trim()){w.push(format!("Stap {n}: ingrediënt {} bestaat niet",i.ingredient.trim()));}
                if !i.grams.is_finite() || i.grams<=0.{w.push(format!("{}: gewicht moet groter dan 0 g zijn ({} g)",cap(&what),nl(i.grams,3)));}
                if i.tolerance_pct.is_some_and(|t|!t.is_finite() || !(0.01..=50.).contains(&t)){w.push(format!("{}: tolerantie 0,01 tot 50 %",cap(&what)));}
                if s.items[..j].iter().any(|o|o.ingredient.trim().eq_ignore_ascii_case(i.ingredient.trim())) && !i.ingredient.trim().is_empty(){
                    w.push(format!("Stap {n}: {} staat twee keer in deze parallelle stap",i.ingredient.trim()));}
            }
        }
        w
    }
}
fn cap(s:&str)->String{let mut c=s.chars();match c.next(){Some(f)=>f.to_uppercase().collect::<String>()+c.as_str(),None=>String::new()}}

#[derive(Clone,Debug,PartialEq)]
pub struct RecipeRow{pub id:i64,pub name:String,pub description:String,pub version_id:i64,pub version:i64,pub steps:usize,pub total_g:f64,pub used:usize,pub archived:bool}
#[derive(Clone,Debug,PartialEq)]
pub struct SaveOutcome{pub recipe_id:i64,pub version_id:i64,pub version:i64,pub new_version:bool}

impl Db{
    pub fn recipes(&self,profile:ProfileKind)->Result<Vec<RecipeRow>>{
        let pid=self.profile(profile)?.id;
        let mut st=self.conn.prepare("SELECT r.id,r.name,r.description,v.id,v.version,r.archived,
              (SELECT count(*) FROM recipe_step s WHERE s.version_id=v.id),
              (SELECT coalesce(sum(i.grams),0) FROM recipe_item i JOIN recipe_step s ON s.id=i.step_id WHERE s.version_id=v.id),
              (SELECT count(*) FROM pot_run p JOIN recipe_version v2 ON v2.id=p.recipe_version_id WHERE v2.recipe_id=r.id)
            FROM recipe r JOIN recipe_version v ON v.id=r.current_version WHERE r.profile_id=?1 ORDER BY r.archived,r.name").map_err(e)?;
        let rows=st.query_map([pid],|r|Ok(RecipeRow{id:r.get(0)?,name:r.get(1)?,description:r.get(2)?,version_id:r.get(3)?,version:r.get(4)?,archived:r.get::<_,i64>(5)?!=0,
            steps:r.get::<_,i64>(6)? as usize,total_g:r.get(7)?,used:r.get::<_,i64>(8)? as usize})).map_err(e)?;
        rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    /// One version: (recipe id, version number, contents).
    pub fn recipe_version(&self,version_id:i64)->Result<(i64,i64,RecipeDraft)>{
        let (rid,ver,name,desc,pot,tol,height):(i64,i64,String,String,Option<f64>,f64,Option<f64>)=self.conn.query_row(
            "SELECT r.id,v.version,r.name,v.description,v.pot_g,v.tolerance_pct,v.pot_height_cm FROM recipe_version v JOIN recipe r ON r.id=v.recipe_id WHERE v.id=?1",[version_id],
            |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).map_err(|x|format!("Receptversie {version_id}: {}",e(x)))?;
        let mut d=RecipeDraft{name,description:desc,pot_g:pot,tolerance_pct:tol,steps:vec![],pot_height_cm:height};
        let mut st=self.conn.prepare("SELECT id,topup FROM recipe_step WHERE version_id=?1 ORDER BY idx").map_err(e)?;
        let steps:Vec<(i64,bool)>=st.query_map([version_id],|r|Ok((r.get(0)?,r.get::<_,i64>(1)?!=0))).map_err(e)?.collect::<std::result::Result<_,_>>().map_err(e)?;
        for (sid,topup) in steps{
            let mut st=self.conn.prepare("SELECT g.name,i.grams,i.tolerance_pct,i.head FROM recipe_item i JOIN ingredient g ON g.id=i.ingredient_id WHERE i.step_id=?1 ORDER BY i.idx").map_err(e)?;
            let items=st.query_map([sid],|r|Ok(ItemDraft{ingredient:r.get(0)?,grams:r.get(1)?,tolerance_pct:r.get(2)?,head:head_from(r.get(3)?)})).map_err(e)?.collect::<std::result::Result<_,_>>().map_err(e)?;
            d.steps.push(StepDraft{topup,items});
        }
        Ok((rid,ver,d))
    }
    /// Versions of a recipe, newest first: (version id, number, created, pot runs).
    pub fn recipe_versions(&self,recipe:i64)->Result<Vec<(i64,i64,i64,usize)>>{
        let mut st=self.conn.prepare("SELECT v.id,v.version,v.created_ms,(SELECT count(*) FROM pot_run p WHERE p.recipe_version_id=v.id) FROM recipe_version v WHERE v.recipe_id=?1 ORDER BY v.version DESC").map_err(e)?;
        let rows=st.query_map([recipe],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get::<_,i64>(3)? as usize))).map_err(e)?;
        rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    /// Save a recipe. New recipe → version 1. Existing recipe whose current version was used by a pot run →
    /// new version (old runs keep pointing at theirs); unused → the current version is replaced.
    pub fn save_recipe(&mut self,profile:ProfileKind,recipe:Option<i64>,d:&RecipeDraft)->Result<SaveOutcome>{
        let pid=self.profile(profile)?.id;
        let names:Vec<String>=self.ingredient_rows(profile)?.into_iter().map(|r|r.name).collect();
        let errs=d.structural_errors(&|n|names.iter().any(|x|x==n));
        if !errs.is_empty(){return Err(errs.join("; "));}
        let name=d.name.trim().to_string();
        let tx=self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
        let clash:Option<i64>=tx.query_row("SELECT id FROM recipe WHERE profile_id=?1 AND name=?2 AND id<>?3",params![pid,name,recipe.unwrap_or(-1)],|r|r.get(0)).optional().map_err(e)?;
        if clash.is_some(){return Err(format!("Recept {name} bestaat al"));}
        let now=now_ms();
        let (rid,vid,ver,new_version)=match recipe{
            None=>{
                tx.execute("INSERT INTO recipe(profile_id,name,description,pot_g,tolerance_pct,created_ms) VALUES(?1,?2,?3,?4,?5,?6)",params![pid,name,d.description,d.pot_g,d.tolerance_pct,now]).map_err(e)?;
                let rid=tx.last_insert_rowid();
                tx.execute("INSERT INTO recipe_version(recipe_id,version,created_ms,description,pot_g,tolerance_pct,pot_height_cm) VALUES(?1,1,?2,?3,?4,?5,?6)",params![rid,now,d.description,d.pot_g,d.tolerance_pct,d.pot_height_cm]).map_err(e)?;
                (rid,tx.last_insert_rowid(),1,true)
            }
            Some(rid)=>{
                let (cur,ver):(i64,i64)=tx.query_row("SELECT v.id,v.version FROM recipe r JOIN recipe_version v ON v.id=r.current_version WHERE r.id=?1",[rid],|r|Ok((r.get(0)?,r.get(1)?))).map_err(e)?;
                let used:i64=tx.query_row("SELECT count(*) FROM pot_run WHERE recipe_version_id=?1",[cur],|r|r.get(0)).map_err(e)?;
                tx.execute("UPDATE recipe SET name=?2,description=?3,pot_g=?4,tolerance_pct=?5 WHERE id=?1",params![rid,name,d.description,d.pot_g,d.tolerance_pct]).map_err(e)?;
                if used>0{
                    tx.execute("INSERT INTO recipe_version(recipe_id,version,created_ms,description,pot_g,tolerance_pct,pot_height_cm) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![rid,ver+1,now,d.description,d.pot_g,d.tolerance_pct,d.pot_height_cm]).map_err(e)?;
                    (rid,tx.last_insert_rowid(),ver+1,true)
                }else{
                    tx.execute("DELETE FROM recipe_step WHERE version_id=?1",[cur]).map_err(e)?;
                    tx.execute("UPDATE recipe_version SET description=?2,pot_g=?3,tolerance_pct=?4,pot_height_cm=?5 WHERE id=?1",params![cur,d.description,d.pot_g,d.tolerance_pct,d.pot_height_cm]).map_err(e)?;
                    (rid,cur,ver,false)
                }
            }
        };
        for (k,s) in d.steps.iter().enumerate(){
            tx.execute("INSERT INTO recipe_step(version_id,idx,topup) VALUES(?1,?2,?3)",params![vid,k as i64,s.topup as i64]).map_err(e)?;
            let sid=tx.last_insert_rowid();
            for (j,i) in s.items.iter().enumerate(){
                let iid:i64=tx.query_row("SELECT id FROM ingredient WHERE profile_id=?1 AND name=?2",params![pid,i.ingredient.trim()],|r|r.get(0)).map_err(e)?;
                tx.execute("INSERT INTO recipe_item(step_id,idx,ingredient_id,grams,tolerance_pct,head) VALUES(?1,?2,?3,?4,?5,?6)",params![sid,j as i64,iid,i.grams,i.tolerance_pct,i.head.map(|h|h.name())]).map_err(e)?;
            }
        }
        tx.execute("UPDATE recipe SET current_version=?2 WHERE id=?1",params![rid,vid]).map_err(e)?;
        tx.commit().map_err(e)?;
        crate::applog::info("RECEPT",&format!("Recept {name} opgeslagen als versie {ver}{}",if new_version && ver>1{" (nieuwe versie: vorige versie is gebruikt)"}else{""}));
        Ok(SaveOutcome{recipe_id:rid,version_id:vid,version:ver,new_version})
    }
    pub fn set_recipe_archived(&self,recipe:i64,archived:bool)->Result<()>{
        self.conn.execute("UPDATE recipe SET archived=?2 WHERE id=?1",params![recipe,archived as i64]).map(|_|()).map_err(e)
    }
}

// ---------- plan: pump per item for a run ----------
#[derive(Clone,Debug,PartialEq)]
pub struct PlanItem{pub step:usize,pub item:usize,pub ingredient_id:i64,pub ingredient:String,pub grams:f64,pub tolerance_pct:f64,pub pump:String,pub head:Head,pub calibration_id:i64}
#[derive(Clone,Debug,PartialEq)]
pub struct PlanStep{pub idx:usize,pub topup:bool,pub items:Vec<PlanItem>}
#[derive(Clone,Debug,PartialEq)]
pub struct PotPlan{pub version_id:i64,pub recipe:String,pub version:i64,pub steps:Vec<PlanStep>,pub pot_height_cm:Option<f64>}
impl PotPlan{
    /// Heads that dose in this recipe, LH1 first.
    pub fn heads(&self)->Vec<Head>{Head::ALL.into_iter().filter(|h|self.steps.iter().any(|s|s.items.iter().any(|i|i.head==*h))).collect()}
    /// The steps of one head (items of other heads left out; steps without items on this head skipped).
    pub fn part(&self,h:Head)->Vec<PlanStep>{
        self.steps.iter().filter_map(|s|{let items:Vec<PlanItem>=s.items.iter().filter(|i|i.head==h).cloned().collect();(!items.is_empty()).then(||PlanStep{idx:s.idx,topup:s.topup,items})}).collect()
    }
}
impl Db{
    /// Validate a recipe version for a run and choose a pump per item (R2). `hardware` = mode of the CAN
    /// connection (calibration rule of iteration 2). Errors name every problem.
    pub fn resolve_plan(&self,profile:ProfileKind,version_id:i64,hardware:bool)->std::result::Result<PotPlan,Vec<String>>{
        let one=|x:String|vec![x];
        let (_,ver,d)=self.recipe_version(version_id).map_err(one)?;
        let rows=self.ingredient_rows(profile).map_err(one)?;
        let mut errs=d.structural_errors(&|n|rows.iter().any(|r|r.name==n));
        let mode=if hardware{"hardware"}else{"simulatie"};
        let mut plan=PotPlan{version_id,recipe:d.name.clone(),version:ver,steps:vec![],pot_height_cm:d.pot_height_cm};
        for (k,s) in d.steps.iter().enumerate(){
            let mut ps=PlanStep{idx:k,topup:s.topup,items:vec![]};
            for (j,i) in s.items.iter().enumerate(){
                let Some(row)=rows.iter().find(|r|r.name==i.ingredient.trim()) else{continue};
                if row.pumps.is_empty(){errs.push(format!("Stap {}: {} heeft geen pomp",k+1,row.name));continue;}
                let mut cands:Vec<(&PumpLink,Option<i64>,String)>=vec![];
                for p in &row.pumps{
                    if i.head.is_some() && p.head!=i.head{continue;}
                    if !p.enabled{cands.push((p,None,format!("pomp {} staat uit",p.pump)));continue;}
                    match self.calibration_for_dosing(profile,&p.pump,calib::KIND_VSD,hardware){
                        Ok(c)=>cands.push((p,Some(c.id),String::new())),
                        Err(_)=>cands.push((p,None,format!("pomp {} niet gekalibreerd ({mode})",p.pump))),
                    }
                }
                let mut ok:Vec<&(&PumpLink,Option<i64>,String)>=cands.iter().filter(|c|c.1.is_some()).collect();
                // Preference: head of the item, head preference of the ingredient, preferred pump, LH1 before LH2.
                ok.sort_by_key(|c|(row.head_pref.is_some_and(|h|c.0.head!=Some(h)),!c.0.preferred,c.0.head.map(|h|h.idx()).unwrap_or(9)));
                match ok.first(){
                    Some((p,Some(cal),_))=>ps.items.push(PlanItem{step:k,item:j,ingredient_id:row.id,ingredient:row.name.clone(),grams:i.grams,
                        tolerance_pct:i.tolerance_pct.or(row.tolerance_pct).unwrap_or(d.tolerance_pct),pump:p.pump.clone(),head:p.head.unwrap_or(Head::LH1),calibration_id:*cal}),
                    _=>{let why=if cands.is_empty(){format!("geen pomp op {}",i.head.map(|h|h.name()).unwrap_or("—"))}else{cands.iter().map(|c|c.2.clone()).collect::<Vec<_>>().join(", ")};
                        errs.push(format!("Stap {}: {} kan niet doseren: {why}",k+1,row.name));}
                }
            }
            // Two items of one parallel step on the same pump cannot run together.
            for (a,x) in ps.items.iter().enumerate(){if ps.items[..a].iter().any(|y|y.pump==x.pump){errs.push(format!("Stap {}: {} en een ander ingrediënt op dezelfde pomp {}",k+1,x.ingredient,x.pump));}}
            plan.steps.push(ps);
        }
        if errs.is_empty(){Ok(plan)}else{errs.dedup();Err(errs)}
    }
}

// ---------- weights ----------
/// Parallel items on one head share the net weight of that head in proportion to the requested grams (Camelot).
pub fn attribute(net:f64,requested:&[f64])->Vec<f64>{
    let sum:f64=requested.iter().sum();
    if requested.len()==1{return vec![net];}
    if sum<=0.{return vec![0.;requested.len()];}
    requested.iter().map(|r|net*r/sum).collect()
}
/// ok / te_weinig / te_veel against ±tolerance.
pub fn judge(requested:f64,dosed:f64,tol:f64)->&'static str{
    let dev=100.*(dosed-requested)/requested;
    if dev< -tol-1e-9{"te_weinig"}else if dev>tol+1e-9{"te_veel"}else{"ok"}
}
pub fn status_label(s:&str)->&'static str{match s{"ok"=>"binnen tolerantie","te_weinig"=>"te weinig","te_veel"=>"te veel","gestopt"=>"gestopt","fout"=>"fout",_=>"—"}}

// ---------- pot runs ----------
pub const POT_RUNNING:&str="loopt";pub const POT_DONE:&str="klaar";pub const POT_STOPPED:&str="gestopt";pub const POT_FAILED:&str="fout";pub const POT_INTERRUPTED:&str="onderbroken";
#[derive(Clone,Debug,PartialEq)]
pub struct PotRunRow{pub id:i64,pub version_id:i64,pub recipe:String,pub version:i64,pub simulation:bool,pub mode:String,pub status:String,pub started_ms:i64,pub ended_ms:Option<i64>,pub barcode:Option<String>,pub order_id:Option<i64>,pub note:String}
#[derive(Clone,Debug,PartialEq,Default)]
pub struct PotItemRow{pub id:i64,pub pot_run_id:i64,pub step_idx:i64,pub item_idx:i64,pub ingredient_id:Option<i64>,pub ingredient:String,pub pump:String,pub head:String,
    pub requested_g:f64,pub dosed_g:Option<f64>,pub attributed:bool,pub topped_up:bool,pub deviation_pct:Option<f64>,pub tolerance_pct:f64,pub status:String,
    pub calibration_id:Option<i64>,pub steps:Option<i64>,pub started_ms:Option<i64>,pub ended_ms:Option<i64>,pub note:String}
const PCOLS:&str="p.id,p.recipe_version_id,r.name,v.version,p.simulation,p.mode,p.status,p.started_ms,p.ended_ms,p.barcode,p.order_id,p.note";
const ICOLS:&str="id,pot_run_id,step_idx,item_idx,ingredient_id,ingredient,pump,head,requested_g,dosed_g,attributed,topped_up,deviation_pct,tolerance_pct,status,calibration_id,steps,started_ms,ended_ms,note";
fn prow(r:&rusqlite::Row)->rusqlite::Result<PotRunRow>{Ok(PotRunRow{id:r.get(0)?,version_id:r.get(1)?,recipe:r.get(2)?,version:r.get(3)?,simulation:r.get::<_,i64>(4)?!=0,mode:r.get(5)?,status:r.get(6)?,started_ms:r.get(7)?,ended_ms:r.get(8)?,barcode:r.get(9)?,order_id:r.get(10)?,note:r.get(11)?})}
fn irow(r:&rusqlite::Row)->rusqlite::Result<PotItemRow>{Ok(PotItemRow{id:r.get(0)?,pot_run_id:r.get(1)?,step_idx:r.get(2)?,item_idx:r.get(3)?,ingredient_id:r.get(4)?,ingredient:r.get(5)?,pump:r.get(6)?,head:r.get(7)?,
    requested_g:r.get(8)?,dosed_g:r.get(9)?,attributed:r.get::<_,i64>(10)?!=0,topped_up:r.get::<_,i64>(11)?!=0,deviation_pct:r.get(12)?,tolerance_pct:r.get(13)?,status:r.get(14)?,
    calibration_id:r.get(15)?,steps:r.get(16)?,started_ms:r.get(17)?,ended_ms:r.get(18)?,note:r.get(19)?})}
impl Db{
    pub fn create_pot_run(&self,profile:ProfileKind,version_id:i64,simulation:bool,mode:&str,barcode:Option<&str>,order:Option<i64>)->Result<i64>{
        let pid=self.profile(profile)?.id;
        self.conn.execute("INSERT INTO pot_run(profile_id,recipe_version_id,simulation,mode,status,started_ms,barcode,order_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![pid,version_id,simulation as i64,mode,POT_RUNNING,now_ms(),barcode,order]).map_err(e)?;Ok(self.conn.last_insert_rowid())
    }
    pub fn add_pot_item(&self,i:&PotItemRow)->Result<i64>{
        self.conn.execute(&format!("INSERT INTO pot_item({}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",&ICOLS[3..]),
            params![i.pot_run_id,i.step_idx,i.item_idx,i.ingredient_id,i.ingredient,i.pump,i.head,i.requested_g,i.dosed_g,i.attributed as i64,i.topped_up as i64,i.deviation_pct,i.tolerance_pct,i.status,i.calibration_id,i.steps,i.started_ms,i.ended_ms,i.note]).map_err(e)?;
        Ok(self.conn.last_insert_rowid())
    }
    pub fn finish_pot_run(&self,id:i64,status:&str,note:&str)->Result<()>{
        self.conn.execute("UPDATE pot_run SET status=?2,ended_ms=?3,note=CASE WHEN ?4='' THEN note ELSE ?4 END WHERE id=?1",params![id,status,now_ms(),note]).map(|_|()).map_err(e)
    }
    pub fn pot_run(&self,id:i64)->Result<PotRunRow>{
        self.conn.query_row(&format!("SELECT {PCOLS} FROM pot_run p JOIN recipe_version v ON v.id=p.recipe_version_id JOIN recipe r ON r.id=v.recipe_id WHERE p.id=?1"),[id],prow).map_err(e)
    }
    /// Pot runs of a recipe (all versions), newest first.
    pub fn pot_runs(&self,recipe:i64)->Result<Vec<PotRunRow>>{
        let mut st=self.conn.prepare(&format!("SELECT {PCOLS} FROM pot_run p JOIN recipe_version v ON v.id=p.recipe_version_id JOIN recipe r ON r.id=v.recipe_id WHERE r.id=?1 ORDER BY p.started_ms DESC,p.id DESC")).map_err(e)?;
        let rows=st.query_map([recipe],prow).map_err(e)?;rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    pub fn pot_items(&self,run:i64)->Result<Vec<PotItemRow>>{
        let mut st=self.conn.prepare(&format!("SELECT {ICOLS} FROM pot_item WHERE pot_run_id=?1 ORDER BY step_idx,item_idx,id")).map_err(e)?;
        let rows=st.query_map([run],irow).map_err(e)?;rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    /// Pot runs still "loopt" after a restart: mark them interrupted (doses so far stay).
    pub fn close_stale_pots(&self)->Result<usize>{
        self.conn.execute("UPDATE pot_run SET status=?1,note=CASE WHEN note='' THEN 'App afgesloten tijdens pot' ELSE note END WHERE status=?2",params![POT_INTERRUPTED,POT_RUNNING]).map_err(e)
    }
}

#[cfg(test)] pub(crate) mod tests{
    use super::*;
    use crate::calib::NewCal;
    pub fn temp(n:&str)->std::path::PathBuf{let d=std::env::temp_dir().join(format!("pumpstudio-db-rec-{n}-{}-{}",std::process::id(),now_ms()));std::fs::create_dir_all(&d).unwrap();d}
    pub fn cleanup(d:&std::path::Path){for _ in 0..100{if std::fs::remove_dir_all(d).is_ok() || !d.exists(){return;}std::thread::sleep(std::time::Duration::from_millis(20));}}
    pub fn item(n:&str,g:f64)->ItemDraft{ItemDraft{ingredient:n.into(),grams:g,..Default::default()}}
    pub fn step(items:Vec<ItemDraft>)->StepDraft{StepDraft{topup:false,items}}
    /// R1 recipe: seq A (LH1), parallel {B,C} (LH1), parallel {D (LH1), E (LH2)}, seq F (LH2).
    /// A..D = pumps P_C01..P_C04 (LH1), E, F = P_A01, P_A02 (LH2); simulation calibrations for all six.
    pub fn r1_setup(db:&mut Db)->(i64,RecipeDraft){
        let vsd=db.profile(ProfileKind::Vsd).unwrap().id;
        for (pump,ing) in [("P_C01","A"),("P_C02","B"),("P_C03","C"),("P_C04","D"),("P_A01","E"),("P_A02","F")]{
            let mut r=db.pumps(vsd).unwrap().into_iter().find(|p|p.name==pump).unwrap();r.ingredient=ing.into();db.update_pump(&r).unwrap();
            db.save_calibration(&NewCal::from_vsd(&crate::calib::tests_sample(pump,0.5),true,"")).unwrap();
        }
        let d=RecipeDraft{name:"R1".into(),steps:vec![step(vec![item("A",1.)]),step(vec![item("B",2.),item("C",1.)]),step(vec![item("D",0.5),item("E",1.5)]),step(vec![item("F",3.)])],..Default::default()};
        let out=db.save_recipe(ProfileKind::Vsd,None,&d).unwrap();(out.version_id,d)
    }
    #[test]fn r1_plan_order_and_heads(){
        let dir=temp("r1");let mut db=Db::open(&dir.join("p.db")).unwrap();let (v,_)=r1_setup(&mut db);
        let p=db.resolve_plan(ProfileKind::Vsd,v,false).unwrap();
        assert_eq!(p.heads(),vec![Head::LH1,Head::LH2]);
        let lh1:Vec<Vec<String>>=p.part(Head::LH1).iter().map(|s|s.items.iter().map(|i|i.ingredient.clone()).collect()).collect();
        let lh2:Vec<Vec<String>>=p.part(Head::LH2).iter().map(|s|s.items.iter().map(|i|i.ingredient.clone()).collect()).collect();
        assert_eq!(lh1,vec![vec!["A"],vec!["B","C"],vec!["D"]]);assert_eq!(lh2,vec![vec!["E"],vec!["F"]]);
        assert_eq!(p.steps[2].items.iter().map(|i|(i.pump.as_str(),i.head)).collect::<Vec<_>>(),vec![("P_C04",Head::LH1),("P_A01",Head::LH2)]);
        drop(db);cleanup(&dir);
    }
    #[test]fn r2_validation_refuses_with_message(){
        let dir=temp("r2");let mut db=Db::open(&dir.join("p.db")).unwrap();let (v,mut d)=r1_setup(&mut db);
        // Hardware: only simulation calibrations exist.
        let e=db.resolve_plan(ProfileKind::Vsd,v,true).unwrap_err();assert!(e.iter().any(|x|x.contains("A kan niet doseren: pomp P_C01 niet gekalibreerd (hardware)")),"{e:?}");
        // Ingredient without pump; inactive pump.
        db.add_ingredient(ProfileKind::Vsd,"Zonder").unwrap();
        let vsd=db.profile(ProfileKind::Vsd).unwrap().id;let mut r=db.pumps(vsd).unwrap().into_iter().find(|p|p.name=="P_C02").unwrap();r.enabled=false;db.update_pump(&r).unwrap();
        d.steps.push(step(vec![item("Zonder",1.)]));let out=db.save_recipe(ProfileKind::Vsd,Some(1),&d).unwrap();
        let e=db.resolve_plan(ProfileKind::Vsd,out.version_id,false).unwrap_err();
        assert!(e.iter().any(|x|x.contains("Zonder heeft geen pomp")) && e.iter().any(|x|x.contains("B kan niet doseren: pomp P_C02 staat uit")),"{e:?}");
        // Structure: duplicate in a parallel step, 0 and negative grams, empty recipe, empty step, unknown ingredient.
        let bad=RecipeDraft{name:"X".into(),steps:vec![step(vec![item("A",1.),item("A",2.)]),step(vec![item("C",0.)]),step(vec![item("D",-1.)]),step(vec![]),step(vec![item("Onbekend",1.)])],..Default::default()};
        let e=db.save_recipe(ProfileKind::Vsd,None,&bad).unwrap_err();
        for s in ["A staat twee keer","Stap 2 C: gewicht moet groter dan 0 g","Stap 3 D: gewicht moet groter dan 0 g","Stap 4: leeg","Onbekend bestaat niet"]{assert!(e.contains(s),"{s} ontbreekt in {e}");}
        assert!(db.save_recipe(ProfileKind::Vsd,None,&RecipeDraft{name:"Leeg".into(),..Default::default()}).unwrap_err().contains("Leeg recept"));
        assert!(db.save_recipe(ProfileKind::Vsd,None,&RecipeDraft{name:"R1".into(),steps:vec![step(vec![item("A",1.)])],..Default::default()}).unwrap_err().contains("bestaat al"));
        drop(db);cleanup(&dir);
    }
    #[test]fn r3_parallel_attribution_and_judgement(){
        // Head net 3,03 g for B 2 g + C 1 g → 2,02 + 1,01 (both +1 %).
        let a=attribute(3.03,&[2.,1.]);assert!((a[0]-2.02).abs()<1e-12 && (a[1]-1.01).abs()<1e-12);
        let b=attribute(10.,&[1.,1.5,2.5]);assert_eq!(b,vec![2.,3.,5.]);assert_eq!(attribute(0.5,&[0.5]),vec![0.5]);
        assert_eq!(judge(2.,2.02,1.),"ok");assert_eq!(judge(2.,2.021,1.),"te_veel");assert_eq!(judge(2.,1.97,1.),"te_weinig");
    }
    #[test]fn r6_editing_a_used_recipe_makes_a_new_version(){
        let dir=temp("r6");let mut db=Db::open(&dir.join("p.db")).unwrap();let (v1,mut d)=r1_setup(&mut db);
        // Unused: edit replaces version 1.
        d.description="eerste".into();let o=db.save_recipe(ProfileKind::Vsd,Some(1),&d).unwrap();assert_eq!((o.version,o.version_id,o.new_version),(1,v1,false));
        let run=db.create_pot_run(ProfileKind::Vsd,v1,true,"handmatig",None,None).unwrap();
        d.steps[0].items[0].grams=1.5;let o2=db.save_recipe(ProfileKind::Vsd,Some(1),&d).unwrap();assert_eq!((o2.version,o2.new_version),(2,true));assert_ne!(o2.version_id,v1);
        assert_eq!(db.recipe_version(v1).unwrap().2.steps[0].items[0].grams,1.,"oude versie onveranderd");
        assert_eq!(db.pot_run(run).unwrap().version,1,"oude run toont oude versie");
        let rows=db.recipes(ProfileKind::Vsd).unwrap();assert_eq!((rows[0].version,rows[0].used,rows[0].steps),(2,1,4));assert!((rows[0].total_g-9.5).abs()<1e-9);
        assert_eq!(db.recipe_versions(1).unwrap().iter().map(|x|(x.1,x.3)).collect::<Vec<_>>(),vec![(2,0),(1,1)]);
        drop(db);cleanup(&dir);
    }
    #[test]fn r7_ingredient_delete_refused_when_used_and_csv(){
        let dir=temp("r7");let mut db=Db::open(&dir.join("p.db")).unwrap();let (_,_)=r1_setup(&mut db);
        let a=db.ingredient_by_name(ProfileKind::Vsd,"A").unwrap().unwrap();
        let e=db.delete_ingredient(a).unwrap_err();assert!(e.contains("A wordt gebruikt in: R1 v1"),"{e}");
        let z=db.add_ingredient(ProfileKind::Vsd,"Los").unwrap();db.delete_ingredient(z).unwrap();assert!(db.ingredient_by_name(ProfileKind::Vsd,"Los").unwrap().is_none());
        assert!(db.add_ingredient(ProfileKind::Vsd,"A").unwrap_err().contains("bestaat al"));
        // Pump link follows the Pompen page (trigger), preferred pump settable.
        let rows=db.ingredient_rows(ProfileKind::Vsd).unwrap();let ra=rows.iter().find(|r|r.name=="A").unwrap();assert_eq!(ra.pumps.iter().map(|p|p.pump.as_str()).collect::<Vec<_>>(),vec!["P_C01"]);
        db.set_preferred_pump(a,Some(ra.pumps[0].pump_id)).unwrap();assert!(db.ingredient_rows(ProfileKind::Vsd).unwrap().iter().find(|r|r.name=="A").unwrap().pumps[0].preferred);
        // CSV round trip (Dutch).
        let mut row=ra.clone();row.density=Some(1.25);row.tolerance_pct=Some(0.5);row.notes="rood; dun".into();db.save_ingredient(&row).unwrap();
        let csv=db.ingredients_csv(ProfileKind::Vsd).unwrap();let t=String::from_utf8(csv[3..].to_vec()).unwrap();assert!(t.contains("A;1,2500;0,50;;\"rood; dun\";P_C01 (LH1)"),"{t}");
        let msg=db.import_ingredients_csv(ProfileKind::Vsd,"\u{feff}Naam;Dichtheid;Tolerantie;Voorkeurkop;Opmerking\nA;1,5;;LH1;\nNieuw;0,9;2;;geïmporteerd\n".as_bytes()).unwrap();
        assert_eq!(msg,"1 toegevoegd, 1 bijgewerkt");let rows=db.ingredient_rows(ProfileKind::Vsd).unwrap();
        let ra=rows.iter().find(|r|r.name=="A").unwrap();assert_eq!((ra.density,ra.head_pref),(Some(1.5),Some(Head::LH1)));assert!(rows.iter().any(|r|r.name=="Nieuw" && r.tolerance_pct==Some(2.)));
        assert!(db.import_ingredients_csv(ProfileKind::Vsd,b"Naam;Dichtheid\nX;abc\n").unwrap_err().contains("Regel 2"));
        let w=ingredient_warnings(&rows,&db.pumps(db.profile(ProfileKind::Vsd).unwrap().id).unwrap(),&|p|p!="P_C03","simulatie");
        assert!(w.iter().any(|x|x=="Nieuw: geen pomp gekoppeld") && w.iter().any(|x|x.contains("C: pomp P_C03 niet gekalibreerd")),"{w:?}");
        drop(db);cleanup(&dir);
    }
}
