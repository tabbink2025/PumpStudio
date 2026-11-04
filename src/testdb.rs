//! Test runs in the database (schema v3): `test_run` (pump, mode sim/hw, calibration used, plan with
//! seed, tolerance, status, start/end, note) and `test_dose` (nr, requested, dosed, correction, target,
//! steps, pump-settings snapshot, duration, status, excluded + reason). Every dose is written at once.
use crate::{db::{Db,ProfileKind},pumptest::{DoseRow,Mode,TestPlan},model::Result,workflow::State};
use rusqlite::{params,OptionalExtension};
use std::path::Path;

pub const RUN_RUNNING:&str="loopt";pub const RUN_DONE:&str="klaar";pub const RUN_STOPPED:&str="gestopt";
pub const RUN_FAILED:&str="fout";pub const RUN_INTERRUPTED:&str="onderbroken";

#[derive(Clone,Debug,PartialEq)]
pub struct RunRow{pub id:i64,pub profile_id:i64,pub pump_id:Option<i64>,pub pump:String,pub simulation:bool,pub calibration_id:Option<i64>,
    pub mode:Mode,pub plan:String,pub tolerance_pct:f64,pub status:String,pub started_ms:i64,pub ended_ms:Option<i64>,pub note:String,pub source:String}
impl RunRow{
    pub fn plan(&self)->Option<TestPlan>{serde_json::from_str(&self.plan).ok()}
    pub fn mode_text(&self)->&'static str{if self.simulation{"simulatie"}else{"hardware"}}
}
fn e(x:rusqlite::Error)->String{format!("Database: {x}")}
fn now_ms()->i64{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis() as i64).unwrap_or(0)}
const RCOLS:&str="id,profile_id,pump_id,pump_name,simulation,calibration_id,mode,plan,tolerance_pct,status,started_ms,ended_ms,note,source";
const DCOLS:&str="id,run_id,nr,requested_g,dosed_g,correction_pct,target_g,steps,settings,duration_s,status,error,excluded,exclude_reason,time_ms";
fn run_row(r:&rusqlite::Row)->rusqlite::Result<RunRow>{
    Ok(RunRow{id:r.get(0)?,profile_id:r.get(1)?,pump_id:r.get(2)?,pump:r.get(3)?,simulation:r.get::<_,i64>(4)?!=0,calibration_id:r.get(5)?,
        mode:Mode::from_key(&r.get::<_,String>(6)?),plan:r.get(7)?,tolerance_pct:r.get(8)?,status:r.get(9)?,started_ms:r.get(10)?,ended_ms:r.get(11)?,note:r.get(12)?,source:r.get(13)?})
}
fn dose_row(r:&rusqlite::Row)->rusqlite::Result<DoseRow>{
    Ok(DoseRow{id:r.get(0)?,run_id:r.get(1)?,nr:r.get(2)?,requested_g:r.get(3)?,dosed_g:r.get(4)?,correction_pct:r.get(5)?,target_g:r.get(6)?,steps:r.get(7)?,
        settings:r.get(8)?,duration_s:r.get(9)?,status:r.get(10)?,error:r.get(11)?,excluded:r.get::<_,i64>(12)?!=0,exclude_reason:r.get(13)?,time_ms:r.get(14)?})
}

impl Db{
    pub fn create_test_run(&mut self,profile:ProfileKind,pump:&str,simulation:bool,calibration_id:Option<i64>,plan:&TestPlan,note:&str)->Result<i64>{
        let pid=self.profile(profile)?.id;
        let tx=self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
        let pump_id:Option<i64>=tx.query_row("SELECT id FROM pump WHERE profile_id=?1 AND name=?2",params![pid,pump],|r|r.get(0)).optional().map_err(e)?;
        tx.execute("INSERT INTO test_run(profile_id,pump_id,pump_name,simulation,calibration_id,mode,plan,tolerance_pct,status,started_ms,note) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![pid,pump_id,pump,simulation as i64,calibration_id,plan.mode.key(),serde_json::to_string(plan).unwrap_or_default(),plan.tolerance_pct,RUN_RUNNING,now_ms(),note]).map_err(e)?;
        let id=tx.last_insert_rowid();tx.commit().map_err(e)?;Ok(id)
    }
    pub fn add_test_dose(&mut self,d:&DoseRow)->Result<i64>{
        self.conn.execute(&format!("INSERT INTO test_dose({}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",&DCOLS[3..]),
            params![d.run_id,d.nr,d.requested_g,d.dosed_g,d.correction_pct,d.target_g,d.steps,d.settings,d.duration_s,d.status,d.error,d.excluded as i64,d.exclude_reason,if d.time_ms>0{d.time_ms}else{now_ms()}]).map_err(e)?;
        Ok(self.conn.last_insert_rowid())
    }
    pub fn finish_test_run(&self,id:i64,status:&str,note:&str)->Result<()>{
        self.conn.execute("UPDATE test_run SET status=?2,ended_ms=?3,note=CASE WHEN ?4='' THEN note ELSE ?4 END WHERE id=?1",params![id,status,now_ms(),note]).map(|_|()).map_err(e)
    }
    pub fn test_runs(&self,profile:ProfileKind,pump:&str)->Result<Vec<RunRow>>{
        let pid=self.profile(profile)?.id;
        let mut st=self.conn.prepare(&format!("SELECT {RCOLS} FROM test_run WHERE profile_id=?1 AND pump_name=?2 ORDER BY started_ms DESC,id DESC")).map_err(e)?;
        let rows=st.query_map(params![pid,pump],run_row).map_err(e)?;rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    pub fn test_run(&self,id:i64)->Result<RunRow>{self.conn.query_row(&format!("SELECT {RCOLS} FROM test_run WHERE id=?1"),[id],run_row).map_err(e)}
    pub fn test_doses(&self,run:i64)->Result<Vec<DoseRow>>{
        let mut st=self.conn.prepare(&format!("SELECT {DCOLS} FROM test_dose WHERE run_id=?1 ORDER BY nr,id")).map_err(e)?;
        let rows=st.query_map([run],dose_row).map_err(e)?;rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    /// Exclude a dose from the statistics (reason mandatory) or include it again. The measured value is never changed.
    pub fn set_excluded(&self,dose:i64,excluded:bool,reason:&str)->Result<()>{
        let reason=reason.trim();
        if excluded && reason.is_empty(){return Err("Reden verplicht bij uitsluiten".into());}
        let n=self.conn.execute("UPDATE test_dose SET excluded=?2,exclude_reason=?3 WHERE id=?1",params![dose,excluded as i64,if excluded{reason}else{""}]).map_err(e)?;
        if n!=1{return Err(format!("Dosis {dose} niet gevonden"));}
        crate::applog::info("POMPTEST",&format!("Dosis {dose} {}{}",if excluded{"uitgesloten: "}else{"weer meegeteld"},reason));Ok(())
    }
    /// Runs still "loopt" after a restart were interrupted (crash, power): mark them, keep their doses.
    pub fn close_stale_runs(&self)->Result<usize>{
        self.conn.execute("UPDATE test_run SET status=?1,note=CASE WHEN note='' THEN 'App afgesloten tijdens test' ELSE note END WHERE status=?2",params![RUN_INTERRUPTED,RUN_RUNNING]).map_err(e)
    }
    /// One-time import of the old app's dose runs (`results/dosering_*.json`, `results/doseertest_*.json`;
    /// files stay). Runs once (meta flag) and skips files already imported. Unreadable files are skipped and logged.
    pub fn import_tests(&mut self,root:&Path)->Result<Option<String>>{
        if self.meta("import_tests")?.is_some(){return Ok(None);}
        let mut files:Vec<std::path::PathBuf>=std::fs::read_dir(root.join("results")).into_iter().flatten().flatten().map(|d|d.path())
            .filter(|p|p.extension().is_some_and(|x|x=="json") && p.file_name().is_some_and(|n|{let n=n.to_string_lossy();n.starts_with("dosering_") || n.starts_with("doseertest_")})).collect();
        files.sort();
        let pump=self.pumps(self.profile(ProfileKind::Keramisch)?.id)?.into_iter().next().map(|p|p.name).unwrap_or("Keramisch 1".into());
        let (mut n,mut skipped)=(0,vec![]);
        for f in files{
            let src=f.display().to_string();
            let seen:Option<i64>=self.conn.query_row("SELECT id FROM test_run WHERE source=?1",[&src],|r|r.get(0)).optional().map_err(e)?;
            if seen.is_some(){continue;}
            let Some(st)=std::fs::read(&f).ok().and_then(|b|serde_json::from_slice::<State>(&b).ok()) else{skipped.push(src);continue};
            let recs:Vec<_>=st.records.iter().filter(|r|r.kind=="Dosering" && r.requested_g.is_some()).collect();
            if recs.is_empty(){skipped.push(src);continue;}
            let mut weights:Vec<f64>=recs.iter().filter_map(|r|r.requested_g).collect();weights.sort_by(f64::total_cmp);weights.dedup();
            let plan=TestPlan{mode:if weights.len()==1{Mode::Enkel}else{Mode::Reeks},weights:weights.clone(),repeats:recs.len()/weights.len().max(1),
                settle_s:st.config.settle_s,..Default::default()};
            let complete=recs.iter().all(|r|r.status=="completed");
            let snapshot=serde_json::json!({"pomptype":st.config.pump_type,"stappen_omw":st.config.steps_rev,"snelheid":st.config.speed,"bron":"oude app"}).to_string();
            let pid=self.profile(ProfileKind::Keramisch)?.id;
            let tx=self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
            let pump_id:Option<i64>=tx.query_row("SELECT id FROM pump WHERE profile_id=?1 AND name=?2",params![pid,pump],|r|r.get(0)).optional().map_err(e)?;
            tx.execute("INSERT INTO test_run(profile_id,pump_id,pump_name,simulation,calibration_id,mode,plan,tolerance_pct,status,started_ms,ended_ms,note,source) VALUES(?1,?2,?3,?4,NULL,?5,?6,1,?7,?8,?8,'Geïmporteerd uit oude app',?9)",
                params![pid,pump_id,pump,st.simulation as i64,plan.mode.key(),serde_json::to_string(&plan).unwrap_or_default(),if complete{RUN_DONE}else{RUN_STOPPED},st.started_ms as i64,src]).map_err(e)?;
            let run=tx.last_insert_rowid();
            for (k,r) in recs.iter().enumerate(){
                let ok=r.status=="completed" && r.mass_g.is_some();
                tx.execute(&format!("INSERT INTO test_dose({}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL,?9,?10,0,'',?11)",&DCOLS[3..]),
                    params![run,(k+1) as i64,r.requested_g,if ok{r.mass_g}else{None},r.correction_pct.unwrap_or(0.),r.curve_target_g,r.target_steps-r.start_steps,snapshot,if ok{"ok"}else{"fout"},r.error,st.started_ms as i64]).map_err(e)?;
            }
            tx.commit().map_err(e)?;n+=1;
        }
        self.set_meta("import_tests",&format!("{n}"))?;
        if !skipped.is_empty(){crate::applog::warn("DATABASE",&format!("Doseerbestanden niet geïmporteerd (onleesbaar of zonder doses): {}",skipped.join(", ")));}
        Ok(if n==0{None}else{Some(format!("{n} oude doseertest(en) geïmporteerd"))})
    }
}

#[cfg(test)] mod tests{
    use super::*;
    use crate::workflow::Record;
    fn temp(n:&str)->std::path::PathBuf{let d=std::env::temp_dir().join(format!("pumpstudio-db-test-{n}-{}-{}",std::process::id(),now_ms()));std::fs::create_dir_all(&d).unwrap();d}
    fn cleanup(d:&Path){for _ in 0..100{if std::fs::remove_dir_all(d).is_ok() || !d.exists(){return;}std::thread::sleep(std::time::Duration::from_millis(20));}}
    #[test]fn t6_runs_doses_and_exclusion_with_reason(){
        let dir=temp("run");let mut db=Db::open(&dir.join("p.db")).unwrap();
        let plan=TestPlan::default();let cal=db.save_calibration(&crate::calib::NewCal::from_vsd(&crate::calib::tests_sample("P_C01",0.5),true,"")).unwrap();let run=db.create_test_run(ProfileKind::Vsd,"P_C01",true,Some(cal),&plan,"").unwrap();
        for (k,g) in [0.5,1.,5.].iter().enumerate(){db.add_test_dose(&DoseRow{run_id:run,nr:k as i64+1,requested_g:*g,dosed_g:Some(g*1.01),correction_pct:0.3,status:"ok".into(),settings:r#"{"snelheid":1}"#.into(),..Default::default()}).unwrap();}
        db.finish_test_run(run,RUN_DONE,"").unwrap();
        let r=db.test_run(run).unwrap();assert_eq!((r.status.as_str(),r.simulation,r.calibration_id,r.mode),(RUN_DONE,true,Some(cal),Mode::Reeks));assert!(r.ended_ms.is_some());assert_eq!(r.plan(),Some(plan.clone()));
        let d=db.test_doses(run).unwrap();assert_eq!(d.len(),3);assert_eq!(d[2].correction_pct,0.3);assert!((d[2].dosed_g.unwrap()-5.05).abs()<1e-12);
        assert!(db.set_excluded(d[0].id,true,"  ").unwrap_err().contains("Reden verplicht"));
        db.set_excluded(d[0].id,true,"luchtbel").unwrap();let d2=db.test_doses(run).unwrap();
        assert!(d2[0].excluded && d2[0].exclude_reason=="luchtbel" && d2[0].dosed_g==d[0].dosed_g,"meetwaarde onveranderd");
        db.set_excluded(d[0].id,false,"").unwrap();assert!(!db.test_doses(run).unwrap()[0].excluded);
        assert_eq!(db.test_runs(ProfileKind::Vsd,"P_C01").unwrap().len(),1);
        // A run left "loopt" by a crash is marked as interrupted on the next start.
        let r2=db.create_test_run(ProfileKind::Vsd,"P_C01",false,None,&plan,"").unwrap();assert_eq!(db.close_stale_runs().unwrap(),1);assert_eq!(db.test_run(r2).unwrap().status,RUN_INTERRUPTED);
        drop(db);cleanup(&dir);
    }
    #[test]fn t8_old_dose_files_imported_once(){
        let dir=temp("imp");std::fs::create_dir_all(dir.join("results")).unwrap();
        let mut st=State{started_ms:1_793_202_769_614,simulation:false,..Default::default()};
        for (i,(g,m)) in [(4.,4.14),(4.,3.98),(5.,5.01)].iter().enumerate(){st.records.push(Record{kind:"Dosering".into(),repeat:i+1,status:"completed".into(),requested_g:Some(*g),mass_g:Some(*m),correction_pct:Some(-1.),start_steps:0,target_steps:1000,..Default::default()});}
        st.records.push(Record{kind:"Dosering".into(),repeat:4,status:"failed".into(),requested_g:Some(5.),error:"Gestopt".into(),..Default::default()});
        std::fs::write(dir.join("results/dosering_1_1.json"),serde_json::to_vec(&st).unwrap()).unwrap();
        std::fs::write(dir.join("results/doseertest_2_1.json"),b"{kapot").unwrap();
        let mut db=Db::open(&dir.join("data/p.db")).unwrap();
        assert!(db.import_tests(&dir).unwrap().unwrap().contains("1 oude"));
        let runs=db.test_runs(ProfileKind::Keramisch,"Keramisch 1").unwrap();assert_eq!(runs.len(),1);
        let r=&runs[0];assert_eq!((r.status.as_str(),r.simulation,r.mode),(RUN_STOPPED,false,Mode::Reeks));
        let d=db.test_doses(r.id).unwrap();assert_eq!(d.len(),4);assert_eq!((d[0].dosed_g,d[0].correction_pct,d[0].steps),(Some(4.14),-1.,Some(1000)));
        assert_eq!((d[3].dosed_g,d[3].status.as_str(),d[3].error.as_str()),(None,"fout","Gestopt"));
        assert!(db.import_tests(&dir).unwrap().is_none());
        db.conn.execute("DELETE FROM meta WHERE key='import_tests'",[]).unwrap();assert!(db.import_tests(&dir).unwrap().is_none(),"bron al bekend");
        assert_eq!(db.test_runs(ProfileKind::Keramisch,"Keramisch 1").unwrap().len(),1);assert!(dir.join("results/dosering_1_1.json").exists());
        drop(db);cleanup(&dir);
    }
}
