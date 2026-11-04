//! Production (Productieproces, PLAN.md): orders → pots with unique barcodes in a queue, and the line cycle after
//! Start, as Camelot `UnitTrainSync` but with time-outs and without forgetting pots:
//! line 1310 Start → loop { dose (LH1 and LH2 at the same time, each its own pot, `potrun::run_heads`) →
//! supply (scan the pot on the scan spot, PLC 1190 + scanner) → transport 1220 one spot (only with both lifts
//! down) } → line empty and (queue empty or Stop) → line 1310 Stop.
//! Pause / Stop (run out) / Afbreken / Hold on error with Herstart. The line model and every pot state are written
//! to the database on every change, so after an app restart the line is known (no automatic resume).
//! Hardware goes through `LineIo` (devices in the app, a fake in the tests).
use crate::{db::{Db,ProfileKind},machine::{Head,height_p1},model::Result,plc::*,potrun::{self,HeadJob,PotIo,PotOutcome},recipe::{self,PotPlan},scanner::ScanResult};
use rusqlite::{params,OptionalExtension};
use serde::{Deserialize,Serialize};
use std::{collections::HashMap,sync::{Arc,Mutex,atomic::{AtomicBool,Ordering},mpsc},time::Duration};

fn e(x:rusqlite::Error)->String{format!("Database: {x}")}
pub fn now_ms()->i64{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis() as i64).unwrap_or(0)}

// ---------- pot states ----------
pub const QUEUED:&str="wacht";pub const ON_LINE:&str="op lijn";pub const DOSING:&str="doseren";pub const DONE:&str="klaar";pub const FAILED:&str="fout";
pub const NOT_DOSED:&str="niet gedoseerd";pub const ABORTED:&str="afgebroken";pub const INTERRUPTED:&str="onderbroken";pub const CANCELLED:&str="geannuleerd";
/// Head state of a pot that must never be dosed there (aborted, interrupted, not dosed).
pub const SKIP:&str="overgeslagen";
pub const MODE:&str="productie";

/// Positions between the scan spot and LH1, between LH1 and LH2, after LH2 (Instellingen; Camelot: scan → wacht → LH1 → LH2 → uit).
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq)]
#[serde(default)]
pub struct LineCfg{pub to_lh1:usize,pub between:usize,pub after:usize,
    /// No-read or unknown code: Hold and ask the operator (true) or let the pot go along undosed (false).
    pub ask_operator:bool}
impl Default for LineCfg{fn default()->Self{Self{to_lh1:1,between:0,after:0,ask_operator:false}}}
impl LineCfg{
    pub fn validate(&self)->Result<()>{if self.to_lh1>5 || self.between>5 || self.after>5{return Err("Lijn: maximaal 5 tussenplekken".into());}Ok(())}
    pub fn names(&self)->Vec<String>{
        let mut v=vec!["Scan".to_string()];
        for k in 0..self.to_lh1{v.push(format!("Wacht {}",k+1));}
        v.push("LH1".into());for k in 0..self.between{v.push(format!("Tussen {}",k+1));}
        v.push("LH2".into());for k in 0..self.after{v.push(format!("Na LH2 {}",k+1));}v
    }
    pub fn head_spot(&self,h:Head)->usize{match h{Head::LH1=>1+self.to_lh1,Head::LH2=>2+self.to_lh1+self.between}}
}
/// One spot of the line.
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Default)]
pub enum Slot{#[default]Empty,Pot(i64),
    /// A pot without a usable code (no-read, unknown or used code): goes along undosed.
    Unknown(String)}

// ---------- database (schema 5) ----------
#[derive(Clone,Debug,PartialEq)]
pub struct OrderRow{pub id:i64,pub name:String,pub batch:String,pub priority:i64,pub count:i64,pub version_id:i64,pub recipe:String,pub version:i64,pub created_ms:i64,pub status:String,
    /// Pots per state.
    pub waiting:i64,pub done:i64,pub other:i64}
#[derive(Clone,Debug,PartialEq)]
pub struct PotRow{pub id:i64,pub barcode:String,pub order_id:Option<i64>,pub order:String,pub version_id:i64,pub recipe:String,pub version:i64,pub priority:i64,
    pub queue_idx:i64,pub nr:i64,pub count:i64,pub status:String,pub spot:Option<String>,pub heads:HashMap<String,String>,pub simulation:bool,pub started_ms:i64,pub ended_ms:Option<i64>,pub note:String}
#[derive(Clone,Debug,Default)]
pub struct HistFilter{pub from_ms:Option<i64>,pub to_ms:Option<i64>,pub order:Option<i64>,pub recipe:Option<i64>,pub status:Option<String>,pub text:String}
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Default)]
pub struct LineState{pub state:String,pub slots:Vec<Slot>,pub message:String,pub updated_ms:i64}

const POTQ:&str="SELECT p.id,coalesce(p.barcode,''),p.order_id,coalesce(o.name,''),p.recipe_version_id,r.name,v.version,coalesce(o.priority,0),coalesce(p.queue_idx,0),coalesce(p.pot_nr,0),coalesce(o.count,1),
    p.status,p.spot,coalesce(p.head_state,''),p.simulation,p.started_ms,p.ended_ms,p.note
    FROM pot_run p JOIN recipe_version v ON v.id=p.recipe_version_id JOIN recipe r ON r.id=v.recipe_id LEFT JOIN prod_order o ON o.id=p.order_id";
fn potrow(r:&rusqlite::Row)->rusqlite::Result<PotRow>{
    let hs:String=r.get(13)?;
    Ok(PotRow{id:r.get(0)?,barcode:r.get(1)?,order_id:r.get(2)?,order:r.get(3)?,version_id:r.get(4)?,recipe:r.get(5)?,version:r.get(6)?,priority:r.get(7)?,queue_idx:r.get(8)?,nr:r.get(9)?,count:r.get(10)?,
        status:r.get(11)?,spot:r.get(12)?,heads:serde_json::from_str(&hs).unwrap_or_default(),simulation:r.get::<_,i64>(14)?!=0,started_ms:r.get(15)?,ended_ms:r.get(16)?,note:r.get(17)?})
}
pub fn barcode_of(id:i64)->String{format!("{id:010}")}

impl Db{
    /// Order = recipe version × count pots; every pot gets a record with barcode = 10-digit pot id.
    pub fn create_order(&mut self,profile:ProfileKind,version_id:i64,name:&str,batch:&str,priority:i64,count:i64)->Result<(i64,Vec<i64>)>{
        if !(1..=500).contains(&count){return Err("Aantal potten 1 tot 500".into());}
        if !(-9..=9).contains(&priority){return Err("Prioriteit −9 tot 9".into());}
        let pid=self.profile(profile)?.id;
        let tx=self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
        let _:i64=tx.query_row("SELECT id FROM recipe_version WHERE id=?1",[version_id],|r|r.get(0)).map_err(|_|format!("Receptversie {version_id} bestaat niet"))?;
        let now=now_ms();
        tx.execute("INSERT INTO prod_order(profile_id,recipe_version_id,name,batch,priority,count,created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![pid,version_id,name.trim(),batch.trim(),priority,count,now]).map_err(e)?;
        let oid=tx.last_insert_rowid();
        let mut q:i64=tx.query_row("SELECT coalesce(max(queue_idx),0) FROM pot_run",[],|r|r.get(0)).map_err(e)?;
        let mut pots=vec![];
        for k in 1..=count{
            q+=1;
            tx.execute("INSERT INTO pot_run(profile_id,recipe_version_id,simulation,mode,status,started_ms,order_id,queue_idx,pot_nr) VALUES(?1,?2,0,?3,?4,?5,?6,?7,?8)",
                params![pid,version_id,MODE,QUEUED,now,oid,q,k]).map_err(e)?;
            let id=tx.last_insert_rowid();
            tx.execute("UPDATE pot_run SET barcode=?2 WHERE id=?1",params![id,barcode_of(id)]).map_err(|x|format!("Barcode {}: {}",barcode_of(id),e(x)))?;
            pots.push(id);
        }
        tx.commit().map_err(e)?;
        crate::applog::info("PRODUCTIE",&format!("Order {oid} '{}' aangemaakt: {count} pot(ten), receptversie {version_id}, prioriteit {priority}",name.trim()));
        Ok((oid,pots))
    }
    pub fn orders(&self,profile:ProfileKind)->Result<Vec<OrderRow>>{
        let pid=self.profile(profile)?.id;
        let mut st=self.conn.prepare("SELECT o.id,o.name,o.batch,o.priority,o.count,o.recipe_version_id,r.name,v.version,o.created_ms,o.status,
              (SELECT count(*) FROM pot_run p WHERE p.order_id=o.id AND p.status='wacht'),(SELECT count(*) FROM pot_run p WHERE p.order_id=o.id AND p.status='klaar'),
              (SELECT count(*) FROM pot_run p WHERE p.order_id=o.id AND p.status NOT IN ('wacht','klaar'))
            FROM prod_order o JOIN recipe_version v ON v.id=o.recipe_version_id JOIN recipe r ON r.id=v.recipe_id WHERE o.profile_id=?1 ORDER BY o.id DESC").map_err(e)?;
        let rows=st.query_map([pid],|r|Ok(OrderRow{id:r.get(0)?,name:r.get(1)?,batch:r.get(2)?,priority:r.get(3)?,count:r.get(4)?,version_id:r.get(5)?,recipe:r.get(6)?,version:r.get(7)?,created_ms:r.get(8)?,status:r.get(9)?,waiting:r.get(10)?,done:r.get(11)?,other:r.get(12)?})).map_err(e)?;
        rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    /// Waiting pots in the order they will be used: priority high first, then queue position.
    pub fn queue(&self,profile:ProfileKind)->Result<Vec<PotRow>>{
        let pid=self.profile(profile)?.id;
        let mut st=self.conn.prepare(&format!("{POTQ} WHERE p.profile_id=?1 AND p.mode='productie' AND p.status='wacht' ORDER BY coalesce(o.priority,0) DESC,p.queue_idx")).map_err(e)?;
        let rows=st.query_map([pid],potrow).map_err(e)?;rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    pub fn prod_pot(&self,id:i64)->Result<PotRow>{self.conn.query_row(&format!("{POTQ} WHERE p.id=?1"),[id],potrow).map_err(e)}
    pub fn pot_by_barcode(&self,code:&str)->Result<Option<PotRow>>{self.conn.query_row(&format!("{POTQ} WHERE p.barcode=?1"),[code.trim()],potrow).optional().map_err(e)}
    pub fn set_order_priority(&self,order:i64,priority:i64)->Result<()>{
        if !(-9..=9).contains(&priority){return Err("Prioriteit −9 tot 9".into());}
        self.conn.execute("UPDATE prod_order SET priority=?2 WHERE id=?1",params![order,priority]).map(|_|()).map_err(e)
    }
    /// Move a waiting pot one place up (-1) or down (+1) within the queue (swaps with its neighbour of equal priority).
    pub fn move_pot(&mut self,profile:ProfileKind,pot:i64,dir:i64)->Result<()>{
        let q=self.queue(profile)?;let i=q.iter().position(|p|p.id==pot).ok_or("Pot staat niet in de wachtrij")?;
        let j=i as i64+dir;if j<0 || j>=q.len() as i64{return Ok(());}let j=j as usize;
        if q[i].priority!=q[j].priority{return Err("Andere prioriteit: pas de prioriteit van de order aan".into());}
        let tx=self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
        tx.execute("UPDATE pot_run SET queue_idx=?2 WHERE id=?1",params![q[i].id,q[j].queue_idx]).map_err(e)?;
        tx.execute("UPDATE pot_run SET queue_idx=?2 WHERE id=?1",params![q[j].id,q[i].queue_idx]).map_err(e)?;
        tx.commit().map_err(e)
    }
    /// Cancel the waiting pots of an order (pots on the line or done stay as they are).
    pub fn cancel_order(&self,order:i64)->Result<usize>{
        let n=self.conn.execute("UPDATE pot_run SET status=?2,ended_ms=?3,note='order geannuleerd' WHERE order_id=?1 AND status='wacht'",params![order,CANCELLED,now_ms()]).map_err(e)?;
        self.conn.execute("UPDATE prod_order SET status='geannuleerd' WHERE id=?1",[order]).map_err(e)?;Ok(n)
    }
    /// Manual code (pre-printed label): only for a waiting pot, unique over the whole database.
    pub fn set_pot_barcode(&self,pot:i64,code:&str)->Result<()>{
        let code=code.trim();
        if code.is_empty() || code.len()>40 || code.chars().any(|c|c.is_control()){return Err("Code: 1 tot 40 tekens, geen stuurtekens".into());}
        let p=self.prod_pot(pot)?;if p.status!=QUEUED{return Err(format!("Pot {pot} staat niet meer in de wachtrij ({}): code niet te wijzigen",p.status));}
        if let Some(o)=self.pot_by_barcode(code)?{if o.id!=pot{return Err(format!("Code {code} hoort al bij pot {} ({}): codes zijn uniek",o.id,o.status));}}
        self.conn.execute("UPDATE pot_run SET barcode=?2 WHERE id=?1",params![pot,code]).map(|_|()).map_err(e)
    }
    pub fn set_pot_state(&self,pot:i64,status:&str,spot:Option<&str>,heads:&HashMap<String,String>,note:&str)->Result<()>{
        let ended=if matches!(status,DONE|FAILED|NOT_DOSED|ABORTED|INTERRUPTED|CANCELLED) && spot.is_none(){Some(now_ms())}else{None};
        self.conn.execute("UPDATE pot_run SET status=?2,spot=?3,head_state=?4,note=CASE WHEN ?5='' THEN note ELSE ?5 END,ended_ms=coalesce(?6,ended_ms) WHERE id=?1",
            params![pot,status,spot,serde_json::to_string(heads).unwrap_or_default(),note,ended]).map(|_|()).map_err(e)
    }
    pub fn set_pot_started(&self,pot:i64,simulation:bool)->Result<()>{
        self.conn.execute("UPDATE pot_run SET simulation=?2,started_ms=?3 WHERE id=?1",params![pot,simulation as i64,now_ms()]).map(|_|()).map_err(e)
    }
    pub fn add_event(&self,pot:Option<i64>,kind:&str,text:&str)->Result<()>{
        self.conn.execute("INSERT INTO prod_event(ms,pot_run_id,kind,text) VALUES(?1,?2,?3,?4)",params![now_ms(),pot,kind,text]).map(|_|()).map_err(e)
    }
    /// Events (time, pot, kind, text), newest first; `pot` = only that pot.
    pub fn events(&self,pot:Option<i64>,limit:usize)->Result<Vec<(i64,Option<i64>,String,String)>>{
        let mut st=self.conn.prepare("SELECT ms,pot_run_id,kind,text FROM prod_event WHERE ?1 IS NULL OR pot_run_id=?1 ORDER BY id DESC LIMIT ?2").map_err(e)?;
        let rows=st.query_map(params![pot,limit as i64],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(e)?;rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    pub fn line_state(&self)->Result<Option<LineState>>{
        self.conn.query_row("SELECT state,slots,message,updated_ms FROM line_state WHERE id=1",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?))).optional().map_err(e)
            .map(|o|o.map(|(state,slots,message,updated_ms)|LineState{state,slots:serde_json::from_str(&slots).unwrap_or_default(),message,updated_ms}))
    }
    pub fn save_line_state(&self,s:&LineState)->Result<()>{
        self.conn.execute("INSERT INTO line_state(id,state,slots,message,updated_ms) VALUES(1,?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET state=?1,slots=?2,message=?3,updated_ms=?4",
            params![s.state,serde_json::to_string(&s.slots).unwrap_or_default(),s.message,now_ms()]).map(|_|()).map_err(e)
    }
    /// After an app restart: a cycle that was running/held/paused is marked interrupted; a pot that was being dosed
    /// becomes "onderbroken" and is never dosed again automatically. The line itself stays as it was.
    pub fn restore_after_restart(&self)->Result<Option<String>>{
        let Some(mut l)=self.line_state()? else{return Ok(None)};
        let mut notes=vec![];
        let mut st=self.conn.prepare(&format!("{POTQ} WHERE p.mode='productie' AND p.status=?1")).map_err(e)?;
        let dosing:Vec<PotRow>=st.query_map([DOSING],potrow).map_err(e)?.collect::<std::result::Result<_,_>>().map_err(e)?;
        for p in dosing{
            let mut h=p.heads.clone();for k in ["LH1","LH2"]{h.entry(k.into()).or_insert_with(||SKIP.into());}
            for v in h.values_mut(){if v==DOSING{*v=INTERRUPTED.into();}}
            self.set_pot_state(p.id,INTERRUPTED,p.spot.as_deref(),&h,"App afgesloten tijdens dosering: niet automatisch opnieuw doseren")?;
            self.add_event(Some(p.id),"herstart",&format!("Pot {} onderbroken bij herstart van de app",p.barcode))?;notes.push(p.barcode);
        }
        if matches!(l.state.as_str(),"loopt"|"pauze"|"stoppen"|"hold"|"afbreken"){
            l.message=format!("Productie was bezig ({}) toen de app sloot. Lijn hersteld: kies Doorgaan met deze lijn of Lijn leegmaken.{}",l.state,
                if notes.is_empty(){String::new()}else{format!(" Onderbroken: {}",notes.join(", "))});
            l.state="onderbroken".into();self.save_line_state(&l)?;return Ok(Some(l.message));
        }
        Ok(None)
    }
    /// Operator emptied the line by hand: pots on it that were not finished get "onderbroken" (dosed so far stays).
    pub fn clear_line(&self)->Result<usize>{
        let Some(mut l)=self.line_state()? else{return Ok(0)};let mut n=0;
        for s in &l.slots{if let Slot::Pot(id)=s{let p=self.prod_pot(*id)?;
            let st=if matches!(p.status.as_str(),ON_LINE|DOSING){INTERRUPTED}else{p.status.as_str()};
            self.set_pot_state(*id,st,None,&p.heads,"lijn handmatig leeggemaakt")?;n+=1;}}
        l.slots=vec![];l.state="leeg".into();l.message="Lijn leeggemaakt".into();self.save_line_state(&l)?;Ok(n)
    }
    /// Production history (pots that left the queue), newest first.
    pub fn history(&self,profile:ProfileKind,f:&HistFilter,limit:usize)->Result<Vec<PotRow>>{
        let pid=self.profile(profile)?.id;
        let mut st=self.conn.prepare(&format!("{POTQ} WHERE p.profile_id=?1 AND p.mode='productie' AND p.status NOT IN ('wacht') AND (?2 IS NULL OR p.started_ms>=?2) AND (?3 IS NULL OR p.started_ms<?3)
            AND (?4 IS NULL OR p.order_id=?4) AND (?5 IS NULL OR r.id=?5) AND (?6 IS NULL OR p.status=?6) AND (?7='' OR p.barcode LIKE '%'||?7||'%' OR o.name LIKE '%'||?7||'%' OR r.name LIKE '%'||?7||'%')
            ORDER BY p.started_ms DESC,p.id DESC LIMIT ?8")).map_err(e)?;
        let rows=st.query_map(params![pid,f.from_ms,f.to_ms,f.order,f.recipe,f.status,f.text.trim(),limit as i64],potrow).map_err(e)?;
        rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
}

// ---------- engine ----------
/// Hardware of the line. Every call has a time-out; `force` = also after STOP (lowering, Abort).
pub trait LineIo:Send{
    /// One PLC command; returns the reply code (any non-update reply of that process to this command).
    fn plc(&mut self,process:u32,cmd:u32,params:&[u32],force:bool)->Result<u32>;
    /// Barcode process Start (P1 = 1) and a scanner read at the same time: (PLC reply, scan result, raw text).
    fn barcode(&mut self,process:u32)->Result<(u32,ScanResult,String)>;
    /// Command without waiting for its reply (Abort, Hold, Restart, as Camelot); also after STOP.
    fn post(&mut self,process:u32,cmd:u32)->Result<()>;
    /// The pot executor of a head (own thread, own CAN block, own weighing).
    fn head(&mut self,h:Head)->Box<dyn PotIo>;
    /// STOP / Afbreken pressed (pumps stopped, waits end).
    fn stopped(&self)->bool;
    fn hardware(&self)->bool;
}
#[derive(Clone,Debug)]
pub struct Processes{pub line:u32,pub transport:u32,pub barcode:u32,pub unit:[u32;2],pub lift:[u32;2]}
#[derive(Clone,Debug)]
pub struct EngineCfg{pub profile:ProfileKind,pub line:LineCfg,pub proc_:Processes,pub heights:[Option<f64>;2],pub retries:u32}
#[derive(Clone,Debug,PartialEq)]
pub enum ProdCmd{Pause,Resume,Stop,Abort,Restart,
    /// Operator typed/scanned the code of the pot on the scan spot (during a Hold for a no-read/unknown code).
    Code(String)}
#[derive(Clone,Debug,Default)]
pub struct ProdStatus{
    pub running:bool,pub state:String,pub hold:Option<String>,pub paused:bool,pub stopping:bool,
    pub names:Vec<String>,pub slots:Vec<Slot>,pub labels:Vec<String>,
    pub messages:Vec<String>,pub version:u64,pub done:usize,pub last_result:Option<String>,
}
pub fn state_label(s:&str)->&str{match s{"loopt"=>"loopt","pauze"=>"pauze","stoppen"=>"loopt leeg (Stop)","hold"=>"HOLD: wacht op operator","afbreken"=>"afbreken…","afgebroken"=>"afgebroken","klaar"=>"klaar","onderbroken"=>"onderbroken (app herstart)","leeg"=>"lijn leeg",x=>x}}

enum Flow{Go,Abort}
/// Head lift state, kept by wrapping each head's `PotIo` (transport only when both are down).
struct Tracked{inner:Box<dyn PotIo>,up:Arc<AtomicBool>}
impl PotIo for Tracked{
    fn lift(&mut self,h:Head,up:bool,p1:u32,force:bool)->Result<()>{
        if up{self.up.store(true,Ordering::SeqCst);}
        let r=self.inner.lift(h,up,p1,force);if !up && r.is_ok(){self.up.store(false,Ordering::SeqCst);}r
    }
    fn unit(&mut self,h:Head,s:bool,f:bool)->Result<()>{self.inner.unit(h,s,f)}
    fn weigh(&mut self,h:Head)->Result<f64>{self.inner.weigh(h)}
    fn dose(&mut self,h:Head,i:&[(String,f64,i64)])->Result<Vec<potrun::BlockDose>>{self.inner.dose(h,i)}
    fn stopped(&self)->bool{self.inner.stopped()}
    fn store(&mut self,i:&recipe::PotItemRow)->Result<()>{self.inner.store(i)}
    fn unit_stop_now(&mut self,h:Head)->Result<()>{self.inner.unit_stop_now(h)}
    fn store_now(&mut self,i:&recipe::PotItemRow)->bool{self.inner.store_now(i)}
    fn wait_operator(&mut self,m:&str)->bool{self.inner.wait_operator(m)}
    fn progress(&mut self,h:Option<Head>,t:&str){self.inner.progress(h,t)}
}

pub struct Engine{
    db:Db,io:Box<dyn LineIo>,cfg:EngineCfg,status:Arc<Mutex<ProdStatus>>,cmds:mpsc::Receiver<ProdCmd>,
    slots:Vec<Slot>,plans:HashMap<i64,std::result::Result<PotPlan,String>>,lift_up:[Arc<AtomicBool>;2],
    pause:bool,stop:bool,abort:bool,code:Option<String>,empty_scans:u32,
    /// Heads whose unit Stop failed at the end of dosing: Herstart repeats it (must answer 9) before anything else.
    unit_retry:[bool;2],
}
impl Engine{
    /// Engine on the persisted line (empty line when none or when the number of spots changed).
    pub fn new(db:Db,io:Box<dyn LineIo>,cfg:EngineCfg,status:Arc<Mutex<ProdStatus>>,cmds:mpsc::Receiver<ProdCmd>)->Result<Self>{
        let n=cfg.line.names().len();
        let mut slots=db.line_state()?.map(|l|l.slots).unwrap_or_default();
        if slots.len()!=n{if slots.iter().any(|s|*s!=Slot::Empty){return Err(format!("De lijn heeft nu {n} plekken, de opgeslagen lijn {}: eerst Lijn leegmaken",slots.len()));}slots=vec![Slot::Empty;n];}
        Ok(Self{db,io,cfg,status,cmds,slots,plans:HashMap::new(),lift_up:[Arc::new(AtomicBool::new(false)),Arc::new(AtomicBool::new(false))],pause:false,stop:false,abort:false,code:None,empty_scans:0,unit_retry:[false;2]})
    }
    fn say(&mut self,t:&str){
        crate::applog::info("PRODUCTIE",t);
        let mut s=self.status.lock().unwrap();s.messages.push(format!("{} {t}",&crate::applog::local_time(now_ms())[11..]));if s.messages.len()>300{s.messages.remove(0);}s.version+=1;
    }
    fn event(&mut self,pot:Option<i64>,kind:&str,t:&str){let _=self.db.add_event(pot,kind,t);self.say(t);}
    /// Line model and state to the UI and the database (every change).
    fn persist(&mut self,state:&str,message:&str){
        let labels:Vec<String>=self.slots.iter().map(|s|match s{Slot::Empty=>"—".into(),Slot::Unknown(c)=>format!("? {c}"),
            Slot::Pot(id)=>self.db.prod_pot(*id).map(|p|format!("{} · {} v{} · {}",p.barcode,p.recipe,p.version,p.status)).unwrap_or(format!("pot {id}"))}).collect();
        {let mut s=self.status.lock().unwrap();s.state=state.into();s.slots=self.slots.clone();s.labels=labels;s.names=self.cfg.line.names();s.paused=self.pause;s.stopping=self.stop;
            s.hold=if state=="hold"{Some(message.into())}else{None};s.version+=1;}
        if let Err(x)=self.db.save_line_state(&LineState{state:state.into(),slots:self.slots.clone(),message:message.into(),updated_ms:0}){crate::applog::error("DATABASE",&format!("Lijnbeeld niet opgeslagen: {x}"));}
    }
    fn running_state(&self)->&'static str{if self.pause{"pauze"}else if self.stop{"stoppen"}else{"loopt"}}
    fn take_cmds(&mut self){
        while let Ok(c)=self.cmds.try_recv(){self.apply(c);}
        if self.io.stopped(){self.abort=true;}
    }
    fn apply(&mut self,c:ProdCmd){
        match c{ProdCmd::Pause=>{self.pause=true;self.say("Pauze: lopende dosering maakt af, geen nieuwe start of transport");}
            ProdCmd::Resume=>{if self.pause{self.say("Verder na pauze");}self.pause=false;}
            ProdCmd::Stop=>{self.stop=true;self.say("Stop: geen nieuwe potten, de lijn loopt leeg");}
            ProdCmd::Abort=>{self.abort=true;}
            ProdCmd::Restart=>{}
            ProdCmd::Code(c)=>{self.code=Some(c);}}
    }
    /// Pause: wait here (no new start/transport) until Resume, Stop keeps pausing until resumed, Abort ends.
    fn pause_point(&mut self)->Flow{
        self.take_cmds();
        if self.pause && !self.abort{
            let st=self.running_state();self.persist(st,"");
            while self.pause && !self.abort{
                if let Ok(c)=self.cmds.recv_timeout(Duration::from_millis(50)){self.apply(c);}
                if self.io.stopped(){self.abort=true;}
            }
            let st=self.running_state();self.persist(st,"");
        }
        if self.abort{Flow::Abort}else{Flow::Go}
    }
    /// Error: Hold with the cause (PLC Hold(6) to the line), the operator chooses Herstart (Restart(7), retry) or Afbreken.
    fn hold(&mut self,msg:&str)->Flow{
        self.event(None,"hold",&format!("HOLD: {msg}"));self.persist("hold",msg);
        let line=self.cfg.proc_.line;if let Err(x)=self.io.post(line,6){self.say(&format!("Hold naar lijn: {x}"));}
        loop{
            if self.io.stopped(){self.abort=true;}
            if self.abort{return Flow::Abort;}
            match self.cmds.recv_timeout(Duration::from_millis(50)){
                Ok(ProdCmd::Restart)=>{
                    self.say("Herstart na hold");if let Err(x)=self.io.post(line,7){self.say(&format!("Restart naar lijn: {x}"));}
                    if let Err(e)=self.lifts_down().and_then(|_|self.units_down()){let m=format!("Herstart: {e}");self.event(None,"hold",&format!("HOLD: {m}"));self.persist("hold",&m);continue;}
                    let st=self.running_state();self.persist(st,"");return Flow::Go;
                }
                Ok(ProdCmd::Code(c))=>{self.code=Some(c);self.say("Code ingevoerd: Herstart om te koppelen");}
                Ok(c)=>self.apply(c),Err(_)=>{}
            }
        }
    }
    /// Run `f` until it succeeds; an error holds the line until Herstart (retry) or Afbreken.
    fn retry(&mut self,what:&str,f:fn(&mut Self)->Result<()>)->Flow{
        loop{
            if self.abort || self.io.stopped(){return Flow::Abort;}
            match f(self){Ok(())=>return Flow::Go,Err(e)=>{
                if self.abort || self.io.stopped(){return Flow::Abort;}
                if let Flow::Abort=self.hold(&format!("{what}: {e}")){return Flow::Abort;}
            }}
        }
    }
    fn plc_ok(&mut self,process:u32,cmd:u32,params:&[u32],ok:&[u32],what:&str)->Result<u32>{
        let c=self.io.plc(process,cmd,params,false)?;
        if ok.contains(&c){Ok(c)}else{Err(format!("{what}: onverwacht PLC-antwoord {c} (proces {process})"))}
    }

    /// The whole cycle until the line is empty and (queue empty or Stop), or Afbreken.
    pub fn run(&mut self)->&'static str{
        {let mut s=self.status.lock().unwrap();s.running=true;s.messages.clear();}
        let restored=self.slots.iter().filter(|s|**s!=Slot::Empty).count();
        self.say(&format!("Productie gestart ({}){}",if self.io.hardware(){"hardware"}else{"simulatie"},if restored>0{format!("; {restored} pot(ten) al op de lijn")}else{String::new()}));
        self.persist("loopt","");
        let end=self.cycle();
        let out=match end{Flow::Go=>"klaar",Flow::Abort=>{self.do_abort();"afgebroken"}};
        self.persist(out,"");self.say(&format!("Productie {out}"));
        {let mut s=self.status.lock().unwrap();s.running=false;s.version+=1;}
        out
    }
    /// M2: the engine never assumes a lift is down. Stop (8) to both lifts, each must answer 9 (stopped = down).
    fn lifts_down(&mut self)->Result<()>{
        for h in Head::ALL{
            let p=self.cfg.proc_.lift[h.idx()];
            let c=self.io.plc(p,CMD_STOP,&[],false)?;
            if c!=RSP_STOPPED{return Err(format!("lift {} omlaag (proces {p}): antwoord {c}, verwacht 9",h.name()));}
            self.lift_up[h.idx()].store(false,Ordering::SeqCst);
        }
        Ok(())
    }
    /// Unit Stop again for the heads whose unit Stop failed; each must answer 9.
    fn units_down(&mut self)->Result<()>{
        for h in Head::ALL{
            if !self.unit_retry[h.idx()]{continue;}
            let p=self.cfg.proc_.unit[h.idx()];let c=self.io.plc(p,CMD_STOP,&[],false)?;
            if c!=RSP_STOPPED{return Err(format!("unit {} stop (proces {p}): antwoord {c}, verwacht 9",h.name()));}
            self.unit_retry[h.idx()]=false;self.say(&format!("Unit {} gestopt (9) na herstart",h.name()));
        }
        Ok(())
    }
    fn cycle(&mut self)->Flow{
        // Start and "Doorgaan met deze lijn": both lifts down first (a lift raised by hand or left up stays up otherwise).
        if let Flow::Abort=self.retry("Liften omlaag bij start",Self::lifts_down){return Flow::Abort;}
        if let Flow::Abort=self.retry("Lijn starten",|e|{let l=e.cfg.proc_.line;e.plc_ok(l,CMD_START,&[],&[RSP_STARTED],"lijn 1310 Start").map(|_|())}){return Flow::Abort;}
        loop{
            if let Flow::Abort=self.pause_point(){return Flow::Abort;}
            // a. Dosing: every head with a pot that still needs that head, at the same time.
            if let Flow::Abort=self.dose_round(){return Flow::Abort;}
            if let Flow::Abort=self.pause_point(){return Flow::Abort;}
            // b. Supply: next pot on the scan spot (not after Stop, not when the queue is empty).
            let waiting=self.db.queue(self.cfg.profile).map(|q|q.len()).unwrap_or(0);
            if !self.stop && waiting>0 && self.slots[0]==Slot::Empty{
                if let Flow::Abort=self.retry("Scannen",Self::supply){return Flow::Abort;}
            }
            // d. Line empty and nothing (more) to supply: done.
            if self.slots.iter().all(|s|*s==Slot::Empty){
                if self.stop || waiting==0{break;}
                // Pots wait but none arrives on the scan spot: hold instead of running empty forever.
                if self.empty_scans>=2{self.empty_scans=0;if let Flow::Abort=self.hold(&format!("Geen pot op de scanplek terwijl {waiting} pot(ten) wachten: zet de volgende pot op de lijn en kies Herstart")){return Flow::Abort;}continue;}
            }
            if let Flow::Abort=self.pause_point(){return Flow::Abort;}
            // c. Transport one spot, only with both lifts down.
            if let Flow::Abort=self.retry("Transport",Self::transport){return Flow::Abort;}
        }
        self.retry("Lijn stoppen",|e|{let l=e.cfg.proc_.line;e.plc_ok(l,CMD_STOP,&[],&[RSP_STOPPED],"lijn 1310 Stop").map(|_|())})
    }
    fn plan(&mut self,version:i64)->std::result::Result<PotPlan,String>{
        let hw=self.io.hardware();let profile=self.cfg.profile;
        self.plans.entry(version).or_insert_with(||self.db.resolve_plan(profile,version,hw).map_err(|e|e.join("; "))).clone()
    }
    fn dose_round(&mut self)->Flow{
        let mut jobs:Vec<(HeadJob,Box<dyn PotIo>)>=vec![];let mut pots:Vec<(Head,i64)>=vec![];
        for h in Head::ALL{
            let Slot::Pot(id)=self.slots[self.cfg.line.head_spot(h)] else{continue};
            let Ok(mut p)=self.db.prod_pot(id) else{continue};
            if p.heads.contains_key(h.name()) || !matches!(p.status.as_str(),ON_LINE|DOSING){continue;}
            let plan=match self.plan(p.version_id){Ok(x)=>x,Err(e)=>{
                p.heads.insert("LH1".into(),SKIP.into());p.heads.insert("LH2".into(),SKIP.into());
                let _=self.db.set_pot_state(id,NOT_DOSED,p.spot.as_deref(),&p.heads,&format!("niet uitvoerbaar: {e}"));self.event(Some(id),"plan",&format!("Pot {}: niet gedoseerd, recept niet uitvoerbaar: {e}",p.barcode));continue}};
            let steps=plan.part(h);
            if steps.is_empty(){p.heads.insert(h.name().into(),"n.v.t.".into());let _=self.db.set_pot_state(id,&p.status,p.spot.as_deref(),&p.heads,"");continue;}
            let cm=plan.pot_height_cm.or(self.cfg.heights[h.idx()]).filter(|v|*v>0.).unwrap_or(crate::devices::SIM_LIFT_CM);
            p.heads.insert(h.name().into(),DOSING.into());let _=self.db.set_pot_state(id,DOSING,p.spot.as_deref(),&p.heads,"");
            if p.started_ms==0 || p.status==ON_LINE{let _=self.db.set_pot_started(id,!self.io.hardware());}
            let io=Box::new(Tracked{inner:self.io.head(h),up:self.lift_up[h.idx()].clone()});
            jobs.push((HeadJob{head:h,run:id,steps,p1:height_p1(cm)},io));pots.push((h,id));
            self.say(&format!("{}: pot {} doseren ({} v{})",h.name(),p.barcode,plan.recipe,plan.version));
        }
        if jobs.is_empty(){return Flow::Go;}
        self.persist(self.running_state(),"");
        let out:Vec<(Head,PotOutcome)>=potrun::run_heads(jobs);
        let mut errs=vec![];
        for (h,o) in &out{
            let Some((_,id))=pots.iter().find(|x|x.0==*h) else{continue};
            let Ok(mut p)=self.db.prod_pot(*id) else{continue};
            let hs=match o.status{recipe::POT_DONE=>DONE,recipe::POT_STOPPED=>ABORTED,_=>FAILED};
            p.heads.insert(h.name().into(),hs.into());
            // Done when every head of its recipe finished; the final state is set when it leaves the line.
            let status=if hs==FAILED{FAILED}else if hs==ABORTED{ABORTED}else{ON_LINE};
            let _=self.db.set_pot_state(*id,status,p.spot.as_deref(),&p.heads,&o.note);
            self.event(Some(*id),"dosering",&format!("{}: pot {} {}{}",h.name(),p.barcode,o.status,if o.note.is_empty(){String::new()}else{format!(" — {}",o.note)}));
            if hs==FAILED{errs.push(format!("{}: pot {}: {}",h.name(),p.barcode,o.note));if o.note.contains("unit stop"){self.unit_retry[h.idx()]=true;}}
        }
        self.persist(self.running_state(),"");
        if self.io.stopped(){self.abort=true;return Flow::Abort;}
        if !errs.is_empty(){return self.hold(&format!("{} — de pot blijft 'fout'; Herstart = verder met de lijn",errs.join("; ")));}
        Flow::Go
    }
    /// Scan the pot on the scan spot (PLC 1190 + scanner), retry a no-read (Stop + Reset), link it to a waiting pot.
    fn supply(&mut self)->Result<()>{
        let bp=self.cfg.proc_.barcode;let tries=self.cfg.retries.max(1);let mut k=0;
        loop{
            k+=1;
            let (rsp,scan,raw)=self.io.barcode(bp)?;
            if rsp==16{
                let _=self.plc_ok(bp,CMD_STOP,&[],&[RSP_STOPPED],"barcode 1190 Stop");
                self.empty_scans+=1;self.event(None,"geen pot","Geen pot op de scanplek (PLC 1190 antwoord 16): plek blijft leeg");return Ok(());
            }
            if rsp!=RSP_STARTED{return Err(format!("barcode 1190 Start: onverwacht PLC-antwoord {rsp}"));}
            self.empty_scans=0;
            match scan{
                ScanResult::NoRead if k<tries=>{
                    self.say(&format!("No-read ({raw}), poging {k} van {tries}: Stop + Reset en opnieuw"));
                    self.plc_ok(bp,CMD_STOP,&[],&[RSP_STOPPED],"barcode 1190 Stop")?;self.io.plc(bp,CMD_RESET,&[],false)?;continue;
                }
                ScanResult::NoRead=>{
                    self.plc_ok(bp,CMD_STOP,&[],&[RSP_STOPPED],"barcode 1190 Stop")?;
                    return self.unknown("no-read",&format!("Geen code gelezen na {tries} poging(en) ({raw})"));
                }
                ScanResult::Code(c)=>{
                    self.plc_ok(bp,CMD_STOP,&[],&[RSP_STOPPED],"barcode 1190 Stop")?;
                    return self.link(&c);
                }
            }
        }
    }
    fn link(&mut self,code:&str)->Result<()>{
        match self.db.pot_by_barcode(code)?{
            Some(p) if p.status==QUEUED && p.order_id.is_some()=>{
                self.slots[0]=Slot::Pot(p.id);
                self.db.set_pot_state(p.id,ON_LINE,Some("Scan"),&HashMap::new(),"")?;
                self.event(Some(p.id),"scan",&format!("Pot {} gescand en gekoppeld ({} v{}, order {})",p.barcode,p.recipe,p.version,p.order));
                self.persist(self.running_state(),"");Ok(())
            }
            // A used or finished code is never dosed again.
            Some(p)=>self.unknown(code,&format!("Code {code} hoort bij pot {} met status '{}': niet opnieuw doseren",p.id,p.status)),
            None=>self.unknown(code,&format!("Onbekende code {code}")),
        }
    }
    /// No usable code: ask the operator (Hold, code can be entered) or let the pot go along undosed.
    fn unknown(&mut self,code:&str,why:&str)->Result<()>{
        if self.cfg.line.ask_operator{
            self.code=None;
            if let Flow::Abort=self.hold(&format!("{why}. Voer de code van de pot in en kies Herstart, of Herstart zonder code = pot gaat ongedoseerd mee")){return Err("Afgebroken".into());}
            if let Some(c)=self.code.take(){
                if let Ok(Some(p))=self.db.pot_by_barcode(&c){if p.status==QUEUED{return self.link(&c);}}
                self.say(&format!("Ingevoerde code {c} is geen wachtende pot"));
            }
        }
        self.slots[0]=Slot::Unknown(code.into());
        self.event(None,"onbekend",&format!("{why}: pot gaat ongedoseerd mee (niet gedoseerd)"));
        self.persist(self.running_state(),"");Ok(())
    }
    fn transport(&mut self)->Result<()>{
        for h in Head::ALL{if self.lift_up[h.idx()].load(Ordering::SeqCst){return Err(format!("lift {} staat niet omlaag: geen transport",h.name()));}}
        let t=self.cfg.proc_.transport;
        // Camelot Transport.cpp: P1..P6 = 0, P9 = −1, P10 = 1234567890.
        let p=[0,0,0,0,0,0,0,0,u32::MAX,1234567890];
        let c=self.plc_ok(t,CMD_START,&p,&[RSP_STARTED,16],"transport 1220 Start")?;
        // M3 (Camelot Transport.cpp): 16 = no container, the transport did not happen: no Stop, the line model stays.
        if c==16{
            if self.slots.iter().any(|s|*s!=Slot::Empty){return Err("PLC antwoordt 16 (geen pot/container): transport niet uitgevoerd terwijl er volgens het lijnbeeld potten op de lijn staan. Controleer de lijn".into());}
            self.say("Transport: PLC meldt 16 (geen pot), lijn leeg: niets verplaatst");return Ok(());
        }
        self.plc_ok(t,CMD_STOP,&[],&[RSP_STOPPED],"transport 1220 Stop")?;
        // Shift: the pot on the last spot leaves the line.
        let out=self.slots.pop().unwrap_or_default();self.slots.insert(0,Slot::Empty);
        let names=self.cfg.line.names();
        for (i,s) in self.slots.iter().enumerate(){if let Slot::Pot(id)=s{if let Ok(p)=self.db.prod_pot(*id){let _=self.db.set_pot_state(*id,&p.status,Some(&names[i]),&p.heads,"");}}}
        match out{
            Slot::Pot(id)=>self.finish_pot(id),
            Slot::Unknown(c)=>self.event(None,"uit",&format!("Pot zonder code ({c}) van de lijn: niet gedoseerd")),
            Slot::Empty=>{}
        }
        self.persist(self.running_state(),"");Ok(())
    }
    /// A pot leaves the line: klaar when every head of its recipe finished, else its error/abort state stays.
    fn finish_pot(&mut self,id:i64){
        let Ok(p)=self.db.prod_pot(id) else{return};
        let need:Vec<&str>=match self.plan(p.version_id){Ok(pl)=>pl.heads().iter().map(|h|h.name()).collect(),Err(_)=>vec![]};
        let status=if matches!(p.status.as_str(),FAILED|ABORTED|INTERRUPTED|NOT_DOSED){p.status.clone()}
            else if !need.is_empty() && need.iter().all(|h|p.heads.get(*h).map(|s|s==DONE).unwrap_or(false)){DONE.into()}
            else{NOT_DOSED.into()};
        let _=self.db.set_pot_state(id,&status,None,&p.heads,"");
        self.status.lock().unwrap().done+=1;
        self.event(Some(id),"uit",&format!("Pot {} van de lijn: {status}",p.barcode));
    }
    /// Afbreken: pumps stopped (by the caller's STOP), both lifts down, units stop, Abort(5) to line, transport,
    /// barcode and units; pots stay known on their spot as "afgebroken".
    fn do_abort(&mut self){
        self.persist("afbreken","");self.say("Afbreken: pompen stoppen, liften omlaag, Abort naar lijn en units");
        // A head that was dosing already lowered its lift on the STOP; any lift still up goes down now.
        for h in Head::ALL{if self.lift_up[h.idx()].load(Ordering::SeqCst){let mut io=self.io.head(h);
            if let Err(x)=io.lift(h,false,0,true){self.say(&format!("Lift {} omlaag mislukt: {x}",h.name()));}else{self.lift_up[h.idx()].store(false,Ordering::SeqCst);}
            let _=io.unit(h,false,true);}}
        // Abort(5) to line, transport, barcode and units without waiting for a reply (as Camelot).
        let pr=self.cfg.proc_.clone();
        for p in [pr.line,pr.transport,pr.barcode,pr.unit[0],pr.unit[1]]{if let Err(x)=self.io.post(p,CMD_ABORT){self.say(&format!("Abort proces {p}: {x}"));}}
        let names=self.cfg.line.names();
        for (i,s) in self.slots.clone().iter().enumerate(){if let Slot::Pot(id)=s{if let Ok(mut p)=self.db.prod_pot(*id){
            if matches!(p.status.as_str(),ON_LINE|DOSING|FAILED){
                for k in ["LH1","LH2"]{let v=p.heads.entry(k.into()).or_insert_with(||SKIP.into());if v==DOSING{*v=ABORTED.into();}}
                let _=self.db.set_pot_state(*id,ABORTED,Some(&names[i]),&p.heads,"afgebroken");
                self.event(Some(*id),"afgebroken",&format!("Pot {} afgebroken op {}",p.barcode,names[i]));
            }}}}
    }
}

/// Pre-start check (plan step 1): what is missing, empty = ready. `hardware` = mode of the CAN connection.
pub fn precheck(db:&Db,profile:ProfileKind,hardware:bool,heights:[Option<f64>;2])->Vec<String>{
    let mut w=vec![];
    let q=match db.queue(profile){Ok(q)=>q,Err(e)=>return vec![e]};
    let mut seen=std::collections::HashSet::new();
    for p in &q{
        if !seen.insert(p.version_id){continue;}
        match db.resolve_plan(profile,p.version_id,hardware){
            Err(errs)=>w.push(format!("{} v{}: {}",p.recipe,p.version,errs.join("; "))),
            Ok(plan)=>for h in plan.heads(){
                match plan.pot_height_cm.or(heights[h.idx()]).filter(|v|*v>0.){
                    Some(cm)=>{if let Err(e)=crate::machine::check_height(cm){w.push(format!("{} v{}: lifthoogte {}: {e}",p.recipe,p.version,h.name()));}}
                    None if hardware=>w.push(format!("{} v{}: geen lifthoogte voor {} (Instellingen of pothoogte op het recept)",p.recipe,p.version,h.name())),
                    None=>{}
                }
            }
        }
    }
    w
}

#[cfg(test)] pub(crate) mod tests{
    use super::*;
    use crate::{potrun::BlockDose,recipe::{tests::{item,step,r1_setup},RecipeDraft},scanner::ScanResult};
    use std::{collections::VecDeque,time::Instant};
    /// Shared fake machine: PLC replies (scripted per process, else as the emulator), scanner answers, heads that dose exactly.
    #[derive(Default)]
    pub struct World{pub log:Vec<(Instant,String)>,pub plc_next:HashMap<u32,VecDeque<u32>>,pub plc_silent:u32,pub scans:VecDeque<(u32,ScanResult)>,pub feed:VecDeque<String>,
        pub weight:[f64;2],pub lift:[bool;2],pub dose_ms:u64,pub stop_after_doses:Option<usize>,pub doses:usize,pub plc_cmds:Vec<(u32,u32,Vec<u32>)>}
    #[derive(Clone)]
    pub struct FakeLine{pub w:Arc<Mutex<World>>,pub stop:Arc<AtomicBool>,pub db:std::path::PathBuf}
    impl FakeLine{fn log(&self,t:String){self.w.lock().unwrap().log.push((Instant::now(),t));}}
    impl LineIo for FakeLine{
        fn plc(&mut self,process:u32,cmd:u32,params:&[u32],force:bool)->Result<u32>{
            if !force && self.stopped(){return Err("Gestopt".into());}
            let mut w=self.w.lock().unwrap();w.plc_cmds.push((process,cmd,params.to_vec()));
            if w.plc_silent>0 && !force{w.plc_silent-=1;drop(w);std::thread::sleep(Duration::from_millis(100));self.log(format!("plc {process} {cmd} time-out"));return Err(format!("Geen antwoord van PLC binnen 1 s"));}
            let scripted=w.plc_next.get_mut(&process).and_then(|q|q.pop_front());
            let lift=process%1000==350;
            let c=scripted.unwrap_or(match cmd{CMD_START if lift=>RSP_UP,CMD_START=>RSP_STARTED,CMD_STOP=>RSP_STOPPED,CMD_ABORT=>RSP_ABORTED,_=>RSP_STARTED});
            if lift && c==RSP_UP{w.lift[if process/1000==1{0}else{1}]=true;}if lift && cmd==CMD_STOP{w.lift[if process/1000==1{0}else{1}]=false;}
            let up=w.lift;drop(w);
            self.log(format!("plc {process} {cmd} -> {c}{}",if process==1220 && cmd==CMD_START{format!(" liften {:?}",up)}else{String::new()}));Ok(c)
        }
        fn barcode(&mut self,process:u32)->Result<(u32,ScanResult,String)>{
            let rsp=self.plc(process,CMD_START,&[1],false)?;
            let mut w=self.w.lock().unwrap();
            let (r,s)=match w.scans.pop_front(){Some((r,s))=>(r,s),None=>match w.feed.pop_front(){Some(c)=>(rsp,ScanResult::Code(c)),None=>(16,ScanResult::NoRead)}};
            drop(w);self.log(format!("scan {r} {}",s.text()));Ok((r,s.clone(),s.text()))
        }
        fn post(&mut self,process:u32,cmd:u32)->Result<()>{self.plc(process,cmd,&[],true).map(|_|())}
        fn head(&mut self,_h:Head)->Box<dyn PotIo>{Box::new(FakeHead{l:self.clone(),db:None})}
        fn stopped(&self)->bool{self.stop.load(Ordering::SeqCst)}
        fn hardware(&self)->bool{false}
    }
    pub struct FakeHead{l:FakeLine,db:Option<Db>}
    impl PotIo for FakeHead{
        fn lift(&mut self,h:Head,up:bool,p1:u32,force:bool)->Result<()>{
            let p=if h==Head::LH1{1350}else{2350};let pv=[p1];let c=self.l.plc(p,if up{CMD_START}else{CMD_STOP},if up{&pv[..]}else{&[]},force)?;
            crate::devices::pot_reply(if up{CMD_START}else{CMD_STOP},p,c,&format!("lift {}",h.name()))
        }
        fn unit(&mut self,h:Head,s:bool,f:bool)->Result<()>{let p=if h==Head::LH1{1300}else{2300};let c=self.l.plc(p,if s{CMD_START}else{CMD_STOP},&[],f)?;
            crate::devices::pot_reply(if s{CMD_START}else{CMD_STOP},p,c,&format!("unit {}",h.name()))}
        fn weigh(&mut self,h:Head)->Result<f64>{if self.l.stopped(){return Err("Gestopt".into());}Ok(self.l.w.lock().unwrap().weight[h.idx()])}
        fn dose(&mut self,h:Head,items:&[(String,f64,i64)])->Result<Vec<BlockDose>>{
            let ms={let mut w=self.l.w.lock().unwrap();w.doses+=1;if w.stop_after_doses.is_some_and(|n|w.doses>n){drop(w);self.l.stop.store(true,Ordering::SeqCst);return Err("Gestopt".into());}w.dose_ms};
            let t0=now_ms();std::thread::sleep(Duration::from_millis(ms));if self.l.stopped(){return Err("Gestopt".into());}
            let mut w=self.l.w.lock().unwrap();for (_,g,_) in items{w.weight[h.idx()]+=g;}drop(w);
            self.l.log(format!("dose {} {}",h.name(),items.iter().map(|x|x.0.clone()).collect::<Vec<_>>().join("+")));
            Ok(items.iter().map(|(p,g,_)|BlockDose{pump:p.clone(),steps:(g*1000.) as i64,start_ms:t0,end_ms:now_ms()}).collect())
        }
        fn stopped(&self)->bool{self.l.stopped()}
        fn store(&mut self,i:&recipe::PotItemRow)->Result<()>{if self.db.is_none(){self.db=Some(Db::open(&self.l.db)?);}self.db.as_ref().unwrap().add_pot_item(i).map(|_|())}
        fn wait_operator(&mut self,_m:&str)->bool{true}
        fn progress(&mut self,_h:Option<Head>,_t:&str){}
    }
    fn cfg(line:LineCfg)->EngineCfg{EngineCfg{profile:ProfileKind::Vsd,line,proc_:Processes{line:1310,transport:1220,barcode:1190,unit:[1300,2300],lift:[1350,2350]},heights:[None,None],retries:2}}
    /// Database with the R1 pumps/ingredients and three recipes: LH1 only (A, B∥C), LH2 only (E, F), mixed (A, E).
    pub fn setup(name:&str)->(std::path::PathBuf,Db,[i64;3]){
        let dir=crate::recipe::tests::temp(name);let mut db=Db::open(&dir.join("p.db")).unwrap();let v=recipes3(&mut db);(dir,db,v)
    }
    /// Mixed recipe (A on LH1, E on LH2) for a database that already has the R1 setup.
    pub fn recipes3_only(db:&mut Db)->i64{
        db.save_recipe(ProfileKind::Vsd,None,&RecipeDraft{name:"Gemengd".into(),steps:vec![step(vec![item("A",0.5)]),step(vec![item("E",0.7)])],..Default::default()}).unwrap().version_id
    }
    pub fn recipes3(db:&mut Db)->[i64;3]{
        r1_setup(db);
        let mk=|db:&mut Db,n:&str,steps:Vec<crate::recipe::StepDraft>|db.save_recipe(ProfileKind::Vsd,None,&RecipeDraft{name:n.into(),steps,..Default::default()}).unwrap().version_id;
        let a=mk(db,"Alleen LH1",vec![step(vec![item("A",1.)]),step(vec![item("B",2.),item("C",1.)])]);
        let b=mk(db,"Alleen LH2",vec![step(vec![item("E",1.)]),step(vec![item("F",0.5)])]);
        let c=mk(db,"Gemengd",vec![step(vec![item("A",0.5)]),step(vec![item("E",0.7)])]);
        [a,b,c]
    }
    fn engine(dir:&std::path::Path,line:LineCfg,w:&Arc<Mutex<World>>)->(Engine,mpsc::Sender<ProdCmd>,Arc<Mutex<ProdStatus>>,Arc<AtomicBool>){
        let stop=Arc::new(AtomicBool::new(false));let io=FakeLine{w:w.clone(),stop:stop.clone(),db:dir.join("p.db")};
        let (tx,rx)=mpsc::channel();let st=Arc::new(Mutex::new(ProdStatus::default()));
        (Engine::new(Db::open(&dir.join("p.db")).unwrap(),Box::new(io),cfg(line),st.clone(),rx).unwrap(),tx,st,stop)
    }
    fn feed(db:&Db,w:&Arc<Mutex<World>>){w.lock().unwrap().feed=db.queue(ProfileKind::Vsd).unwrap().into_iter().map(|p|p.barcode).collect();}
    fn plc_lines(w:&Arc<Mutex<World>>)->Vec<String>{w.lock().unwrap().log.iter().map(|x|x.1.clone()).filter(|l|l.starts_with("plc")).collect()}

    #[test]fn p1_three_orders_run_out_both_heads_on_different_pots(){
        let (dir,mut db,v)=setup("p1");
        db.create_order(ProfileKind::Vsd,v[0],"O-LH1","B1",0,2).unwrap();db.create_order(ProfileKind::Vsd,v[1],"O-LH2","",0,2).unwrap();db.create_order(ProfileKind::Vsd,v[2],"O-mix","",0,3).unwrap();
        let w=Arc::new(Mutex::new(World{dose_ms:30,..Default::default()}));feed(&db,&w);
        let (mut eng,_tx,st,_)=engine(&dir,LineCfg::default(),&w);
        assert_eq!(eng.run(),"klaar");
        let all=db.history(ProfileKind::Vsd,&HistFilter::default(),100).unwrap();assert_eq!(all.len(),7);
        assert!(all.iter().all(|p|p.status==DONE && p.spot.is_none()),"{:?}",all.iter().map(|p|(&p.barcode,&p.status,&p.heads)).collect::<Vec<_>>());
        // Each pot dosed only on the heads of its recipe, never twice.
        for p in &all{let it=db.pot_items(p.id).unwrap();let heads:std::collections::BTreeSet<String>=it.iter().map(|x|x.head.clone()).collect();
            let want:Vec<&str>=match p.recipe.as_str(){"Alleen LH1"=>vec!["LH1"],"Alleen LH2"=>vec!["LH2"],_=>vec!["LH1","LH2"]};
            assert_eq!(heads.iter().map(|s|s.as_str()).collect::<Vec<_>>(),want,"{}",p.recipe);
            assert_eq!(it.len(),match p.recipe.as_str(){"Alleen LH1"=>3,"Alleen LH2"=>2,_=>2});}
        // LH1 and LH2 busy at the same time on different pots (item time stamps overlap).
        let items:Vec<(i64,String,i64,i64)>=all.iter().flat_map(|p|db.pot_items(p.id).unwrap().into_iter().map(|x|(x.pot_run_id,x.head,x.started_ms.unwrap(),x.ended_ms.unwrap()))).collect();
        let overlap=items.iter().any(|a|a.1=="LH1" && items.iter().any(|b|b.1=="LH2" && b.0!=a.0 && a.2<b.3 && b.2<a.3));assert!(overlap,"geen overlap LH1/LH2");
        // 1310 Start first, Stop last; every transport with both lifts down.
        // Both lifts down (9) first, then 1310 Start; Stop last.
        let l=plc_lines(&w);let s0=l.iter().position(|x|x.starts_with("plc 1310 1 ->")).unwrap();assert_eq!(&l[..s0],&["plc 1350 8 -> 9","plc 2350 8 -> 9"],"{l:?}");assert!(l.last().unwrap().starts_with("plc 1310 8 -> 9"),"{l:?}");
        let tr:Vec<&String>=l.iter().filter(|x|x.starts_with("plc 1220 1")).collect();assert!(tr.len()>=7);assert!(tr.iter().all(|x|x.ends_with("liften [false, false]")),"{tr:?}");
        assert!(st.lock().unwrap().slots.iter().all(|s|*s==Slot::Empty));assert_eq!(db.line_state().unwrap().unwrap().state,"klaar");
        drop(db);drop(eng);crate::recipe::tests::cleanup(&dir);
    }
    #[test]fn p2_noread_unknown_used_code_and_no_pot(){
        let (dir,mut db,v)=setup("p2");
        let (_,pots)=db.create_order(ProfileKind::Vsd,v[0],"O","",0,2).unwrap();
        // First pot: no-read twice (retry with Stop + Reset, then undosed); then a used code; then pot 2; then no pot.
        let w=Arc::new(Mutex::new(World::default()));
        {let mut x=w.lock().unwrap();x.scans=VecDeque::from(vec![(2,ScanResult::NoRead),(2,ScanResult::NoRead),(2,ScanResult::Code("9999999999".into())),(2,ScanResult::Code(barcode_of(pots[1])))]);x.feed=VecDeque::from(vec![barcode_of(pots[0])]);}
        let (mut eng,_tx,_st,_)=engine(&dir,LineCfg::default(),&w);
        assert_eq!(eng.run(),"klaar");
        let l=plc_lines(&w);let i=l.iter().position(|x|x.starts_with("plc 1190 2")).expect("Reset na no-read");assert!(l[i-1].starts_with("plc 1190 8"),"Stop vóór Reset");
        let ev:Vec<String>=db.events(None,100).unwrap().into_iter().map(|e|e.3).collect();
        assert!(ev.iter().any(|e|e.contains("Geen code gelezen na 2 poging(en)")),"{ev:?}");assert!(ev.iter().any(|e|e.contains("Onbekende code 9999999999")));
        assert_eq!(db.prod_pot(pots[0]).unwrap().status,DONE);assert_eq!(db.prod_pot(pots[1]).unwrap().status,DONE);
        // Finished code scanned again in a new run: never dosed again.
        let (_,p3)=db.create_order(ProfileKind::Vsd,v[0],"O2","",0,1).unwrap();
        let w=Arc::new(Mutex::new(World::default()));{let mut x=w.lock().unwrap();x.scans=VecDeque::from(vec![(2,ScanResult::Code(barcode_of(pots[0])))]);x.feed=VecDeque::from(vec![barcode_of(p3[0])]);}
        let n=db.pot_items(pots[0]).unwrap().len();let (mut eng,_tx,_st,_)=engine(&dir,LineCfg::default(),&w);assert_eq!(eng.run(),"klaar");
        assert_eq!(db.pot_items(pots[0]).unwrap().len(),n,"afgeronde code niet opnieuw gedoseerd");
        assert!(db.events(None,100).unwrap().iter().any(|e|e.3.contains("niet opnieuw doseren")));
        // No pot on the scan spot while pots wait: two empty scans → Hold; Abort ends it.
        db.create_order(ProfileKind::Vsd,v[0],"O3","",0,1).unwrap();
        let w=Arc::new(Mutex::new(World::default()));let (mut eng,tx,st,_)=engine(&dir,LineCfg::default(),&w);
        let h=std::thread::spawn(move||eng.run());
        let t=Instant::now();while st.lock().unwrap().hold.is_none(){assert!(t.elapsed()<Duration::from_secs(5));std::thread::sleep(Duration::from_millis(10));}
        assert!(st.lock().unwrap().hold.clone().unwrap().contains("Geen pot op de scanplek"));tx.send(ProdCmd::Abort).unwrap();assert_eq!(h.join().unwrap(),"afgebroken");
        assert!(db.events(None,100).unwrap().iter().any(|e|e.3.contains("antwoord 16")));
        drop(db);crate::recipe::tests::cleanup(&dir);
    }
    #[test]fn p3_pause_stop_abort_and_hold_restart(){
        let (dir,mut db,v)=setup("p3");
        db.create_order(ProfileKind::Vsd,v[2],"O","",0,4).unwrap();
        let w=Arc::new(Mutex::new(World{dose_ms:20,..Default::default()}));feed(&db,&w);
        let (mut eng,tx,st,_)=engine(&dir,LineCfg::default(),&w);
        // Pause right away: no transport while paused; Stop: run out with the pots already on the line.
        tx.send(ProdCmd::Pause).unwrap();let h=std::thread::spawn(move||eng.run());
        std::thread::sleep(Duration::from_millis(300));assert_eq!(st.lock().unwrap().state,"pauze");
        assert!(!plc_lines(&w).iter().any(|l|l.starts_with("plc 1220")),"geen transport tijdens pauze");
        tx.send(ProdCmd::Resume).unwrap();
        let t=Instant::now();while db.history(ProfileKind::Vsd,&HistFilter::default(),10).unwrap().is_empty() && db.queue(ProfileKind::Vsd).unwrap().len()>2{assert!(t.elapsed()<Duration::from_secs(10));std::thread::sleep(Duration::from_millis(5));}
        tx.send(ProdCmd::Stop).unwrap();assert_eq!(h.join().unwrap(),"klaar");
        let left=db.queue(ProfileKind::Vsd).unwrap().len();assert!(left>=1,"Stop: geen nieuwe potten meer ({left} wachten nog)");
        assert!(db.history(ProfileKind::Vsd,&HistFilter::default(),10).unwrap().iter().all(|p|p.status==DONE),"lijn leeggelopen");
        // Hold on a PLC time-out in transport; Herstart retries and the line continues.
        let w=Arc::new(Mutex::new(World::default()));feed(&db,&w);
        let (mut eng,tx,st,_)=engine(&dir,LineCfg::default(),&w);
        w.lock().unwrap().plc_next.insert(1220,VecDeque::from(vec![12]));
        let h=std::thread::spawn(move||eng.run());
        let t=Instant::now();while st.lock().unwrap().hold.is_none(){assert!(t.elapsed()<Duration::from_secs(5));std::thread::sleep(Duration::from_millis(5));}
        assert!(st.lock().unwrap().hold.clone().unwrap().contains("Transport"),"{:?}",st.lock().unwrap().hold);
        // Hold(6) goes to the line right after the hold is shown (no wait for its reply).
        let t=Instant::now();while !plc_lines(&w).iter().any(|l|l.starts_with("plc 1310 6")){assert!(t.elapsed()<Duration::from_secs(2),"Hold(6) naar de lijn");std::thread::sleep(Duration::from_millis(5));}
        tx.send(ProdCmd::Restart).unwrap();assert_eq!(h.join().unwrap(),"klaar");assert!(plc_lines(&w).iter().any(|l|l.starts_with("plc 1310 7")),"Restart(7)");
        // Afbreken while dosing: lifts down, Abort(5) to line/transport/barcode/units, pots kept as afgebroken on their spot.
        db.create_order(ProfileKind::Vsd,v[2],"O2","",0,3).unwrap();
        let w=Arc::new(Mutex::new(World{dose_ms:400,..Default::default()}));feed(&db,&w);
        let (mut eng,tx,st,stop)=engine(&dir,LineCfg::default(),&w);
        let h=std::thread::spawn(move||eng.run());
        let t=Instant::now();while !w.lock().unwrap().lift[0]{assert!(t.elapsed()<Duration::from_secs(10));std::thread::sleep(Duration::from_millis(5));}
        let t=Instant::now();stop.store(true,Ordering::SeqCst);tx.send(ProdCmd::Abort).unwrap();assert_eq!(h.join().unwrap(),"afgebroken");assert!(t.elapsed()<Duration::from_secs(1),"{:?}",t.elapsed());
        assert_eq!(w.lock().unwrap().lift,[false,false]);
        let l=plc_lines(&w);for p in [1310,1220,1190,1300,2300]{assert!(l.iter().any(|x|x.starts_with(&format!("plc {p} 5 ->"))),"Abort naar {p}: {l:?}");}
        let line=db.line_state().unwrap().unwrap();assert_eq!(line.state,"afgebroken");
        let on:Vec<PotRow>=line.slots.iter().filter_map(|s|if let Slot::Pot(id)=s{db.prod_pot(*id).ok()}else{None}).collect();
        assert!(!on.is_empty() && on.iter().all(|p|p.status==ABORTED && p.spot.is_some()),"{:?}",on.iter().map(|p|(&p.status,&p.spot)).collect::<Vec<_>>());
        let _=st;drop(db);crate::recipe::tests::cleanup(&dir);
    }
    #[test]fn m2_lifts_lowered_at_start_and_herstart_before_any_transport(){
        let (dir,mut db,v)=setup("m2");db.create_order(ProfileKind::Vsd,v[0],"O","",0,1).unwrap();
        // A lift left up by hand; the engine's own flags say nothing about it.
        let w=Arc::new(Mutex::new(World{lift:[true,true],..Default::default()}));feed(&db,&w);
        let (mut eng,_tx,_st,_)=engine(&dir,LineCfg::default(),&w);assert_eq!(eng.run(),"klaar");
        let l=plc_lines(&w);let first_tr=l.iter().position(|x|x.starts_with("plc 1220 1")).unwrap();
        let s1=l.iter().position(|x|x.starts_with("plc 1350 8 -> 9")).unwrap();let s2=l.iter().position(|x|x.starts_with("plc 2350 8 -> 9")).unwrap();
        assert!(s1<first_tr && s2<first_tr && s1<2 && s2<2,"liften eerst omlaag: {l:?}");
        // Lift answers something else than 9 at Start: Hold; Herstart lowers both lifts again before going on.
        db.create_order(ProfileKind::Vsd,v[0],"O2","",0,1).unwrap();
        let w=Arc::new(Mutex::new(World::default()));feed(&db,&w);w.lock().unwrap().plc_next.insert(2350,VecDeque::from(vec![12]));
        let (mut eng,tx,st,_)=engine(&dir,LineCfg::default(),&w);let h=std::thread::spawn(move||eng.run());
        let t=Instant::now();while st.lock().unwrap().hold.is_none(){assert!(t.elapsed()<Duration::from_secs(5));std::thread::sleep(Duration::from_millis(5));}
        assert!(st.lock().unwrap().hold.clone().unwrap().contains("Liften omlaag bij start"));assert!(!plc_lines(&w).iter().any(|x|x.starts_with("plc 1220")));
        tx.send(ProdCmd::Restart).unwrap();assert_eq!(h.join().unwrap(),"klaar");
        // Herstart after a transport hold: lifts down again before the next step.
        db.create_order(ProfileKind::Vsd,v[0],"O3","",0,1).unwrap();
        let w=Arc::new(Mutex::new(World::default()));feed(&db,&w);w.lock().unwrap().plc_next.insert(1220,VecDeque::from(vec![12]));
        let (mut eng,tx,st,_)=engine(&dir,LineCfg::default(),&w);let h=std::thread::spawn(move||eng.run());
        let t=Instant::now();while st.lock().unwrap().hold.is_none(){assert!(t.elapsed()<Duration::from_secs(5));std::thread::sleep(Duration::from_millis(5));}
        let n=plc_lines(&w).len();tx.send(ProdCmd::Restart).unwrap();assert_eq!(h.join().unwrap(),"klaar");
        let l=plc_lines(&w);let after=&l[n..];let tr=after.iter().position(|x|x.starts_with("plc 1220 1")).unwrap();
        assert!(after[..tr].iter().any(|x|x.starts_with("plc 1350 8 -> 9")) && after[..tr].iter().any(|x|x.starts_with("plc 2350 8 -> 9")),"{after:?}");
        drop(db);crate::recipe::tests::cleanup(&dir);
    }
    #[test]fn unit_stop_not_9_at_normal_end_holds_the_line(){
        let (dir,mut db,v)=setup("unit9");db.create_order(ProfileKind::Vsd,v[0],"O","",0,1).unwrap();
        // Unit LH1: Start → 2, Stop → 12 (not 9): head fails, Hold with the cause before any further transport.
        let w=Arc::new(Mutex::new(World::default()));feed(&db,&w);w.lock().unwrap().plc_next.insert(1300,VecDeque::from(vec![2,12]));
        let (mut eng,tx,st,_)=engine(&dir,LineCfg::default(),&w);let h=std::thread::spawn(move||eng.run());
        let t=Instant::now();while st.lock().unwrap().hold.is_none(){assert!(t.elapsed()<Duration::from_secs(5));std::thread::sleep(Duration::from_millis(5));}
        let m=st.lock().unwrap().hold.clone().unwrap();assert!(m.contains("unit stop") && m.contains("12"),"{m}");
        let l=plc_lines(&w);let i=l.iter().position(|x|x.starts_with("plc 1300 8 -> 12")).unwrap();assert!(!l[i..].iter().any(|x|x.starts_with("plc 1220")),"geen transport na mislukte unit stop: {l:?}");
        // Herstart: unit Stop of LH1 again (now 9) before the next transport.
        let n=plc_lines(&w).len();tx.send(ProdCmd::Restart).unwrap();assert_eq!(h.join().unwrap(),"klaar");
        let l=plc_lines(&w);let after=&l[n..];let u=after.iter().position(|x|x.starts_with("plc 1300 8 -> 9")).expect("unit stop herhaald");
        let t=after.iter().position(|x|x.starts_with("plc 1220 1")).unwrap();assert!(u<t,"unit Stop 9 vóór transport: {after:?}");
        drop(db);crate::recipe::tests::cleanup(&dir);
    }
    #[test]fn m3_transport_16_does_not_move_the_line(){
        let (dir,mut db,v)=setup("m3");db.create_order(ProfileKind::Vsd,v[0],"O","",0,1).unwrap();
        let w=Arc::new(Mutex::new(World::default()));feed(&db,&w);w.lock().unwrap().plc_next.insert(1220,VecDeque::from(vec![16]));
        let (mut eng,tx,st,_)=engine(&dir,LineCfg::default(),&w);let h=std::thread::spawn(move||eng.run());
        let t=Instant::now();while st.lock().unwrap().hold.is_none(){assert!(t.elapsed()<Duration::from_secs(5));std::thread::sleep(Duration::from_millis(5));}
        assert!(st.lock().unwrap().hold.clone().unwrap().contains("transport niet uitgevoerd"));
        let l=plc_lines(&w);let i=l.iter().position(|x|x.starts_with("plc 1220 1 -> 16")).unwrap();assert!(!l[i+1..].iter().any(|x|x.starts_with("plc 1220 8")),"geen Stop na 16: {l:?}");
        assert_eq!(st.lock().unwrap().slots[0],Slot::Pot(db.history(ProfileKind::Vsd,&HistFilter::default(),5).unwrap()[0].id),"pot nog op de scanplek");
        tx.send(ProdCmd::Restart).unwrap();assert_eq!(h.join().unwrap(),"klaar");assert_eq!(db.history(ProfileKind::Vsd,&HistFilter::default(),5).unwrap()[0].status,DONE);
        drop(db);crate::recipe::tests::cleanup(&dir);
    }
    #[test]fn p4_plc_silent_gives_hold_then_restart_continues(){
        let (dir,mut db,v)=setup("p4");db.create_order(ProfileKind::Vsd,v[0],"O","",0,1).unwrap();
        let w=Arc::new(Mutex::new(World{plc_silent:1,..Default::default()}));feed(&db,&w);
        let (mut eng,tx,st,_)=engine(&dir,LineCfg::default(),&w);let h=std::thread::spawn(move||eng.run());
        let t=Instant::now();while st.lock().unwrap().hold.is_none(){assert!(t.elapsed()<Duration::from_secs(5));std::thread::sleep(Duration::from_millis(5));}
        let m=st.lock().unwrap().hold.clone().unwrap();assert!(m.contains("Liften omlaag bij start") && m.contains("Geen antwoord"),"{m}");
        tx.send(ProdCmd::Restart).unwrap();assert_eq!(h.join().unwrap(),"klaar");
        assert_eq!(db.history(ProfileKind::Vsd,&HistFilter::default(),10).unwrap()[0].status,DONE);
        drop(db);crate::recipe::tests::cleanup(&dir);
    }
    #[test]fn p5_restart_restores_line_and_never_redoses(){
        let (dir,mut db,v)=setup("p5");let (_,pots)=db.create_order(ProfileKind::Vsd,v[2],"O","",0,2).unwrap();
        // State as the app left it: pot 1 dosing at LH1 (LH1 doses stored), pot 2 waiting on the "Wacht 1" spot.
        let mut h=HashMap::new();h.insert("LH1".to_string(),DOSING.to_string());
        db.set_pot_state(pots[0],DOSING,Some("LH1"),&h,"").unwrap();db.set_pot_state(pots[1],ON_LINE,Some("Wacht 1"),&HashMap::new(),"").unwrap();
        db.add_pot_item(&recipe::PotItemRow{pot_run_id:pots[0],ingredient:"A".into(),pump:"P_C01".into(),head:"LH1".into(),requested_g:0.5,dosed_g:Some(0.3),tolerance_pct:1.,status:"gestopt".into(),..Default::default()}).unwrap();
        db.save_line_state(&LineState{state:"loopt".into(),slots:vec![Slot::Empty,Slot::Pot(pots[1]),Slot::Pot(pots[0]),Slot::Empty],..Default::default()}).unwrap();
        let msg=db.restore_after_restart().unwrap().unwrap();assert!(msg.contains("Lijn hersteld") && msg.contains(&barcode_of(pots[0])),"{msg}");
        let p=db.prod_pot(pots[0]).unwrap();assert_eq!(p.status,INTERRUPTED);assert_eq!(p.spot.as_deref(),Some("LH1"));
        // Operator: continue with this line. Pot 1 is not dosed again (neither LH1 nor LH2); pot 2 is dosed normally.
        let w=Arc::new(Mutex::new(World::default()));let (mut eng,_tx,st,_)=engine(&dir,LineCfg::default(),&w);
        assert_eq!(st.lock().unwrap().slots.len(),0);assert_eq!(eng.run(),"klaar");
        assert_eq!(db.pot_items(pots[0]).unwrap().len(),1,"onderbroken pot nooit opnieuw gedoseerd");
        assert_eq!(db.prod_pot(pots[0]).unwrap().status,INTERRUPTED);assert_eq!(db.prod_pot(pots[1]).unwrap().status,DONE);
        // Clear the line instead: pots on it become onderbroken.
        db.save_line_state(&LineState{state:"onderbroken".into(),slots:vec![Slot::Pot(pots[1]),Slot::Empty,Slot::Empty,Slot::Empty],..Default::default()}).unwrap();
        assert_eq!(db.clear_line().unwrap(),1);assert!(db.line_state().unwrap().unwrap().slots.is_empty());
        drop(db);crate::recipe::tests::cleanup(&dir);
    }
    #[test]fn p6_barcodes_unique_and_manual_code(){
        let (dir,mut db,v)=setup("p6");
        let (_,a)=db.create_order(ProfileKind::Vsd,v[0],"A","",0,3).unwrap();let (_,b)=db.create_order(ProfileKind::Vsd,v[1],"B","",5,2).unwrap();
        let q=db.queue(ProfileKind::Vsd).unwrap();
        assert_eq!(q.iter().map(|p|p.id).collect::<Vec<_>>(),vec![b[0],b[1],a[0],a[1],a[2]],"prioriteit eerst");
        assert!(q.iter().all(|p|p.barcode==barcode_of(p.id) && p.barcode.len()==10));
        assert!(db.set_pot_barcode(a[0],&barcode_of(a[1])).unwrap_err().contains("uniek"));
        db.set_pot_barcode(a[0],"LBL-0001").unwrap();assert_eq!(db.pot_by_barcode("LBL-0001").unwrap().unwrap().id,a[0]);
        assert!(db.conn.execute("UPDATE pot_run SET barcode=?2 WHERE id=?1",params![a[2],"LBL-0001"]).is_err(),"UNIQUE in de database");
        db.move_pot(ProfileKind::Vsd,a[1],-1).unwrap();assert_eq!(db.queue(ProfileKind::Vsd).unwrap()[2].id,a[1]);
        db.set_pot_state(a[2],DONE,None,&HashMap::new(),"").unwrap();assert!(db.set_pot_barcode(a[2],"X1").is_err(),"afgeronde pot: code vast");
        let ob=db.prod_pot(b[0]).unwrap().order_id.unwrap();assert_eq!(db.cancel_order(ob).unwrap(),2);assert!(db.queue(ProfileKind::Vsd).unwrap().iter().all(|p|p.order_id!=Some(ob)));
        assert!(db.create_order(ProfileKind::Vsd,v[0],"X","",0,0).is_err());
        drop(db);crate::recipe::tests::cleanup(&dir);
    }
    #[test]fn precheck_names_what_is_missing(){
        let (dir,mut db,v)=setup("pc");db.create_order(ProfileKind::Vsd,v[2],"O","",0,1).unwrap();
        assert!(precheck(&db,ProfileKind::Vsd,false,[None,None]).is_empty());
        let w=precheck(&db,ProfileKind::Vsd,true,[Some(12.),None]);assert!(w.iter().any(|x|x.contains("niet gekalibreerd (hardware)")),"{w:?}");
        let w=precheck(&db,ProfileKind::Vsd,false,[Some(500.),None]);assert!(w.iter().any(|x|x.contains("lifthoogte LH1")),"{w:?}");
        drop(db);crate::recipe::tests::cleanup(&dir);
    }
}
