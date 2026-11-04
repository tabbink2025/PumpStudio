//! Calibrations in the database (schema v2): one `calibration` row per run with its measuring points in
//! `calibration_point` (positions in steps). The newest successful calibration of a pump, kind **and mode**
//! (simulation or hardware) is "actief"; earlier ones of that same mode become "archief" (a simulation
//! never archives or replaces a hardware calibration); a stopped or failed run is stored as "afgebroken" and never
//! becomes active. The full result (VSD `VsdCalibration`, ceramic curve) is kept as JSON in `data`, so
//! dosing uses exactly what was measured.
use crate::{db::{Db,ProfileKind},model::{stats,Curve,Result},vsdcal::VsdCalibration,workflow::State};
use rusqlite::{params,OptionalExtension};
use std::{collections::HashMap,path::Path};

pub const KIND_VSD:&str="vsd";
pub const KIND_BENCH_CAL:&str="keramisch_kalibratie";
pub const KIND_BENCH_CURVE:&str="keramisch_curve";
pub const ACTIVE:&str="actief";pub const ARCHIVE:&str="archief";pub const ABORTED:&str="afgebroken";

pub fn kind_label(k:&str)->&'static str{match k{KIND_VSD=>"VSD (Camelot)",KIND_BENCH_CAL=>"Kalibratie g/omw",KIND_BENCH_CURVE=>"360°-meetcurve",_=>"onbekend"}}

#[derive(Clone,Debug,PartialEq)]
pub struct CalRow{pub id:i64,pub profile_id:i64,pub pump_id:Option<i64>,pub pump:String,pub date_ms:i64,pub kind:String,pub simulation:bool,pub settings:String,
    pub g_per_rev:Option<f64>,pub g_per_stroke:Option<f64>,pub spread:Option<f64>,pub repaired:i64,pub status:String,pub note:String,pub source:String,
    pub check_requested:Option<f64>,pub check_measured:Option<f64>,pub check_error_pct:Option<f64>}
/// One measuring point. VSD: phase 1 = part of a stroke, phase 2 = whole revolutions; run 0 = averaged
/// (and repaired) values, run 1.. = raw runs. Ceramic: phase 1 = calibration repeats, phase 3 = 360° curve.
#[derive(Clone,Debug,PartialEq)]
pub struct CalPoint{pub phase:i64,pub run:i64,pub idx:i64,pub position_steps:i64,pub grams:f64,pub repaired:bool}

/// A calibration ready to be stored.
#[derive(Clone,Debug)]
pub struct NewCal{pub profile:ProfileKind,pub pump:String,pub date_ms:i64,pub kind:&'static str,pub simulation:bool,pub settings:String,
    pub g_per_rev:Option<f64>,pub g_per_stroke:Option<f64>,pub spread:Option<f64>,pub repaired:i64,pub ok:bool,pub note:String,pub source:String,
    pub data:String,pub points:Vec<CalPoint>}

fn now_ms()->i64{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis() as i64).unwrap_or(0)}
fn sd(v:&[f64])->Option<f64>{if v.len()<2{None}else{Some(stats(v).1)}}

impl NewCal{
    /// VSD calibration (complete or partial). `ok` = finished; otherwise stored as "afgebroken".
    pub fn from_vsd(cal:&VsdCalibration,ok:bool,note:&str)->Self{
        let spr=cal.steps_per_rev();let s=&cal.settings;let mut pts=vec![];
        let pos=crate::vsdcal::cosine_positions(s.phase1_samples,s.phase1_max_deg);
        let at=|rev:f64|(rev*spr as f64).round() as i64;
        for (r,run) in cal.pd_runs.iter().enumerate(){for (k,g) in run.iter().enumerate(){pts.push(CalPoint{phase:1,run:r as i64+1,idx:k as i64,position_steps:at(pos.get(k).copied().unwrap_or(0.)),grams:*g,repaired:false});}}
        for (k,p) in cal.pd.iter().enumerate(){pts.push(CalPoint{phase:1,run:0,idx:k as i64,position_steps:at(p.pos),grams:p.g,repaired:p.repaired});}
        for (r,run) in cal.stroke_runs.iter().enumerate(){for (k,g) in run.iter().enumerate(){pts.push(CalPoint{phase:2,run:r as i64+1,idx:k as i64,position_steps:s.stroke_samples.get(k).map(|n|*n as i64*spr).unwrap_or(0),grams:*g,repaired:false});}}
        for (k,p) in cal.stroke.iter().enumerate(){pts.push(CalPoint{phase:2,run:0,idx:k as i64,position_steps:p.rot as i64*spr,grams:p.g,repaired:false});}
        // g/omw from the longest phase-2 measurement; spread = standard deviation of that rate between runs.
        let last=s.stroke_samples.len().checked_sub(1);
        let rates:Vec<f64>=match last{Some(k) if s.phase2=>cal.stroke_runs.iter().filter_map(|r|r.get(k).map(|g|g/s.stroke_samples[k] as f64)).collect(),_=>vec![]};
        let g_per_rev=if ok{cal.stroke.last().map(|p|p.g/p.rot.max(1) as f64).or(Some(cal.g_per_stroke))}else{None};
        let spread=sd(&rates).or_else(||sd(&cal.pd_runs.iter().map(|r|r.iter().cloned().fold(0.,f64::max)).collect::<Vec<_>>()));
        Self{profile:ProfileKind::Vsd,pump:cal.pump.clone(),date_ms:if cal.date_ms>0{cal.date_ms as i64}else{now_ms()},kind:KIND_VSD,simulation:cal.simulation,
            settings:serde_json::to_string(&cal.settings).unwrap_or_default(),g_per_rev,g_per_stroke:if ok{Some(cal.g_per_stroke)}else{None},spread,
            repaired:cal.pd.iter().filter(|p|p.repaired).count() as i64,ok,note:note.into(),source:String::new(),data:serde_json::to_string(cal).unwrap_or_default(),points:pts}
    }
    /// Ceramic calibration (`KIND_BENCH_CAL`) or 360° curve (`KIND_BENCH_CURVE`) from the bench engine state.
    pub fn from_bench(kind:&'static str,pump:&str,st:&State,ok:bool,note:&str)->Self{
        let n=st.config.steps_rev.max(1);let mut pts=vec![];let (g_per_rev,spread,data);
        if kind==KIND_BENCH_CURVE{
            for (k,r) in st.records.iter().filter(|r|r.kind=="360°").enumerate(){
                if let (Some(a),Some(c))=(r.angle_deg,r.cumulative_g){pts.push(CalPoint{phase:3,run:r.repeat as i64,idx:k as i64,position_steps:(a/360.*n as f64).round() as i64,grams:c,repaired:false});}
            }
            let ok=ok && st.curve.is_some();
            g_per_rev=if ok{st.curve.as_ref().map(|c|c.yield_g())}else{None};
            let yields:Vec<f64>=st.curve.as_ref().map(|c|c.runs.iter().filter_map(|r|r.last().copied()).collect()).unwrap_or_default();spread=sd(&yields);
            data=serde_json::json!({"curve":st.curve,"steps_rev":n,"path":st.path}).to_string();
        }else{
            let mut yields=vec![];
            for r in st.records.iter().filter(|r|r.kind=="Kalibratie"){
                let d=r.target_steps-r.start_steps;
                if let Some(m)=r.mass_g{pts.push(CalPoint{phase:1,run:r.repeat as i64,idx:0,position_steps:d,grams:m,repaired:false});if d>0{yields.push(m*n as f64/d as f64);}}
            }
            g_per_rev=if ok{st.calibration_g_rev.or(if yields.is_empty(){None}else{Some(stats(&yields).0)})}else{None};spread=sd(&yields);
            data=serde_json::json!({"calibration_g_rev":st.calibration_g_rev,"steps_rev":n,"path":st.path}).to_string();
        }
        let ok=ok && g_per_rev.is_some();
        Self{profile:ProfileKind::Keramisch,pump:pump.into(),date_ms:if st.started_ms>0{st.started_ms as i64}else{now_ms()},kind,simulation:st.simulation,
            settings:serde_json::to_string(&st.config).unwrap_or_default(),g_per_rev,g_per_stroke:None,spread,repaired:0,ok,note:note.into(),source:String::new(),data,points:pts}
    }
}

fn row(r:&rusqlite::Row)->rusqlite::Result<CalRow>{
    Ok(CalRow{id:r.get(0)?,profile_id:r.get(1)?,pump_id:r.get(2)?,pump:r.get(3)?,date_ms:r.get(4)?,kind:r.get(5)?,simulation:r.get::<_,i64>(6)?!=0,settings:r.get(7)?,
        g_per_rev:r.get(8)?,g_per_stroke:r.get(9)?,spread:r.get(10)?,repaired:r.get(11)?,status:r.get(12)?,note:r.get(13)?,source:r.get(14)?,
        check_requested:r.get(15)?,check_measured:r.get(16)?,check_error_pct:r.get(17)?})
}
const COLS:&str="id,profile_id,pump_id,pump_name,date_ms,kind,simulation,settings,g_per_rev,g_per_stroke,spread,repaired,status,note,source,check_requested,check_measured,check_error_pct";
fn e(x:rusqlite::Error)->String{format!("Database: {x}")}

impl Db{
    /// Store a calibration with its points in one transaction. A successful one becomes "actief" and the
    /// previous active one of that pump, kind and mode "archief" (unless the stored one is newer, e.g. on import).
    pub fn save_calibration(&mut self,c:&NewCal)->Result<i64>{
        let profile=self.profile(c.profile)?.id;
        let tx=self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
        let pump_id:Option<i64>=tx.query_row("SELECT id FROM pump WHERE profile_id=?1 AND name=?2",params![profile,c.pump],|r|r.get(0)).optional().map_err(e)?;
        let status=if !c.ok{ABORTED}else{
            let sim=c.simulation as i64;
            let newer:Option<i64>=tx.query_row("SELECT id FROM calibration WHERE profile_id=?1 AND pump_name=?2 AND kind=?3 AND simulation=?4 AND status='actief' AND date_ms>?5",params![profile,c.pump,c.kind,sim,c.date_ms],|r|r.get(0)).optional().map_err(e)?;
            if newer.is_some(){ARCHIVE}else{
                tx.execute("UPDATE calibration SET status='archief' WHERE profile_id=?1 AND pump_name=?2 AND kind=?3 AND simulation=?4 AND status='actief'",params![profile,c.pump,c.kind,sim]).map_err(e)?;ACTIVE}
        };
        tx.execute("INSERT INTO calibration(profile_id,pump_id,pump_name,date_ms,kind,simulation,settings,g_per_rev,g_per_stroke,spread,repaired,status,note,source,data)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",params![profile,pump_id,c.pump,c.date_ms,c.kind,c.simulation as i64,c.settings,c.g_per_rev,c.g_per_stroke,c.spread,c.repaired,status,c.note,c.source,c.data]).map_err(e)?;
        let id=tx.last_insert_rowid();
        {let mut st=tx.prepare_cached("INSERT INTO calibration_point(calibration_id,phase,run,idx,position_steps,grams,repaired) VALUES(?1,?2,?3,?4,?5,?6,?7)").map_err(e)?;
            for p in &c.points{st.execute(params![id,p.phase,p.run,p.idx,p.position_steps,p.grams,p.repaired as i64]).map_err(e)?;}}
        tx.commit().map_err(e)?;Ok(id)
    }
    /// History of one pump, newest first.
    pub fn calibrations(&self,profile:ProfileKind,pump:&str)->Result<Vec<CalRow>>{
        let pid=self.profile(profile)?.id;
        let mut st=self.conn.prepare(&format!("SELECT {COLS} FROM calibration WHERE profile_id=?1 AND pump_name=?2 ORDER BY date_ms DESC,id DESC")).map_err(e)?;
        let rows=st.query_map(params![pid,pump],row).map_err(e)?;rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    pub fn calibration(&self,id:i64)->Result<CalRow>{
        self.conn.query_row(&format!("SELECT {COLS} FROM calibration WHERE id=?1"),[id],row).map_err(e)
    }
    /// Newest active calibration of a pump and kind in either mode (for display; hardware first when both).
    pub fn active_calibration(&self,profile:ProfileKind,pump:&str,kind:&str)->Result<Option<CalRow>>{
        Ok(match self.active_in_mode(profile,pump,kind,false)?{Some(r)=>Some(r),None=>self.active_in_mode(profile,pump,kind,true)?})
    }
    /// Active calibration of exactly this mode.
    pub fn active_in_mode(&self,profile:ProfileKind,pump:&str,kind:&str,simulation:bool)->Result<Option<CalRow>>{
        let pid=self.profile(profile)?.id;
        self.conn.query_row(&format!("SELECT {COLS} FROM calibration WHERE profile_id=?1 AND pump_name=?2 AND kind=?3 AND simulation=?4 AND status='actief' ORDER BY date_ms DESC LIMIT 1"),params![pid,pump,kind,simulation as i64],row).optional().map_err(e)
    }
    /// The one lookup for dosing with a calibration (control dose now, recipes later). Hardware uses only a
    /// hardware calibration; a simulation uses its own, else borrows the hardware one.
    pub fn calibration_for_dosing(&self,profile:ProfileKind,pump:&str,kind:&str,hardware:bool)->Result<CalRow>{
        if hardware{
            return self.active_in_mode(profile,pump,kind,false)?.ok_or_else(||format!("Geen hardware-kalibratie voor {pump}: eerst op de machine kalibreren (een simulatie-kalibratie wordt op hardware nooit gebruikt)"));
        }
        match self.active_in_mode(profile,pump,kind,true)?{Some(r)=>Ok(r),None=>self.active_in_mode(profile,pump,kind,false)?.ok_or_else(||format!("Geen actieve kalibratie voor {pump}"))}
    }
    pub fn calibration_points(&self,id:i64)->Result<Vec<CalPoint>>{
        let mut st=self.conn.prepare("SELECT phase,run,idx,position_steps,grams,repaired FROM calibration_point WHERE calibration_id=?1 ORDER BY phase,run,idx").map_err(e)?;
        let rows=st.query_map([id],|r|Ok(CalPoint{phase:r.get(0)?,run:r.get(1)?,idx:r.get(2)?,position_steps:r.get(3)?,grams:r.get(4)?,repaired:r.get::<_,i64>(5)?!=0})).map_err(e)?;
        rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    pub fn calibration_data(&self,id:i64)->Result<String>{self.conn.query_row("SELECT data FROM calibration WHERE id=?1",[id],|r|r.get(0)).map_err(e)}
    /// The VSD calibration as measured (for dosing and plots).
    pub fn vsd_calibration(&self,id:i64)->Result<VsdCalibration>{
        serde_json::from_str(&self.calibration_data(id)?).map_err(|x|format!("Kalibratie {id} onleesbaar: {x}"))
    }
    pub fn bench_curve(&self,id:i64)->Result<Option<Curve>>{
        let v:serde_json::Value=serde_json::from_str(&self.calibration_data(id)?).map_err(|x|x.to_string())?;
        Ok(serde_json::from_value(v["curve"].clone()).ok())
    }
    /// Control dose after calibration, stored with the calibration.
    /// Store a control dose with calibration `id`, but only when it was done in the same mode as that
    /// calibration (`simulation` = mode of the CAN that dosed). A simulated check on a borrowed hardware
    /// calibration is never written; returns whether it was stored.
    pub fn set_check(&self,id:i64,simulation:bool,requested:f64,measured:Option<f64>,error_pct:Option<f64>)->Result<bool>{
        let n=self.conn.execute("UPDATE calibration SET check_requested=?2,check_measured=?3,check_error_pct=?4 WHERE id=?1 AND simulation=?5",params![id,requested,measured,error_pct,simulation as i64]).map_err(e)?;
        Ok(n==1)
    }
    /// Active calibration per pump for the Pompen page: date and mode (simulation?) of the newest active
    /// hardware calibration, else of the newest active simulation calibration.
    pub fn active_dates(&self,profile:ProfileKind)->Result<HashMap<String,(i64,bool)>>{
        let pid=self.profile(profile)?.id;
        let mut st=self.conn.prepare("SELECT pump_name,simulation,max(date_ms) FROM calibration WHERE profile_id=?1 AND status='actief' GROUP BY pump_name,simulation ORDER BY simulation DESC").map_err(e)?;
        let rows=st.query_map([pid],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?!=0,r.get::<_,i64>(2)?))).map_err(e)?;
        let mut out=HashMap::new();
        // Simulation rows first, hardware rows after them overwrite: hardware wins.
        for x in rows{let (p,sim,d)=x.map_err(e)?;out.insert(p,(d,sim));}
        Ok(out)
    }
    /// One-time import of the previous app's files (left in place): `machine/calibration/*.json` (VSD) and
    /// `results/characterization_*.json` (ceramic curves). Runs once (meta flag) and skips files already
    /// imported (source), so running it again changes nothing.
    pub fn import_calibrations(&mut self,root:&Path)->Result<Option<String>>{
        if self.meta("import_calibrations")?.is_some(){return Ok(None);}
        let list=|dir:&Path,pre:&str|->Vec<std::path::PathBuf>{let mut v:Vec<_>=std::fs::read_dir(dir).into_iter().flatten().flatten().map(|d|d.path())
            .filter(|p|p.extension().is_some_and(|x|x=="json") && p.file_name().is_some_and(|n|n.to_string_lossy().starts_with(pre))).collect();v.sort();v};
        let mut cals:Vec<NewCal>=vec![];let mut skipped=vec![];
        let vsd_names:Vec<String>=self.pumps(self.profile(ProfileKind::Vsd)?.id)?.into_iter().map(|p|p.name).collect();
        for p in list(&root.join("machine").join("calibration"),""){
            match std::fs::read(&p).ok().and_then(|b|serde_json::from_slice::<VsdCalibration>(&b).ok()){
                Some(c) if vsd_names.contains(&c.pump)=>{let mut n=NewCal::from_vsd(&c,c.g_per_stroke>0.,"Geïmporteerd uit oude app");n.source=p.display().to_string();cals.push(n);}
                _=>skipped.push(p.display().to_string()),
            }
        }
        let bench=self.pumps(self.profile(ProfileKind::Keramisch)?.id)?.into_iter().next().map(|p|p.name).unwrap_or("Keramisch 1".into());
        for p in list(&root.join("results"),"characterization_"){
            match std::fs::read(&p).ok().and_then(|b|serde_json::from_slice::<State>(&b).ok()){
                Some(st)=>{let mut n=NewCal::from_bench(KIND_BENCH_CURVE,&bench,&st,st.curve.is_some(),"Geïmporteerd uit oude app");n.source=p.display().to_string();cals.push(n);}
                None=>skipped.push(p.display().to_string()),
            }
        }
        cals.sort_by_key(|c|c.date_ms);
        let (mut v,mut k)=(0,0);
        for c in &cals{
            let seen:Option<i64>=self.conn.query_row("SELECT id FROM calibration WHERE source=?1",[&c.source],|r|r.get(0)).optional().map_err(e)?;
            if seen.is_some(){continue;}
            self.save_calibration(c)?;if c.kind==KIND_VSD{v+=1}else{k+=1}
        }
        self.set_meta("import_calibrations",&format!("{v} vsd, {k} keramisch"))?;
        if !skipped.is_empty(){crate::applog::warn("DATABASE",&format!("Niet geïmporteerd (onleesbaar of onbekende pomp): {}",skipped.join(", ")));}
        Ok(if v+k==0{None}else{Some(format!("Kalibraties geïmporteerd: {v} VSD, {k} keramische meetcurve(s)"))})
    }
}

/// Simulated VSD calibration for tests (0.5 g per stroke model, 2 runs).
#[cfg(test)] pub fn tests_sample(pump:&str,g:f64)->VsdCalibration{tests::sample(pump,g,1000)}
#[cfg(test)] mod tests{
    use super::*;
    use crate::vsdcal::CalSettings;
    fn temp(n:&str)->std::path::PathBuf{let d=std::env::temp_dir().join(format!("pumpstudio-db-cal-{n}-{}-{}",std::process::id(),now_ms()));std::fs::create_dir_all(&d).unwrap();d}
    fn cleanup(d:&Path){for _ in 0..100{if std::fs::remove_dir_all(d).is_ok() || !d.exists(){return;}std::thread::sleep(std::time::Duration::from_millis(20));}}
    pub fn sample(pump:&str,g:f64,date:u128)->VsdCalibration{
        let s=CalSettings{phase1_samples:11,phase1_runs:2,phase2_runs:2,stroke_samples:vec![1,2,5],..Default::default()};
        let pos=crate::vsdcal::cosine_positions(11,s.phase1_max_deg);
        let mut c=VsdCalibration{pump:pump.into(),head:"LH1".into(),settings:s.clone(),date_ms:date,simulation:true,spr:51200,..Default::default()};
        c.pd_runs=vec![pos.iter().map(|p|g*(2.*p).min(1.)).collect();2];c.stroke_runs=vec![s.stroke_samples.iter().map(|k|*k as f64*g).collect();2];c.finish();c
    }
    #[test]fn k2_second_calibration_archives_first_and_aborted_never_active(){
        let dir=temp("arch");let mut db=Db::open(&dir.join("p.db")).unwrap();
        let a=db.save_calibration(&NewCal::from_vsd(&sample("P_C01",0.5,1000),true,"")).unwrap();
        assert_eq!(db.active_calibration(ProfileKind::Vsd,"P_C01",KIND_VSD).unwrap().unwrap().id,a);
        let pts=db.calibration_points(a).unwrap();
        assert_eq!(pts.iter().filter(|p|p.phase==1 && p.run==0).count(),11);assert_eq!(pts.iter().filter(|p|p.phase==2 && p.run==2).count(),3);
        assert_eq!(pts.iter().find(|p|p.phase==2 && p.run==0 && p.idx==2).unwrap().position_steps,5*51200);
        let b=db.save_calibration(&NewCal::from_vsd(&sample("P_C01",0.52,2000),true,"")).unwrap();
        let h=db.calibrations(ProfileKind::Vsd,"P_C01").unwrap();assert_eq!(h.iter().map(|r|(r.id,r.status.as_str())).collect::<Vec<_>>(),vec![(b,ACTIVE),(a,ARCHIVE)]);
        assert!((h[0].g_per_stroke.unwrap()-0.52).abs()<1e-9 && (h[0].g_per_rev.unwrap()-0.52).abs()<1e-9);
        // Stopped run: stored, not active, active unchanged.
        let c=db.save_calibration(&NewCal::from_vsd(&sample("P_C01",0.4,3000),false,"Gestopt")).unwrap();
        assert_eq!(db.calibration(c).unwrap().status,ABORTED);assert_eq!(db.active_calibration(ProfileKind::Vsd,"P_C01",KIND_VSD).unwrap().unwrap().id,b);
        assert_eq!(db.vsd_calibration(b).unwrap().pd.len(),11);assert!(db.active_dates(ProfileKind::Vsd).unwrap().contains_key("P_C01"));
        assert!(db.set_check(b,true,1.,Some(1.01),Some(1.)).unwrap());assert_eq!(db.calibration(b).unwrap().check_measured,Some(1.01));
        // A check in the other mode is never written onto this calibration.
        assert!(!db.set_check(b,false,2.,Some(2.),Some(0.)).unwrap());assert_eq!(db.calibration(b).unwrap().check_requested,Some(1.));
        drop(db);cleanup(&dir);
    }
    #[test]fn v1_database_with_data_upgrades_to_v2(){
        let dir=temp("v1");let path=dir.join("p.db");
        {let db=Db::open(&path).unwrap();let vsd=db.profile(ProfileKind::Vsd).unwrap().id;
            let mut r=db.pumps(vsd).unwrap()[3].clone();r.ingredient="Blauw".into();db.update_pump(&r).unwrap();
            db.set_pump_position(r.id,Some(0.25)).unwrap();
            // Machine set to 25 600 steps per revolution: 0.25 rev becomes 6 400 steps, not 12 800.
            let mut m=db.machine_settings().unwrap();m.spr=25600;db.set_setting(ProfileKind::Vsd,"machine",&m).unwrap();
            let q=db.pumps(vsd).unwrap()[4].id;db.conn.execute("UPDATE pump SET steps_rev=10000,position=0.5 WHERE id=?1",[q]).unwrap();
            // Back to version 1: drop the v2 tables, as a database of iteration 1 has them not.
            db.conn.execute_batch("DROP TABLE prod_event;DROP TABLE line_state;DROP TABLE prod_order;DROP TABLE pot_item;DROP TABLE pot_run;DROP TABLE recipe_item;DROP TABLE recipe_step;DROP TABLE recipe_version;DROP TABLE recipe;DROP TRIGGER pump_ing_ins;DROP TRIGGER pump_ing_upd;DROP TRIGGER pump_ing_del;DROP TABLE ingredient_pump;ALTER TABLE ingredient DROP COLUMN tolerance_pct;ALTER TABLE ingredient DROP COLUMN head_pref;DROP TABLE test_dose;DROP TABLE test_run;DROP TABLE calibration_point;DROP TABLE calibration;DELETE FROM schema_version;INSERT INTO schema_version(version) VALUES(1);").unwrap();}
        let mut db=Db::open(&path).unwrap();assert!(!db.created);assert_eq!(db.schema_version().unwrap(),crate::db::SCHEMA_VERSION);
        let vsd=db.profile(ProfileKind::Vsd).unwrap().id;assert_eq!(db.pumps(vsd).unwrap()[3].ingredient,"Blauw");assert_eq!(db.pumps(vsd).unwrap()[3].position,Some(6400.),"positie omw -> stappen met ingestelde stappen/omw");
        assert_eq!(db.pumps(vsd).unwrap()[4].position,Some(5000.),"eigen stappen/omw van de pomp");assert_eq!(db.pumps(vsd).unwrap().len(),192);
        db.save_calibration(&NewCal::from_vsd(&sample("P_A04",0.5,1),true,"")).unwrap();
        drop(db);cleanup(&dir);
    }
    #[test]fn k6_import_old_files_once(){
        let dir=temp("imp");std::fs::create_dir_all(dir.join("machine/calibration")).unwrap();std::fs::create_dir_all(dir.join("results")).unwrap();
        for (p,g,d) in [("P_A01",0.5,10u128),("P_D07",0.45,20)]{sample(p,g,d).save(&dir.join("machine/calibration")).unwrap();}
        std::fs::write(dir.join("machine/calibration/P_ZZ.json"),serde_json::to_vec(&sample("P_ZZ",0.5,5)).unwrap()).unwrap();
        // Ceramic curve as the old app saved it (engine state with a curve).
        let mut st=State::default();st.started_ms=30;st.simulation=true;
        st.curve=Some(Curve::fit(25600,0,vec![0,12800,25600],&[vec![0.,0.2,1.3],vec![0.,0.21,1.31]]).unwrap());
        std::fs::write(dir.join("results/characterization_30_1.json"),serde_json::to_vec(&st).unwrap()).unwrap();
        std::fs::write(dir.join("results/characterization_40_1.json"),b"{kapot").unwrap();
        let mut db=Db::open(&dir.join("data/p.db")).unwrap();
        let n=db.import_calibrations(&dir).unwrap().unwrap();assert!(n.contains("2 VSD, 1 keramische"),"{n}");
        assert!(db.active_calibration(ProfileKind::Vsd,"P_D07",KIND_VSD).unwrap().is_some());
        let k=db.active_calibration(ProfileKind::Keramisch,"Keramisch 1",KIND_BENCH_CURVE).unwrap().unwrap();assert!((k.g_per_rev.unwrap()-1.305).abs()<1e-9);
        assert!(db.bench_curve(k.id).unwrap().is_some());
        // Idempotent: second run imports nothing, also after clearing the flag (sources already known).
        assert!(db.import_calibrations(&dir).unwrap().is_none());
        db.conn.execute("DELETE FROM meta WHERE key='import_calibrations'",[]).unwrap();assert!(db.import_calibrations(&dir).unwrap().is_none());
        let total:i64=db.conn.query_row("SELECT count(*) FROM calibration",[],|r|r.get(0)).unwrap();assert_eq!(total,3);
        assert!(dir.join("machine/calibration/P_A01.json").exists() && dir.join("results/characterization_30_1.json").exists());
        drop(db);cleanup(&dir);
    }
    #[test]fn s1_simulation_never_replaces_hardware_and_dosing_uses_the_right_mode(){
        let dir=temp("s1");let mut db=Db::open(&dir.join("p.db")).unwrap();
        let mut hw=sample("P_C01",0.41,1000);hw.simulation=false;
        let h=db.save_calibration(&NewCal::from_vsd(&hw,true,"")).unwrap();
        let sim=db.save_calibration(&NewCal::from_vsd(&sample("P_C01",0.5,2000),true,"")).unwrap();
        assert_eq!(db.calibration(h).unwrap().status,ACTIVE,"hardware blijft actief");assert_eq!(db.calibration(sim).unwrap().status,ACTIVE);
        assert_eq!(db.calibration_for_dosing(ProfileKind::Vsd,"P_C01",KIND_VSD,true).unwrap().id,h);
        assert_eq!(db.calibration_for_dosing(ProfileKind::Vsd,"P_C01",KIND_VSD,false).unwrap().id,sim);
        // A second hardware calibration archives only the first hardware one.
        let mut hw2=sample("P_C01",0.42,3000);hw2.simulation=false;let h2=db.save_calibration(&NewCal::from_vsd(&hw2,true,"")).unwrap();
        assert_eq!((db.calibration(h).unwrap().status.as_str(),db.calibration(sim).unwrap().status.as_str(),db.calibration(h2).unwrap().status.as_str()),(ARCHIVE,ACTIVE,ACTIVE));
        assert_eq!(db.active_dates(ProfileKind::Vsd).unwrap()["P_C01"],(3000,false),"Pompen toont de hardware-kalibratie");
        // Only a simulation calibration: hardware refuses, simulation uses it.
        db.save_calibration(&NewCal::from_vsd(&sample("P_C02",0.5,10),true,"")).unwrap();
        let e=db.calibration_for_dosing(ProfileKind::Vsd,"P_C02",KIND_VSD,true).unwrap_err();assert!(e.contains("Geen hardware-kalibratie"),"{e}");
        assert!(db.calibration_for_dosing(ProfileKind::Vsd,"P_C02",KIND_VSD,false).is_ok());assert_eq!(db.active_dates(ProfileKind::Vsd).unwrap()["P_C02"],(10,true));
        // Only hardware: simulation borrows it.
        let mut hw3=sample("P_C03",0.4,10);hw3.simulation=false;let h3=db.save_calibration(&NewCal::from_vsd(&hw3,true,"")).unwrap();
        assert_eq!(db.calibration_for_dosing(ProfileKind::Vsd,"P_C03",KIND_VSD,false).unwrap().id,h3);
        assert!(db.calibration_for_dosing(ProfileKind::Vsd,"P_C04",KIND_VSD,false).is_err());
        drop(db);cleanup(&dir);
    }
}
