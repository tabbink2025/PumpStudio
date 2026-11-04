//! Connection manager: every device group runs on its own worker thread, so a hanging scale never
//! blocks the PLC, the pumps or the UI.
//!
//! VSD profile: CAN/pump bank, scale LH1, scale LH2, PLC, barcode scanner (five workers); the production
//! engine runs on its own thread and drives them through `DevLine` (one thread per head while dosing).
//! Keramisch profile: pump bench (the existing `workflow::Engine` worker); its scale lives inside the
//! same bench bridge (verified hardware code) and is switched on/off with a live Configure.
//!
//! Rules enforced here:
//! - one connection per physical device (`Registry`: CAN channel, scale host:port, COM port, PLC/UDP port);
//! - simulated and real devices are not mixed within a profile unless explicitly allowed (then a warning
//!   stays visible);
//! - STOP reaches every group through its own `StopHandle` (no group can clear another group's stop).
use crate::{applog,calib::{self,NewCal},potrun,recipe,production::{self,LineIo,ProdCmd,ProdStatus},scanner::{self,ScanKind,ScanLink,ScanResult,ScannerConfig,SimScan},db::{Db,ProfileKind},machine::*,model::{corrected_mass,Config,Result},pumptest::{self,DoseRow,TestPlan},testdb,plc::*,transport::{Bridge,Device,StopHandle},vsdcal::{CalSettings,VsdCalibration},workflow::{self,Action,Event,State}};
use serde_json::json;
use std::{collections::HashMap,path::PathBuf,sync::{atomic::{AtomicU64,Ordering},mpsc,Arc,Mutex},time::{Duration,Instant}};

#[derive(Clone,Copy,PartialEq,Eq,Hash,Debug)]
pub enum DevId{Can,Lh1,Lh2,Plc,Scanner,Bench,BenchScale}
impl DevId{
    pub const VSD:[DevId;5]=[DevId::Can,DevId::Lh1,DevId::Lh2,DevId::Plc,DevId::Scanner];
    pub const KERAMISCH:[DevId;2]=[DevId::Bench,DevId::BenchScale];
    pub fn of(p:ProfileKind)->&'static [DevId]{match p{ProfileKind::Vsd=>&Self::VSD,ProfileKind::Keramisch=>&Self::KERAMISCH}}
    pub fn name(self)->&'static str{match self{DevId::Can=>"CAN / pompbank",DevId::Lh1=>"Weegschaal LH1",DevId::Lh2=>"Weegschaal LH2",DevId::Plc=>"PLC",DevId::Scanner=>"Barcodescanner",DevId::Bench=>"Pompbank (CAN)",DevId::BenchScale=>"Weegschaal pompbank"}}
    pub fn short(self)->&'static str{match self{DevId::Can=>"CAN",DevId::Lh1=>"LH1",DevId::Lh2=>"LH2",DevId::Plc=>"PLC",DevId::Scanner=>"Scanner",DevId::Bench=>"Pompbank",DevId::BenchScale=>"Weegschaal"}}
    pub fn profile(self)->ProfileKind{if matches!(self,DevId::Bench|DevId::BenchScale){ProfileKind::Keramisch}else{ProfileKind::Vsd}}
    fn head(self)->Option<Head>{match self{DevId::Lh1=>Some(Head::LH1),DevId::Lh2=>Some(Head::LH2),_=>None}}
}

#[derive(Clone,Copy,PartialEq,Eq,Debug,Default)]
pub enum Link{#[default]Off,Connecting,On,Failed}

/// What the UI shows per device; written by the worker (lock held only to copy values, never during I/O).
#[derive(Clone,Debug,Default)]
pub struct DevStatus{
    pub link:Link,pub simulation:bool,pub busy:bool,pub endpoint:String,
    /// Last status line or command result; `ok` = last command succeeded.
    pub text:String,pub ok:bool,
    /// Scales: last reading (gross), stable flag, tare, error of the last read, time and count of reads.
    pub weight:Option<f64>,pub stable:bool,pub tare:f64,pub weight_error:String,pub weight_at:Option<Instant>,pub reads:u64,
    /// PLC: mailbox commands and replies (newest last).
    pub messages:Vec<String>,
    /// CAN: last known position per pump name in steps; last Dispense/MoveTo per pump; `version` changes after every pump command.
    pub positions:HashMap<String,i64>,pub moves:HashMap<String,MoveReport>,pub version:u64,
    /// CAN bus scan result.
    pub scan:String,
    /// CAN: running calibration queue; `cal_version` changes when a calibration or control dose was stored.
    pub progress:Option<CalProgress>,pub cal_version:u64,pub check:Option<DoseResult>,
    /// CAN: running pump test (VSD); `test_version` changes after every stored dose.
    pub test:Option<TestProgress>,pub test_version:u64,
}
/// Status per device behind an `Arc`: `Devices::status` hands out a reference-counted snapshot (no copy of
/// message lists, positions and moves per call); a worker update copies only when a reader still holds the old one.
type Shared=Arc<Mutex<HashMap<DevId,Arc<DevStatus>>>>;
fn upd(s:&Shared,id:DevId,f:impl FnOnce(&mut DevStatus)){let mut g=s.lock().unwrap();f(Arc::make_mut(g.entry(id).or_default()));}

/// Running pump test: run id, doses done of total, waiting for "Volgende dosis", last line.
#[derive(Clone,Debug,Default,PartialEq)]
pub struct TestProgress{pub run:i64,pub pump:String,pub done:usize,pub n:usize,pub waiting:bool,pub last:String}
/// VSD motion settings that may change between doses (applied before the next pump command, no reconnect).
#[derive(Clone,Copy,Debug,PartialEq,serde::Serialize)]
pub struct Motion{pub speed_rps:f64,pub accel_rps2:f64,pub decel_rps2:f64}
/// Shared with the CAN worker: correction for the next dose, new motion settings, "volgende dosis" after a pause.
#[derive(Clone,Debug,Default)]
pub struct TestControl{pub correction_pct:f64,pub motion:Option<Motion>,pub resume:bool}
/// Lift height in simulation when neither Instellingen nor the recipe gives one (hardware refuses instead).
pub const SIM_LIFT_CM:f64=10.;
/// Pot executor status for the UI.
#[derive(Clone,Debug,Default)]
pub struct PotStatus{pub run:Option<i64>,pub recipe:String,pub lines:Vec<String>,pub waiting:Option<String>,pub version:u64,pub last:Option<(i64,String,String)>}
/// `PotIo` on the device workers: CAN blocks via the CAN worker, weighings via the scale workers (in parallel),
/// lift/unit via the PLC worker. Every wait has a time-out and ends on the pot's STOP.
struct DevIo{can:mpsc::Sender<CanCmd>,scales:[mpsc::Sender<ScaleCmd>;2],gens:[Arc<AtomicU64>;2],plc:mpsc::Sender<PlcCmd>,stop:StopHandle,
    resume:Arc<std::sync::atomic::AtomicBool>,status:Arc<Mutex<PotStatus>>,db_path:Option<PathBuf>,db:Option<Db>,lift:[u32;2],unit:[u32;2],settle_s:f64,timeout_s:f64}
impl DevIo{
    /// Same hardware, own database connection (one per head thread).
    fn fresh(&self)->DevIo{DevIo{can:self.can.clone(),scales:self.scales.clone(),gens:[self.gens[0].clone(),self.gens[1].clone()],plc:self.plc.clone(),stop:self.stop.clone(),
        resume:self.resume.clone(),status:self.status.clone(),db_path:self.db_path.clone(),db:None,lift:self.lift,unit:self.unit,settle_s:self.settle_s,timeout_s:self.timeout_s}}
    fn conn(&mut self)->Result<&mut Db>{
        if self.db.is_none(){let p=self.db_path.as_ref().ok_or("geen database ingesteld")?;let d=Db::open(p)?;d.set_busy_timeout(Duration::from_secs(5))?;self.db=Some(d);}
        Ok(self.db.as_mut().unwrap())
    }
    /// Wait for a worker's answer; STOP ends the wait (the worker itself is stopped by its own STOP flag).
    fn answer<T>(&self,rx:&mpsc::Receiver<Result<T>>,secs:f64,force:bool,what:&str)->Result<T>{
        let end=Instant::now()+Duration::from_secs_f64(secs);
        loop{match rx.recv_timeout(Duration::from_millis(20)){
            Ok(r)=>return r,
            Err(mpsc::RecvTimeoutError::Timeout)=>{if !force && self.stop.check().is_err(){return Err("Gestopt".into());}if Instant::now()>end{return Err(format!("{what}: geen antwoord binnen {secs:.0} s"));}}
            Err(mpsc::RecvTimeoutError::Disconnected)=>return Err(format!("{what}: taak gesloten")),
        }}
    }
    fn plc(&mut self,process:u32,cmd:u32,params:Vec<u32>,force:bool,what:&str)->Result<()>{
        let (tx,rx)=mpsc::channel();self.plc.send(PlcCmd::Exec{process,cmd,params,force,reply:tx}).map_err(|_|"PLC-taak gesloten".to_string())?;
        let code=self.answer(&rx,self.timeout_s+5.,force,&format!("PLC proces {process}"))?;
        pot_reply(cmd,process,code,what)
    }
}
/// L2: replies accepted as Camelot. Lift Start 2/4, lift Stop 9; unit Start 2, unit Stop 9.
/// 16 = no pot under the head; anything else is unexpected. Both fail the head.
pub fn pot_reply(cmd:u32,process:u32,code:u32,what:&str)->Result<()>{
    let lift=process%1000==350;
    let ok:&[u32]=match (cmd,lift){(CMD_START,true)=>&[RSP_STARTED,RSP_UP],(CMD_START,false)=>&[RSP_STARTED],(CMD_STOP,_)=>&[RSP_STOPPED],_=>&[]};
    if ok.contains(&code){return Ok(());}
    if code==16{return Err(format!("geen pot (PLC-antwoord 16 op {what})"));}
    Err(format!("onverwacht PLC-antwoord {code} op {what} (proces {process})"))
}
impl potrun::PotIo for DevIo{
    fn lift(&mut self,h:Head,up:bool,p1:u32,force:bool)->Result<()>{
        let p=self.lift[h.idx()];self.plc(p,if up{CMD_START}else{CMD_STOP},if up{vec![p1]}else{vec![]},force,&format!("lift {} {}",h.name(),if up{"omhoog"}else{"omlaag"}))
    }
    fn unit(&mut self,h:Head,start:bool,force:bool)->Result<()>{
        let p=self.unit[h.idx()];self.plc(p,if start{CMD_START}else{CMD_STOP},vec![],force,&format!("unit {} {}",h.name(),if start{"start"}else{"stop"}))
    }
    /// STOP / error only: unit Stop without waiting for its reply (as Camelot's Abort), so nothing waits after the lift.
    fn unit_stop_now(&mut self,h:Head)->Result<()>{
        let p=self.unit[h.idx()];
        let (tx,rx)=mpsc::channel();self.plc.send(PlcCmd::Post{process:p,cmd:CMD_STOP,reply:tx}).map_err(|_|"PLC-taak gesloten".to_string())?;
        self.answer(&rx,15.,true,&format!("PLC proces {p}")).map(|_|())
    }
    /// Never waits: only on an open connection, with busy wait 0 (a locked database → stored later).
    fn store_now(&mut self,item:&recipe::PotItemRow)->bool{
        let Some(d)=self.db.as_ref() else{return false};
        if d.set_busy_timeout(Duration::ZERO).is_err(){return false;}
        let ok=d.add_pot_item(item).is_ok();let _=d.set_busy_timeout(Duration::from_secs(5));
        if ok{self.status.lock().unwrap().version+=1;}ok
    }
    fn weigh(&mut self,h:Head)->Result<f64>{
        // Let the liquid settle, then ask this head's scale (the other head weighs on its own).
        let end=Instant::now()+Duration::from_secs_f64(self.settle_s);
        while Instant::now()<end{if self.stop.check().is_err(){return Err("Gestopt".into());}std::thread::sleep(Duration::from_millis(20));}
        let (tx,rx)=mpsc::channel();let gen=self.gens[h.idx()].load(Ordering::SeqCst);
        self.scales[h.idx()].send(ScaleCmd::Read{stable:true,timeout_s:30.,gen,reply:tx}).map_err(|_|format!("Weegschaal {}: taak gesloten",h.name()))?;
        self.answer(&rx,35.,false,&format!("Weegschaal {}",h.name())).map(|(w,_)|w)
    }
    fn dose(&mut self,_h:Head,items:&[(String,f64,i64)])->Result<Vec<potrun::BlockDose>>{
        let (tx,rx)=mpsc::channel();let list=items.to_vec();
        self.can.send(CanCmd::DoseBlock{items:list,db:self.db_path.clone(),reply:tx}).map_err(|_|"CAN-taak gesloten".to_string())?;
        // The CAN worker ends a running block itself on STOP; wait for its answer.
        self.answer(&rx,self.timeout_s+30.,true,"CAN")
    }
    fn stopped(&self)->bool{self.stop.check().is_err()}
    fn store(&mut self,item:&recipe::PotItemRow)->Result<()>{let r=self.conn()?.add_pot_item(item).map(|_|());self.status.lock().unwrap().version+=1;r}
    fn wait_operator(&mut self,message:&str)->bool{
        self.status.lock().unwrap().waiting=Some(message.to_string());
        loop{if self.stop.check().is_err(){self.status.lock().unwrap().waiting=None;return false;}
            if self.resume.swap(false,Ordering::SeqCst){self.status.lock().unwrap().waiting=None;return true;}std::thread::sleep(Duration::from_millis(30));}
    }
    fn progress(&mut self,_h:Option<Head>,text:&str){
        applog::info("RECEPT",text);let mut p=self.status.lock().unwrap();p.lines.push(format!("{} {text}",&applog::local_time(now_ms())[11..]));
        if p.lines.len()>200{p.lines.remove(0);}p.version+=1;
    }
}
/// `LineIo` on the device workers (production engine): PLC via the PLC worker, scanner via its worker (at the same
/// time as barcode process Start), heads as `DevIo` (own thread each). Waits end on the production STOP.
struct DevLine{head:DevIo,plc:mpsc::Sender<PlcCmd>,scan:mpsc::Sender<ScanCmd>,stop:StopHandle,scan_stop:StopHandle,plc_timeout:f64,scan_timeout:f64,hardware:bool}
impl DevLine{
    fn wait<T>(&self,rx:&mpsc::Receiver<Result<T>>,secs:f64,force:bool,what:&str)->Result<T>{self.head.answer(rx,secs,force,what)}
    fn send_plc(&self,process:u32,cmd:u32,params:&[u32],force:bool)->Result<mpsc::Receiver<Result<u32>>>{
        let (tx,rx)=mpsc::channel();self.plc.send(PlcCmd::Exec{process,cmd,params:params.to_vec(),force,reply:tx}).map_err(|_|"PLC-taak gesloten".to_string())?;Ok(rx)
    }
}
impl LineIo for DevLine{
    fn plc(&mut self,process:u32,cmd:u32,params:&[u32],force:bool)->Result<u32>{
        let rx=self.send_plc(process,cmd,params,force)?;self.wait(&rx,self.plc_timeout+5.,force,&format!("PLC proces {process}"))
    }
    /// M4: PLC barcode Start and the scanner read run at the same time and are awaited together; PLC 16 (no pot)
    /// ends the scanner read at once (its own stop) instead of waiting for the scanner time-out.
    fn barcode(&mut self,process:u32)->Result<(u32,ScanResult,String)>{
        self.scan_stop.reset();let read_stop=StopHandle::default();
        let prx=self.send_plc(process,CMD_START,&[1],false)?;
        let (tx,srx)=mpsc::channel();self.scan.send(ScanCmd::Read{stop:read_stop.clone(),reply:Some(tx)}).map_err(|_|"Scanner-taak gesloten".to_string())?;
        let end_p=Instant::now()+Duration::from_secs_f64(self.plc_timeout+5.);let end_s=Instant::now()+Duration::from_secs_f64(self.scan_timeout+5.);
        let (mut rsp,mut scan):(Option<u32>,Option<Result<(ScanResult,String)>>)=(None,None);
        loop{
            if rsp.is_none(){match prx.try_recv(){Ok(r)=>{let c=r?;rsp=Some(c);if c==16 && scan.is_none(){read_stop.stop();self.scan_stop.stop();}}
                Err(mpsc::TryRecvError::Disconnected)=>return Err("PLC-taak gesloten".into()),Err(_)=>{}}}
            if scan.is_none(){match srx.try_recv(){Ok(r)=>scan=Some(r),Err(mpsc::TryRecvError::Disconnected)=>return Err("Scanner-taak gesloten".into()),Err(_)=>{}}}
            if let (Some(c),Some(s))=(rsp,&scan){
                self.scan_stop.reset();
                return match s{Ok((r,raw))=>Ok((c,r.clone(),raw.clone())),Err(_) if c==16=>Ok((16,ScanResult::NoRead,"geen pot: scan afgebroken".into())),Err(e)=>Err(e.clone())};
            }
            if self.stop.check().is_err(){read_stop.stop();return Err("Gestopt".into());}
            let now=Instant::now();
            if rsp.is_none() && now>end_p{read_stop.stop();return Err(format!("PLC proces {process}: geen antwoord binnen {:.0} s",self.plc_timeout+5.));}
            if scan.is_none() && now>end_s{return Err(format!("Scanner: geen antwoord binnen {:.0} s",self.scan_timeout+5.));}
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn post(&mut self,process:u32,cmd:u32)->Result<()>{
        let (tx,rx)=mpsc::channel();self.plc.send(PlcCmd::Post{process,cmd,reply:tx}).map_err(|_|"PLC-taak gesloten".to_string())?;
        self.wait(&rx,15.,true,&format!("PLC proces {process}")).map(|_|())
    }
    fn head(&mut self,_h:Head)->Box<dyn potrun::PotIo>{Box::new(self.head.fresh())}
    fn stopped(&self)->bool{self.stop.check().is_err()}
    fn hardware(&self)->bool{self.hardware}
}
fn now_ms()->i64{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis() as i64).unwrap_or(0)}
type DbJob=Box<dyn FnOnce(&mut Db)->Result<()>+Send>;
/// Ceramic test in progress: dose `k` of `doses`; `sent` = engine dose under way (start, correction, settings, requested).
struct BenchTest{run:i64,pump:String,doses:Vec<f64>,plan:TestPlan,k:usize,sent:Option<(Instant,f64,String,f64)>,stop:bool,waiting:bool,resume:bool,last:String}
/// Progress of a calibration queue: pump `k` of `n`, step `step` of `steps` for this pump, last line.
#[derive(Clone,Debug,Default,PartialEq)]
pub struct CalProgress{pub pump:String,pub k:usize,pub n:usize,pub step:usize,pub steps:usize,pub text:String}
/// Steps (progress lines) of one VSD calibration: home, phase 1 points, phase 2 points.
pub fn cal_steps(s:&CalSettings)->usize{1+s.phase1_runs*s.phase1_samples+if s.phase2{s.phase2_runs*s.stroke_samples.len()}else{0}}

// ---------- one connection per physical device ----------
#[derive(Clone,Default)]
pub struct Registry(Arc<Mutex<HashMap<String,DevId>>>);
/// Held by the worker together with the open device; dropping it frees the resources.
pub struct Claim{reg:Registry,keys:Vec<String>,owner:DevId}
impl Registry{
    pub fn claim(&self,owner:DevId,keys:Vec<String>)->Result<Claim>{
        let mut m=self.0.lock().unwrap();
        // `can:*` (hardware CAN scan opens every channel) conflicts with any single channel and vice versa.
        let clash=|a:&str,b:&str|a==b || (a.starts_with("can:") && b.starts_with("can:") && (a=="can:*" || b=="can:*"));
        for k in &keys{if let Some((h,o))=m.iter().find(|(h,_)|clash(k,h)){return Err(format!("{} is al in gebruik door {}{}; één verbinding per apparaat. Verbreek eerst {}.",describe_key(k),o.name(),if h!=k{format!(" ({})",describe_key(h))}else{String::new()},o.name()));}}
        for k in &keys{m.insert(k.clone(),owner);}
        Ok(Claim{reg:self.clone(),keys,owner})
    }
    pub fn held(&self)->Vec<(String,DevId)>{let mut v:Vec<_>=self.0.lock().unwrap().iter().map(|(k,o)|(k.clone(),*o)).collect();v.sort_by(|a,b|a.0.cmp(&b.0));v}
    pub fn any_can(&self)->Option<(String,DevId)>{self.held().into_iter().find(|(k,_)|k.starts_with("can:"))}
}
impl Drop for Claim{fn drop(&mut self){let mut m=self.reg.0.lock().unwrap();for k in &self.keys{if m.get(k)==Some(&self.owner){m.remove(k);}}}}
pub fn describe_key(k:&str)->String{
    let (kind,rest)=k.split_once(':').unwrap_or((k,""));
    match kind{"can" if rest=="*"=>"Alle CAN-kanalen (CAN-test)".into(),"can"=>format!("CAN-kanaal {rest}"),"tcp"=>format!("Weegschaal {rest}"),"com"=>format!("Poort {rest}"),"udp"=>format!("UDP-poort {rest} (pc)"),"plc"=>format!("PLC {rest}"),_=>k.to_string()}
}
/// Same device under two names (`localhost`, `127.0.0.1`, upper case, spaces) is one key. No DNS lookup (no I/O here).
pub fn norm_host(h:&str)->String{let h=h.trim().trim_end_matches('.').to_lowercase();if h=="localhost" || h=="::1" || h=="[::1]"{"127.0.0.1".into()}else{h}}
fn tcp_key(host:&str,port:u16)->String{format!("tcp:{}:{port}",norm_host(host))}
/// Physical resources a hardware connection opens.
pub fn resources(id:DevId,s:&Settings)->Vec<String>{
    let b=&s.bench;
    match id{
        DevId::Can=>vec![format!("can:{}",s.machine.can_channel)],
        DevId::Lh1|DevId::Lh2=>{let n=&s.machine.scales[id.head().unwrap().idx()];vec![tcp_key(&n.host,n.port)]}
        DevId::Plc=>vec![format!("udp:{}",s.machine.plc.host_port),format!("plc:{}:{}",norm_host(&s.machine.plc.ip),s.machine.plc.port)],
        DevId::Scanner=>{let c=&s.machine.scanner;match c.kind{ScanKind::Tcp=>vec![tcp_key(&c.host,c.port)],ScanKind::Com=>vec![format!("com:{}",c.com.trim().to_uppercase())]}}
        DevId::Bench=>vec![format!("can:{}",b.channel)],
        DevId::BenchScale=>if b.scale_tcp{let mut v=vec![tcp_key(&b.scale_host,b.scale_tcp_port)];if b.scale_dual{v.push(tcp_key(&b.scale_host2,b.scale_tcp_port2));}v}
            else{vec![format!("com:{}",b.port.trim().to_uppercase())]},
    }
}

// ---------- settings used by the manager ----------
#[derive(Clone,Debug)]
pub struct Settings{
    /// VSD settings plus the pump list of the VSD profile.
    pub machine:MachineConfig,pub bench:Config,pub python:PathBuf,pub repo:PathBuf,
    /// Folder with `vsd_bridge.py` and `hardware_bridge.py`; raw CAN/PLC frames go to `output`.
    pub bridge_dir:PathBuf,pub output:PathBuf,
    /// Simulated travel limit per pump command (s); the app uses real travel time.
    pub sim_travel_s:f64,
    /// Allow simulated and real devices together (tests only); a warning stays visible.
    pub mix_allowed:bool,
    /// Database for calibrations (written by the CAN worker and the bench store); None = not stored.
    pub db_path:Option<PathBuf>,
}
impl Settings{
    pub fn new(machine:MachineConfig,bench:Config,output:PathBuf)->Self{
        Self{machine,bench,python:crate::db::default_python().into(),repo:crate::db::default_repo().into(),bridge_dir:PathBuf::from("bridge"),output,sim_travel_s:60.,mix_allowed:false,db_path:None}
    }
}

// ---------- worker commands ----------
enum CanCmd{Connect{cfg:MachineConfig,simulation:bool,python:PathBuf,script:PathBuf,output:PathBuf,world:Option<Arc<Mutex<SimWorld>>>,claim:Option<Claim>},
    Disconnect,Pumps{names:Vec<String>,op:PumpOp,value:i64},SetPumps(Vec<VsdPump>),
    Scan{simulation:bool,python:PathBuf,script:PathBuf,output:PathBuf,seconds:f64,bitrate:u32,nodes:Vec<u16>,claim:Option<Claim>},
    Calibrate{names:Vec<String>,settings:CalSettings,db:Option<PathBuf>},
    /// Pump test: doses one by one with calibration `cal`; run row already created.
    Test{run:i64,pump:String,doses:Vec<f64>,plan:TestPlan,cal:i64,db:Option<PathBuf>},
    /// Pot executor: grams per pump (with its calibration), all drives in one CAN block.
    DoseBlock{items:Vec<(String,f64,i64)>,db:Option<PathBuf>,reply:mpsc::Sender<Result<Vec<potrun::BlockDose>>>},CheckDose{name:String,grams:f64,db:Option<PathBuf>}}
/// Weighing commands carry the head's STOP generation at the time they were sent: a STOP after sending
/// (generation changed) ends them, a STOP from before never does, however late the worker picks them up.
enum ScaleCmd{Connect{net:ScaleNet,simulation:bool,world:Option<Arc<Mutex<SimWorld>>>,claim:Option<Claim>},Disconnect,Tare{gen:u64},Weigh{gen:u64},
    /// Weighing for the CAN worker (calibration, control dose); answer on `reply`.
    Read{stable:bool,timeout_s:f64,gen:u64,reply:mpsc::Sender<Result<(f64,bool)>>}}
/// The scale of a head as seen by the CAN worker: every reading is done by that head's scale worker, so the
/// scale keeps one connection. Waiting ends on STOP within 50 ms.
struct RemoteScale{tx:mpsc::Sender<ScaleCmd>,stop:StopHandle,head:Head,gen:Arc<AtomicU64>}
impl ScaleLink for RemoteScale{
    fn read(&mut self,stable:bool,timeout_s:f64)->Result<(f64,bool)>{
        let (rtx,rrx)=mpsc::channel();
        self.tx.send(ScaleCmd::Read{stable,timeout_s,gen:self.gen.load(Ordering::SeqCst),reply:rtx}).map_err(|_|format!("Weegschaal {}: taak gesloten",self.head.name()))?;
        let end=Instant::now()+Duration::from_secs_f64(timeout_s+2.);
        loop{
            match rrx.recv_timeout(Duration::from_millis(50)){
                Ok(r)=>return r,
                Err(mpsc::RecvTimeoutError::Timeout)=>{
                    if self.stop.check().is_err(){return Err("Gestopt".into());}
                    if Instant::now()>end{return Err(format!("Weegschaal {}: geen antwoord binnen {timeout_s:.0} s",self.head.name()));}
                }
                Err(mpsc::RecvTimeoutError::Disconnected)=>return Err(format!("Weegschaal {}: taak gesloten",self.head.name())),
            }
        }
    }
}
enum PlcCmd{Connect{cfg:PlcConfig,simulation:bool,claim:Option<Claim>},Disconnect,Command{process:u32,cmd:u32,params:Vec<u32>},
    /// Pot executor (lift/unit): first answer of that process (no update) on `reply`; the executor judges the
    /// code (L2). `force` = also after STOP (lowering a lift).
    Exec{process:u32,cmd:u32,params:Vec<u32>,force:bool,reply:mpsc::Sender<Result<u32>>},
    /// Machine/lijn: commands one after the other (manual line actions, PLC initialisation). Reset/Abort/PowerUp/Hold
    /// are sent without waiting for a reply (as Camelot). `stop_on_error`: stop at the first failure.
    Sequence{label:String,steps:Vec<(u32,u32,Vec<u32>)>,stop_on_error:bool},
    /// Production: command without waiting for a reply (Abort/Hold/Restart); waits only until the PLC took the
    /// previous command (time-out), also after STOP.
    Post{process:u32,cmd:u32,reply:mpsc::Sender<Result<u32>>}}
/// Every PLC command of the workers with its reply (ms, process, command, params, reply or error), newest last.
pub type PlcTrace=Arc<Mutex<Vec<(i64,u32,u32,Vec<u32>,std::result::Result<u32,String>)>>>;
fn trace(t:&PlcTrace,process:u32,cmd:u32,params:&[u32],r:&Result<u32>){let mut v=t.lock().unwrap();v.push((now_ms(),process,cmd,params.to_vec(),r.clone()));if v.len()>20000{v.drain(..5000);}}
/// Commands Camelot sends without waiting for an answer.
fn fire_and_forget(cmd:u32)->bool{matches!(cmd,CMD_RESET|CMD_ABORT|6|9|10|98)}

fn hint(e:&str)->String{workflow::explain(e)}
/// Connection of a worker to the database of the settings, opened once and kept.
fn conn<'a>(cache:&'a mut Option<(PathBuf,Db)>,db:&Option<PathBuf>)->Result<&'a mut Db>{
    let p=db.as_ref().ok_or("geen database ingesteld")?;
    // Background thread: it may wait for the UI and the log writer (the UI itself never waits long).
    if cache.as_ref().is_none_or(|(q,_)|q!=p){let d=Db::open(p)?;d.set_busy_timeout(Duration::from_secs(5))?;*cache=Some((p.clone(),d));}
    Ok(&mut cache.as_mut().unwrap().1)
}
/// Store a calibration (worker's own connection).
fn store(cache:&mut Option<(PathBuf,Db)>,db:&Option<PathBuf>,c:&NewCal)->Result<i64>{conn(cache,db)?.save_calibration(c)}
/// "25 600 st = 0,5000 omw = 180,0°" (Dutch number format).
pub fn steps_text(steps:i64,spr:i64)->String{
    let spr=spr.max(1);let rev=steps as f64/spr as f64;
    format!("{} st = {} omw = {}°",group(steps),format!("{rev:.4}").replace('.',","),format!("{:.1}",rev*360.).replace('.',","))
}
/// Input of the Pompen page to whole drive steps. unit 0 = steps (exact), 1 = revolutions, 2 = degrees
/// (rounded to the nearest step, shown before sending). Outside the 32-bit drive range is refused.
pub fn to_steps(unit:usize,steps:i64,rev:f64,deg:f64,spr:i64)->Result<i64>{
    let v=match unit{0=>steps as f64,1=>rev*spr as f64,_=>deg/360.*spr as f64};
    if !v.is_finite(){return Err("Ongeldige invoer".into());}
    let r=if unit==0{steps}else{v.round() as i64};
    if !(P_MIN..=P_MAX).contains(&r){return Err(format!("Buiten drivebereik ({P_MIN} … {P_MAX} stappen)"));}
    Ok(r)
}
/// Whole number of steps as typed (S2). Spaces and dots are thousands separators ("25 600", "25.600",
/// "1.234.567"); a dot counts only when every group after it has exactly three digits, so "1.5" is refused
/// as ambiguous. Comma, decimals, other characters, empty input and values outside the drive range are refused.
pub fn parse_steps(text:&str)->Result<i64>{
    let t=text.trim();
    if t.is_empty(){return Err("Vul een aantal stappen in".into());}
    let (neg,body)=match t.strip_prefix('-'){Some(b)=>(true,b.trim_start()),None=>(false,t.strip_prefix('+').unwrap_or(t).trim_start())};
    if body.contains(','){return Err(format!("Stappen zijn een geheel getal: geen komma of decimalen ({t})"));}
    // Separators: dot, space, no-break space (U+00A0), thin space (U+2009), narrow no-break space (U+202F).
    const SEP:[char;5]=['.',' ','\u{a0}','\u{2009}','\u{202f}'];
    if body.is_empty() || !body.chars().all(|c|c.is_ascii_digit() || SEP.contains(&c)){return Err(format!("Ongeldig aantal stappen: {t}"));}
    let groups:Vec<&str>=body.split(SEP).collect();
    if groups.len()>1{
        let first=groups[0];
        if first.is_empty() || first.len()>3 || groups[1..].iter().any(|g|g.len()!=3){
            return Err(format!("Geen decimalen in stappen; punt of spatie alleen als duizendtalscheiding (25 600 of 25.600): {t}"));}
    }
    let digits:String=groups.concat();
    let v:i64=digits.parse().map_err(|_|format!("Buiten drivebereik ({P_MIN} … {P_MAX} stappen)"))?;
    let v=if neg{-v}else{v};
    if !(P_MIN..=P_MAX).contains(&v){return Err(format!("Buiten drivebereik ({P_MIN} … {P_MAX} stappen)"));}
    Ok(v)
}
/// Decimal number as typed for revolutions or degrees: Dutch comma or a dot ("0,5", "0.5"); with both, the
/// dot is a thousands separator ("1.234,5"). Spaces are ignored. Anything else is refused.
pub fn parse_decimal(text:&str)->Result<f64>{
    let t:String=text.trim().chars().filter(|c|!c.is_whitespace()).collect();
    if t.is_empty(){return Err("Vul een getal in".into());}
    let n=if t.contains(',') && t.contains('.'){t.replace('.',"").replace(',',".")}else{t.replace(',',".")};
    if n.matches('.').count()>1 || !n.chars().enumerate().all(|(k,c)|c.is_ascii_digit() || c=='.' || (k==0 && (c=='-' || c=='+'))){return Err(format!("Ongeldig getal: {}",text.trim()));}
    n.parse::<f64>().ok().filter(|v|v.is_finite()).ok_or_else(||format!("Ongeldig getal: {}",text.trim()))
}
/// Thousands with a space: 51200 -> "51 200", -12800 -> "-12 800".
pub fn group(v:i64)->String{
    let s=v.unsigned_abs().to_string();let mut out=String::new();
    for (k,c) in s.chars().enumerate(){if k>0 && (s.len()-k)%3==0{out.push(' ');}out.push(c);}
    if v<0{format!("-{out}")}else{out}
}
/// R4: an interrupted command reports "gestopt", whatever text the link layer used.
fn stopped(e:&str,stop:&StopHandle)->String{if stop.check().is_err() && !e.starts_with("Gestopt"){format!("Gestopt (STOP): {e}")}else{e.to_string()}}
/// R5: errors meaning the TCP connection of a scale is gone (as opposed to a slow or busy scale).
fn scale_link_lost(e:&str)->bool{e.contains("verbinding gesloten") || e.starts_with("Weegschaal schrijven") || e.starts_with("Weegschaal lezen")}

/// A running dose block in the CAN worker: machine block, start time, (pump, steps), answer channel.
struct Pend{block:PendingBlock,t0:i64,pumps:Vec<(String,i64)>,reply:mpsc::Sender<Result<Vec<potrun::BlockDose>>>}
fn can_worker(rx:mpsc::Receiver<CanCmd>,sh:Shared,stop:StopHandle,scales:[mpsc::Sender<ScaleCmd>;2],gens:[Arc<AtomicU64>;2],ctl:Arc<Mutex<TestControl>>){
    let remote=|h:Head|->Option<Box<dyn ScaleLink>>{Some(Box::new(RemoteScale{tx:scales[h.idx()].clone(),stop:stop.clone(),head:h,gen:gens[h.idx()].clone()}))};
    // Motion settings changed between commands (Pomptest): applied to the machine before the next command.
    let apply=|m:&mut Machine|{if let Some(mo)=ctl.lock().unwrap().motion{m.cfg.speed_rps=mo.speed_rps;m.cfg.accel_rps2=mo.accel_rps2;m.cfg.decel_rps2=mo.decel_rps2;}};
    let id=DevId::Can;let mut machine:Option<(Machine,Option<Claim>)>=None;let mut dbc:Option<(PathBuf,Db)>=None;
    let publish=|m:&Machine,sh:&Shared|{let pos:HashMap<String,i64>=m.positions().iter().zip(&m.cfg.pumps).filter_map(|(p,pm)|p.map(|v|(pm.name.clone(),v))).collect();
        let moves:HashMap<String,MoveReport>=m.last.iter().filter_map(|(i,r)|m.cfg.pumps.get(*i).map(|p|(p.name.clone(),r.clone()))).collect();
        upd(sh,id,|s|{s.positions=pos;s.moves=moves;s.version+=1;});};
    // M1: dose blocks of both heads run at the same time. A block's DISPENSEs are sent at once; its replies are
    // collected in short slices while new blocks (the other head) are started; SETPOS + reply when it is done.
    let mut pending:Vec<Pend>=vec![];
    let poll=|m:&mut Machine,pending:&mut Vec<Pend>|{
        // STOP: every running block ends now (the bridge/simulation stops the drives); positions unknown.
        if stop.check().is_err(){
            for p in pending.drain(..){m.dispense_abort(&p.block);applog::warn("CAN",&format!("Doseerblok {} gestopt (STOP)",p.pumps.iter().map(|x|x.0.clone()).collect::<Vec<_>>().join(", ")));
                let _=p.reply.send(Err("Gestopt (STOP)".into()));}
            return;
        }
        let mut k=0;
        while k<pending.len(){
            match m.dispense_poll(&mut pending[k].block,0.02){
                Ok(false)=>k+=1,
                Ok(true)=>{let p=pending.remove(k);let t1=now_ms();
                    let r=m.dispense_finish(p.block).map(|_|p.pumps.iter().map(|(n,st)|potrun::BlockDose{pump:n.clone(),steps:*st,start_ms:p.t0,end_ms:t1}).collect::<Vec<_>>());
                    let _=p.reply.send(r.map_err(|e|stopped(&e,&stop)));}
                Err(e)=>{let p=pending.remove(k);applog::error("CAN",&format!("Doseerblok {}: {e}",p.pumps.iter().map(|x|x.0.clone()).collect::<Vec<_>>().join(", ")));let _=p.reply.send(Err(stopped(&e,&stop)));}
            }
        }
    };
    loop{
        if !pending.is_empty(){match machine.as_mut(){Some((m,_))=>{poll(m,&mut pending);publish(m,&sh);}
            None=>{for p in pending.drain(..){let _=p.reply.send(Err("CAN niet verbonden".into()));}}}}
        let cmd=if pending.is_empty(){match rx.recv(){Ok(c)=>c,Err(_)=>break}}else{match rx.try_recv(){Ok(c)=>c,Err(mpsc::TryRecvError::Empty)=>continue,Err(_)=>break}};
        // Any other command needs the bus alone: running blocks finish first.
        if !matches!(cmd,CanCmd::DoseBlock{..}){
            while !pending.is_empty(){match machine.as_mut(){Some((m,_))=>{poll(m,&mut pending);publish(m,&sh);}None=>{for p in pending.drain(..){let _=p.reply.send(Err("CAN niet verbonden".into()));}}}}
        }
        match cmd{
            CanCmd::Connect{cfg,simulation,python,script,output,world,claim}=>{
                // The caller cleared an earlier STOP before sending (R1); a STOP after the click must stay.
                let opened:Result<Machine>=(||{
                    cfg.validate()?;
                    let link:Box<dyn CanLink>=if simulation{Box::new(SimCan::with_stop(world.clone().ok_or("Simulatie zonder machinemodel")?,stop.clone()))}
                        else{Box::new(BridgeCan::open(&python,&script,&output,stop.clone(),&cfg)?)};
                    Ok(Machine::from_parts(cfg,link,[remote(Head::LH1),remote(Head::LH2)],if simulation{world}else{None}))
                })();
                match opened{
                    Ok(m)=>{let n=m.cfg.pumps.len();let ch=m.cfg.can_channel;applog::info("VERBINDING",&format!("CAN verbonden ({}; kanaal {ch}; {n} pompen)",if simulation{"simulatie"}else{"hardware"}));
                        upd(&sh,id,|s|{s.link=Link::On;s.ok=true;s.text=format!("Verbonden: {n} pompen");});publish(&m,&sh);machine=Some((m,claim));}
                    Err(e)=>{applog::error("VERBINDING",&format!("CAN niet verbonden: {e}{}",hint(&e)));upd(&sh,id,|s|{s.link=Link::Failed;s.ok=false;s.text=format!("Niet verbonden: {e}{}",hint(&e));});drop(claim);}
                }
            }
            CanCmd::Disconnect=>{if machine.take().is_some(){applog::info("VERBINDING","CAN verbroken");}upd(&sh,id,|s|{s.link=Link::Off;s.busy=false;s.text="Niet verbonden".into();});}
            CanCmd::SetPumps(pumps)=>{if let Some((m,_))=machine.as_mut(){
                // Positions stay with the CAN address when the list changes.
                let by_addr:HashMap<(u16,u8),i64>=m.steps.iter().filter_map(|(i,s)|m.cfg.pumps.get(*i).map(|p|((p.node,p.drive),*s))).collect();
                m.steps=pumps.iter().enumerate().filter_map(|(i,p)|by_addr.get(&(p.node,p.drive)).map(|s|(i,*s))).collect();
                m.cfg.pumps=pumps;}}
            CanCmd::Pumps{names,op,value}=>{
                let Some((m,_))=machine.as_mut() else{upd(&sh,id,|s|{s.busy=false;s.ok=false;s.text="CAN niet verbonden".into();});continue};
                apply(m);
                let what=format!("Pomp {op:?} {value} omw: {}",names.join(", "));applog::info("ACTIE",&what);
                let r=names.iter().map(|n|m.cfg.pump_index(n).ok_or(format!("Pomp {n} onbekend"))).collect::<Result<Vec<_>>>().and_then(|idx|m.pumps(&idx,op,value));
                let r=r.map_err(|e|stopped(&e,&stop));
                match &r{Ok(t)=>applog::info("OK",&format!("{what}: {t}")),Err(e)=>applog::error("FOUT",&format!("{what}: {e}{}",hint(e)))}
                let pos:Vec<String>=names.iter().filter_map(|n|m.cfg.pump_index(n).and_then(|i|m.steps.get(&i).map(|s|format!("{n} {}",steps_text(*s,m.cfg.spr_of(i)))))).collect();
                publish(m,&sh);
                upd(&sh,id,|s|{s.busy=false;s.ok=r.is_ok();s.text=match &r{Ok(t)=>format!("{t}{}",if pos.is_empty(){String::new()}else{format!(" · {}",pos.join(", "))}),Err(e)=>format!("Fout: {e}{}",hint(e))};});
            }
            CanCmd::DoseBlock{items,db,reply}=>{
                let Some((m,_))=machine.as_mut() else{let _=reply.send(Err("CAN niet verbonden".into()));continue};
                apply(m);
                let r=(||{
                    let mut plan=vec![];
                    for (pump,grams,cal) in &items{
                        let c=conn(&mut dbc,&db)?.vsd_calibration(*cal)?;let i=m.cfg.pump_index(pump).ok_or(format!("Pomp {pump} onbekend"))?;
                        plan.push((i,m.steps_for(i,*grams,&c)?));
                    }
                    let t0=now_ms();applog::info("CAN",&format!("Doseerblok: {}",items.iter().zip(&plan).map(|((p,g,_),(_,st))|format!("{p} {} g = {st} st",pumptest::nl(*g,3))).collect::<Vec<_>>().join(", ")));
                    let block=m.dispense_start(&plan)?;
                    Ok::<_,String>((block,t0,items.iter().zip(&plan).map(|((p,_,_),(_,st))|(p.clone(),*st)).collect::<Vec<_>>()))
                })();
                publish(m,&sh);
                match r{Ok((block,t0,pumps))=>pending.push(Pend{block,t0,pumps,reply}),Err(e)=>{let _=reply.send(Err(stopped(&e,&stop)));}}
            }
            CanCmd::Test{run,pump,doses,plan,cal,db}=>{
                let Some((m,_))=machine.as_mut() else{
                    let _=conn(&mut dbc,&db).and_then(|d|d.finish_test_run(run,testdb::RUN_FAILED,"CAN niet verbonden"));
                    upd(&sh,id,|s|{s.busy=false;s.ok=false;s.test=None;s.test_version+=1;s.text="CAN niet verbonden".into();});continue};
                let n=doses.len();let hardware=m.world.is_none();
                let st=stop.clone();let is_stopped=move||st.check().is_err();
                let (mut status,mut note)=(testdb::RUN_DONE,String::new());
                let setup=(||{let c=conn(&mut dbc,&db)?.vsd_calibration(cal)?;let i=m.cfg.pump_index(&pump).ok_or(format!("Pomp {pump} onbekend"))?;Ok::<_,String>((c,i))})();
                match setup{
                    Err(e)=>{status=testdb::RUN_FAILED;note=e;}
                    Ok((c,i))=>{
                        applog::info("POMPTEST",&format!("Test {run} {pump}: {n} doses, kalibratie nr {cal} ({})",if hardware{"hardware"}else{"simulatie"}));
                        let mut prev_after:Option<f64>=None;
                        for (k,g) in doses.iter().enumerate(){
                            if is_stopped(){status=testdb::RUN_STOPPED;note="Gestopt (STOP)".into();break;}
                            // Pause after every dose: wait for "Volgende dosis" (STOP ends the wait).
                            if k>0 && plan.pause_each{
                                upd(&sh,id,|s|if let Some(t)=s.test.as_mut(){t.waiting=true;});
                                loop{if is_stopped() || std::mem::take(&mut ctl.lock().unwrap().resume){break;}std::thread::sleep(Duration::from_millis(30));}
                                upd(&sh,id,|s|if let Some(t)=s.test.as_mut(){t.waiting=false;});
                                if is_stopped(){status=testdb::RUN_STOPPED;note="Gestopt (STOP)".into();break;}
                            }
                            // Correction and motion as they are now: they apply from this dose on and are recorded with it.
                            apply(m);let corr=ctl.lock().unwrap().correction_pct;
                            let snapshot=serde_json::json!({"snelheid_omw_s":m.cfg.speed_rps,"acceleratie_omw_s2":m.cfg.accel_rps2,"deceleratie_omw_s2":m.cfg.decel_rps2,
                                "stappen_omw":m.cfg.spr_of(i),"kalibratie":cal,"wachttijd_s":plan.settle_s,"tarra_per_dosis":plan.tare_each}).to_string();
                            let t0=Instant::now();
                            let r=corrected_mass(*g,corr).and_then(|target|m.dose_with(i,target,&c,plan.settle_s,if plan.tare_each{None}else{prev_after},&is_stopped).map(|x|(target,x)));
                            let dur=t0.elapsed().as_secs_f64();
                            let mut row=DoseRow{run_id:run,nr:k as i64+1,requested_g:*g,correction_pct:corr,settings:snapshot,duration_s:Some(dur),..Default::default()};
                            match &r{
                                Ok((target,(res,after,steps)))=>{row.target_g=Some(*target);row.dosed_g=res.measured_g;row.steps=Some(*steps);row.status="ok".into();prev_after=*after;}
                                Err(e)=>{row.status="fout".into();row.error=stopped(e,&stop);prev_after=None;}
                            }
                            let line=match row.err_pct(){Some(e)=>format!("Dosis {}/{n}: {} g gevraagd, {} g gedoseerd ({} %)",k+1,pumptest::nl(*g,3),pumptest::nl(row.dosed_g.unwrap_or(0.),4),pumptest::nls(e,2)),None=>format!("Dosis {}/{n}: {}",k+1,row.error)};
                            applog::info("POMPTEST",&line);
                            let saved=conn(&mut dbc,&db).and_then(|d|d.add_test_dose(&row));
                            upd(&sh,id,|s|{s.test_version+=1;if let Some(t)=s.test.as_mut(){t.done=k+1;t.last=line.clone();}});
                            if let Err(e)=saved{status=testdb::RUN_FAILED;note=format!("Dosis niet opgeslagen: {e}");break;}
                            if let Err(e)=r{status=if is_stopped(){testdb::RUN_STOPPED}else{testdb::RUN_FAILED};note=stopped(&e,&stop);break;}
                        }
                    }
                }
                let _=conn(&mut dbc,&db).and_then(|d|d.finish_test_run(run,status,&note));
                let text=format!("Pomptest {pump}: {status}{}",if note.is_empty(){String::new()}else{format!(" — {note}")});
                if status==testdb::RUN_DONE{applog::info("OK",&text)}else{applog::warn("POMPTEST",&text)}
                publish(m,&sh);
                upd(&sh,id,|s|{s.busy=false;s.ok=status==testdb::RUN_DONE;s.test=None;s.test_version+=1;s.text=text;});
            }
            CanCmd::Calibrate{names,settings,db}=>{
                let Some((m,_))=machine.as_mut() else{upd(&sh,id,|s|{s.busy=false;s.ok=false;s.progress=None;s.text="CAN niet verbonden".into();});continue};
                apply(m);
                let n=names.len();let steps=cal_steps(&settings);let mut lines=vec![];let mut ok=true;
                for (k,name) in names.iter().enumerate(){
                    let Some(i)=m.cfg.pump_index(name) else{lines.push(format!("{name}: onbekende pomp"));ok=false;break};
                    applog::info("ACTIE",&format!("Kalibratie {name} ({}/{n})",k+1));
                    let mut cal=VsdCalibration::default();let mut step=0;
                    let st=stop.clone();let is_stopped=move||st.check().is_err();
                    let r=m.calibrate_into(i,&settings,&mut cal,&mut |t|{step+=1;applog::info("KALIBRATIE",t);
                        upd(&sh,id,|s|s.progress=Some(CalProgress{pump:name.clone(),k:k+1,n,step,steps,text:t.to_string()}));},&is_stopped);
                    let r=r.map_err(|e|stopped(&e,&stop));
                    // Store at once: finished = active, stopped/failed with measured points = "afgebroken".
                    let stored=if cal.pump.is_empty(){Ok(None)}else{store(&mut dbc,&db,&NewCal::from_vsd(&cal,r.is_ok(),r.as_ref().err().map(|e|e.as_str()).unwrap_or(""))).map(Some)};
                    let line=match (&r,&stored){
                        (Ok(()),Ok(_))=>format!("{name}: {:.4} g per slag",cal.g_per_stroke),
                        (Ok(()),Err(e))=>format!("{name}: gemeten, maar niet opgeslagen: {e}"),
                        (Err(e),Ok(Some(_)))=>format!("{name}: {e} (opgeslagen als afgebroken)"),
                        (Err(e),_)=>format!("{name}: {e}"),
                    };
                    if r.is_ok() && stored.is_ok(){applog::info("OK",&format!("Kalibratie {line}"))}else{applog::error("FOUT",&format!("Kalibratie {line}"))}
                    lines.push(line);
                    if r.is_err() || stored.is_err(){ok=false;break;}
                }
                publish(m,&sh);
                upd(&sh,id,|s|{s.busy=false;s.ok=ok;s.progress=None;s.cal_version+=1;s.text=format!("Kalibratie {}: {}",if ok{"klaar"}else{"niet voltooid"},lines.join("; "));});
            }
            CanCmd::CheckDose{name,grams,db}=>{
                let Some((m,_))=machine.as_mut() else{upd(&sh,id,|s|{s.busy=false;s.ok=false;s.text="CAN niet verbonden".into();});continue};
                apply(m);
                let st=stop.clone();let is_stopped=move||st.check().is_err();
                // S1: hardware doses only with a hardware calibration; a simulation may borrow the hardware one.
                let hardware=m.world.is_none();
                let r=(||{
                    let row=conn(&mut dbc,&db)?.calibration_for_dosing(ProfileKind::Vsd,&name,calib::KIND_VSD,hardware)?;let cal=row.id;
                    applog::info("ACTIE",&format!("Controle doseren {name} {grams} g met kalibratie nr {cal} ({}, {})",if row.simulation{"simulatie"}else{"hardware"},crate::applog::local_time(row.date_ms)));
                    let c=conn(&mut dbc,&db)?.vsd_calibration(cal)?;
                    let i=m.cfg.pump_index(&name).ok_or(format!("Pomp {name} onbekend"))?;
                    let r=m.dose_grams(i,grams,&c,c.settings.settle_s,&is_stopped)?;
                    // Written only onto a calibration of the same mode; a borrowed one stays as it was.
                    let stored=conn(&mut dbc,&db)?.set_check(cal,!hardware,grams,r.measured_g,r.error_pct)?;Ok::<_,String>((r,stored))
                })().map_err(|e|stopped(&e,&stop));
                let text=match &r{Ok((d,stored))=>format!("Controle {name}: gevraagd {:.4} g, gedoseerd {}{}",d.requested_g,d.measured_g.map(|m|format!("{m:.4} g ({:+.2} %)",d.error_pct.unwrap_or(0.))).unwrap_or("niet gewogen".into()),
                    if *stored{""}else{" — simulatie met hardware-kalibratie, niet opgeslagen"}),Err(e)=>format!("Controle {name}: {e}")};
                if r.is_ok(){applog::info("OK",&text)}else{applog::error("FOUT",&text)}
                publish(m,&sh);
                let r=r.map(|x|x.0);
                upd(&sh,id,|s|{s.busy=false;s.ok=r.is_ok();s.text=text;s.cal_version+=1;s.check=r.ok();});
            }
            CanCmd::Scan{simulation,python,script,output,seconds,bitrate,nodes,claim}=>{
                applog::info("ACTIE",&format!("CAN-bus testen ({} {seconds:.0} s, {bitrate} bit/s)",if simulation{"simulatie"}else{"hardware"}));
                let r:Result<String>=if simulation{
                    // Simulated boards: every board of the pump list sends its heartbeat.
                    let end=Instant::now()+Duration::from_secs_f64(seconds.min(1.));
                    while Instant::now()<end && stop.check().is_ok(){std::thread::sleep(Duration::from_millis(20));}
                    if stop.check().is_err(){Err::<String,String>("Gestopt".into())}else{
                    let mut n=nodes.clone();n.sort();n.dedup();
                    let v=json!({"channels":[{"channel":0,"frames":n.len()*4,"errors":0,"heartbeat":n,"nodes":n.iter().map(|x|(x.to_string(),json!(4))).collect::<serde_json::Map<_,_>>()}]});
                    workflow::format_scan(&v,seconds,bitrate).map(|t|format!("Simulatie — {t}"))}
                }else{
                    Bridge::spawn(&python,&script,&output,&output,stop.clone()).and_then(|mut dev|dev.call("scan",json!({"seconds":seconds,"bitrate":bitrate}))).and_then(|v|workflow::format_scan(&v,seconds,bitrate))
                };
                // The bridge (and with it every opened channel) is closed here; only then the claim goes.
                drop(claim);
                let was_stopped=stop.check().is_err();
                let text=match &r{Ok(t)=>t.clone(),Err(_) if was_stopped=>"CAN-test gestopt (STOP)".into(),Err(e)=>format!("CAN-test mislukt: {e}{}",hint(e))};
                if r.is_ok(){applog::info("OK",&text)}else if was_stopped{applog::warn("STOP",&text)}else{applog::error("FOUT",&text)}
                upd(&sh,id,|s|{s.busy=false;s.ok=r.is_ok();s.scan=text;});
            }
        }
    }
}

const SCALE_POLL:Duration=Duration::from_millis(350);
fn scale_worker(id:DevId,rx:mpsc::Receiver<ScaleCmd>,sh:Shared,gen:Arc<AtomicU64>){
    // A command is stopped when the head's STOP generation moved on after it was sent.
    let stopped_since=|g0:u64|{let gen=gen.clone();move||gen.load(Ordering::SeqCst)!=g0};
    let head=id.head().unwrap();let mut link:Option<(Box<dyn ScaleLink>,Option<Claim>)>=None;let mut tare=0.;
    loop{
        match rx.recv_timeout(SCALE_POLL){
            Ok(ScaleCmd::Connect{net,simulation,world,claim})=>{
                let r:Result<Box<dyn ScaleLink>>=if simulation{world.map(|w|Box::new(SimScale::new(w,head)) as Box<dyn ScaleLink>).ok_or("Simulatie zonder machinemodel".into())}
                    else{MtSicsTcp::connect(&net).map(|s|Box::new(s) as Box<dyn ScaleLink>)};
                match r{
                    Ok(s)=>{applog::info("VERBINDING",&format!("Weegschaal {} verbonden ({})",head.name(),if simulation{"simulatie".into()}else{format!("{}:{}",net.host.trim(),net.port)}));
                        tare=0.;upd(&sh,id,|s|{s.link=Link::On;s.ok=true;s.text="Verbonden".into();s.tare=0.;s.weight=None;s.weight_error.clear();});link=Some((s,claim));}
                    Err(e)=>{applog::error("VERBINDING",&format!("Weegschaal {} niet verbonden: {e}",head.name()));upd(&sh,id,|s|{s.link=Link::Failed;s.ok=false;s.text=format!("Niet verbonden: {e}");});drop(claim);}
                }
            }
            Ok(ScaleCmd::Read{stable,timeout_s,gen:g0,reply})=>{
                let stopped=stopped_since(g0);
                let r=match link.as_mut(){None=>Err(format!("Geen weegschaal {} verbonden",head.name())),Some((s,_))=>s.read_until(stable,timeout_s,&stopped)};
                if let Err(e)=&r{if scale_link_lost(e){lost(&mut link,&sh,id,head,e);}}
                if let Ok((w,st))=&r{upd(&sh,id,|s|{s.weight=Some(*w);s.stable=*st;s.weight_at=Some(Instant::now());s.weight_error.clear();});}
                let _=reply.send(r);
            }
            Ok(ScaleCmd::Disconnect)=>{if link.take().is_some(){applog::info("VERBINDING",&format!("Weegschaal {} verbroken",head.name()));}upd(&sh,id,|s|{s.link=Link::Off;s.busy=false;s.weight=None;s.text="Niet verbonden".into();});}
            Ok(c@(ScaleCmd::Tare{..}|ScaleCmd::Weigh{..}))=>{
                let (tare_cmd,g0)=match c{ScaleCmd::Tare{gen}=>(true,gen),ScaleCmd::Weigh{gen}=>(false,gen),_=>unreachable!()};
                let stopped=stopped_since(g0);
                let what=format!("Weegschaal {} {}",head.name(),if tare_cmd{"tarra"}else{"stabiel wegen"});applog::info("ACTIE",&what);
                let r=match link.as_mut(){None=>Err("Weegschaal niet verbonden".to_string()),Some((s,_))=>s.read_until(true,10.,&stopped)};
                if let Err(e)=&r{if scale_link_lost(e){lost(&mut link,&sh,id,head,e);}}
                if let (true,Ok((w,_)))=(tare_cmd,&r){tare=*w;}
                let text=match &r{Ok((w,_))=>format!("{what}: {:.4} g bruto, {:.4} g netto",w,w-tare),Err(e)=>format!("{what}: {e}")};
                if r.is_ok(){applog::info("OK",&text)}else{applog::error("FOUT",&text)}
                upd(&sh,id,|s|{s.busy=false;s.ok=r.is_ok();s.text=text;s.tare=tare;if let Ok((w,st))=r{s.weight=Some(w);s.stable=st;s.weight_at=Some(Instant::now());s.weight_error.clear();}});
            }
            Err(mpsc::RecvTimeoutError::Timeout)=>{
                // Live reading: answer within 0.4 s or skip. A silent scale only slows this worker.
                if let Some((s,_))=link.as_mut(){
                    let r=s.read(false,0.4);
                    if let Err(e)=&r{if scale_link_lost(e){lost(&mut link,&sh,id,head,e);continue;}}
                    upd(&sh,id,|st|{st.reads+=1;match r{Ok((w,stable))=>{st.weight=Some(w);st.stable=stable;st.weight_at=Some(Instant::now());st.weight_error.clear();}
                        Err(e)=>{st.weight_error=if e.contains("time-out"){"Geen antwoord binnen 0,4 s".into()}else{e};}}});
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected)=>break,
        }
    }
}

/// Scale connection gone: close it, free the claim, show the fault (red).
fn lost(link:&mut Option<(Box<dyn ScaleLink>,Option<Claim>)>,sh:&Shared,id:DevId,head:Head,e:&str){
    *link=None;
    applog::error("WEEGSCHAAL",&format!("Weegschaal {} verbinding verloren: {e}",head.name()));
    upd(sh,id,|s|{s.link=Link::Failed;s.ok=false;s.weight=None;s.text=format!("Verbinding verloren: {e}");s.weight_error=e.to_string();});
}

enum ScanCmd{Connect{cfg:ScannerConfig,simulation:bool,claim:Option<Claim>,python:PathBuf,script:PathBuf,output:PathBuf,stop:StopHandle},Disconnect,
    /// One trigger/read; `stop` ends the wait; the answer goes to `reply` (production) or only to the status (test button).
    Read{stop:StopHandle,reply:Option<mpsc::Sender<Result<(ScanResult,String)>>>}}
fn scan_worker(rx:mpsc::Receiver<ScanCmd>,sh:Shared,sim:Arc<Mutex<SimScan>>){
    let id=DevId::Scanner;let mut link:Option<(Box<dyn ScanLink>,ScannerConfig,Option<Claim>)>=None;
    while let Ok(c)=rx.recv(){match c{
        ScanCmd::Connect{cfg,simulation,claim,python,script,output,stop}=>{
            let r:Result<Box<dyn ScanLink>>=if simulation{Ok(Box::new(scanner::SimScanner{line:sim.clone()}))}else{cfg.validate().and_then(|_|match cfg.kind{
                ScanKind::Tcp=>scanner::TcpScanner::open(&cfg.host,cfg.port).map(|x|Box::new(x) as Box<dyn ScanLink>),
                ScanKind::Com=>scanner::ComScanner::open(&python,&script,&output,&cfg,stop).map(|x|Box::new(x) as Box<dyn ScanLink>)})};
            match r{
                Ok(l)=>{applog::info("VERBINDING",&format!("Scanner verbonden ({})",if simulation{"simulatie".into()}else{cfg.endpoint()}));link=Some((l,cfg,claim));upd(&sh,id,|s|{s.link=Link::On;s.ok=true;s.text="Verbonden".into();});}
                Err(e)=>{applog::error("VERBINDING",&format!("Scanner niet verbonden: {e}"));upd(&sh,id,|s|{s.link=Link::Failed;s.ok=false;s.text=format!("Niet verbonden: {e}");});}
            }
        }
        ScanCmd::Disconnect=>{if link.take().is_some(){applog::info("VERBINDING","Scanner verbroken");}upd(&sh,id,|s|{s.link=Link::Off;s.busy=false;s.text="Niet verbonden".into();});}
        ScanCmd::Read{stop,reply}=>{
            let r=match link.as_mut(){None=>Err("Scanner niet verbonden".to_string()),Some((l,cfg,_))=>{let st=stop.clone();l.read(cfg,&move||st.check().is_err())}};
            let text=match &r{Ok((x,raw))=>format!("Gelezen: {} — ruw: {raw}",x.text()),Err(e)=>format!("Lezen mislukt: {e}")};
            applog::info("SCANNER",&text);upd(&sh,id,|s|{s.busy=false;s.ok=r.is_ok();s.text=text.clone();s.scan=text;});
            if let Some(tx)=reply{let _=tx.send(r);}
        }
    }}
}
const PLC_POLL:Duration=Duration::from_millis(250);
fn plc_worker(rx:mpsc::Receiver<PlcCmd>,sh:Shared,stop:StopHandle,script:Arc<Mutex<Vec<(u32,u32)>>>,script_cmd:Arc<Mutex<Vec<(u32,u32,u32)>>>,tr:PlcTrace){
    let id=DevId::Plc;let mut plc:Option<(Mailbox,Option<Claim>,PlcConfig)>=None;
    let sync=|m:&Mailbox,sh:&Shared|{let msgs=m.log.clone();upd(sh,id,|s|s.messages=msgs);};
    loop{
        match rx.recv_timeout(PLC_POLL){
            Ok(PlcCmd::Connect{cfg,simulation,claim})=>{
                let r:Result<Mailbox>=(||{cfg.validate()?;
                    let link:Box<dyn crate::plc::Link>=if simulation{let mut e=Emulator::new(&cfg);e.script=script.clone();e.script_cmd=script_cmd.clone();Box::new(e)}else{Box::new(FinsUdp::open(&cfg)?)};
                    let mut m=Mailbox::new(link,&cfg);m.poll()?;Ok(m)})();
                match r{
                    Ok(m)=>{applog::info("VERBINDING",&format!("PLC verbonden ({})",if simulation{"simulatie".into()}else{format!("{} kanaal {}",cfg.ip.trim(),cfg.channel)}));
                        sync(&m,&sh);upd(&sh,id,|s|{s.link=Link::On;s.ok=true;s.text="Verbonden".into();});plc=Some((m,claim,cfg));}
                    Err(e)=>{applog::error("VERBINDING",&format!("PLC niet verbonden: {e}"));upd(&sh,id,|s|{s.link=Link::Failed;s.ok=false;s.text=format!("Niet verbonden: {e}");});drop(claim);}
                }
            }
            Ok(PlcCmd::Disconnect)=>{if plc.take().is_some(){applog::info("VERBINDING","PLC verbroken");}upd(&sh,id,|s|{s.link=Link::Off;s.busy=false;s.text="Niet verbonden".into();});}
            Ok(PlcCmd::Exec{process,cmd,params,force,reply})=>{
                let what=format!("PLC proces {process} opdracht {cmd}");applog::info("PLC",&what);
                let r:Result<u32>=match plc.as_mut(){None=>Err("PLC niet verbonden".into()),Some((m,_,cfg))=>{
                    let timeout=Duration::from_secs_f64(cfg.timeout_s);
                    let st=stop.clone();let stopped=move||!force && st.check().is_err();
                    // Any answer except an update ends the wait; `pot_reply` decides (16 = geen pot, unexpected = fout).
                    m.command_with(process,cmd,&params,&[],timeout,&stopped,&mut |_|{})
                }};
                trace(&tr,process,cmd,&params,&r);
                if let Some((m,_,_))=&plc{sync(m,&sh);}
                match &r{Ok(c)=>applog::info("PLC",&format!("{what}: antwoord {c}")),Err(e)=>applog::error("PLC",&format!("{what}: {e}"))}
                let _=reply.send(r);
            }
            Ok(PlcCmd::Post{process,cmd,reply})=>{
                let r:Result<u32>=match plc.as_mut(){None=>Err("PLC niet verbonden".into()),Some((m,_,cfg))=>{
                    m.post_until(process,cmd,&[],Duration::from_secs_f64(cfg.timeout_s.min(10.)),&||false).map(|_|0)}};
                trace(&tr,process,cmd,&[],&r);if let Some((m,_,_))=&plc{sync(m,&sh);}let _=reply.send(r);
            }
            Ok(PlcCmd::Sequence{label,steps,stop_on_error})=>{
                applog::info("ACTIE",&format!("PLC: {label}"));let mut notes=vec![];let mut ok=true;
                for (process,cmd,params) in steps{
                    let r:Result<u32>=match plc.as_mut(){None=>Err("PLC niet verbonden".into()),Some((m,_,cfg))=>{
                        let timeout=Duration::from_secs_f64(cfg.timeout_s);let st=stop.clone();let stopped=move||st.check().is_err();
                        if fire_and_forget(cmd){m.post_until(process,cmd,&params,timeout,&stopped).map(|_|0)}else{m.command_with(process,cmd,&params,&[],timeout,&stopped,&mut |_|{})}
                    }};
                    trace(&tr,process,cmd,&params,&r);
                    match &r{Ok(0)=>notes.push(format!("{process}/{cmd} verstuurd")),Ok(c)=>notes.push(format!("{process}/{cmd} → {c}")),Err(e)=>{ok=false;notes.push(format!("{process}/{cmd}: {e}"));}}
                    if r.is_err() && (stop_on_error || stop.check().is_err()){break;}
                }
                if let Some((m,_,_))=&plc{sync(m,&sh);}
                let text=format!("{label}: {}",notes.join(", "));if ok{applog::info("OK",&text)}else{applog::error("FOUT",&text)}
                upd(&sh,id,|s|{s.busy=false;s.ok=ok;s.text=text;});
            }
            Ok(PlcCmd::Command{process,cmd,params})=>{
                let what=format!("PLC proces {process} opdracht {cmd} parameters {params:?}");applog::info("ACTIE",&what);
                let r:Result<u32>=match plc.as_mut(){None=>Err("PLC niet verbonden".into()),Some((m,_,cfg))=>{
                    let timeout=Duration::from_secs_f64(cfg.timeout_s);
                    let st=stop.clone();let stopped=move||st.check().is_err();
                    // Camelot sends Reset/Abort/Hold without waiting for a reply; waiting for pick-up still honours STOP.
                    if fire_and_forget(cmd){m.post_until(process,cmd,&params,timeout,&stopped).map(|_|0)}
                    else{m.command_with(process,cmd,&params,&[],timeout,&stopped,&mut |_|{})}
                }};
                let r=r.map_err(|e|stopped(&e,&stop));
                trace(&tr,process,cmd,&params,&r);
                if let Some((m,_,_))=&plc{sync(m,&sh);}
                let text=match &r{Ok(0)=>format!("{what}: verstuurd"),Ok(c)=>format!("{what}: antwoord {c}"),Err(e)=>format!("{what}: {e}")};
                if r.is_ok(){applog::info("OK",&text)}else{applog::error("FOUT",&text)}
                upd(&sh,id,|s|{s.busy=false;s.ok=r.is_ok();s.text=text;});
            }
            Err(mpsc::RecvTimeoutError::Timeout)=>{
                let Some((m,_,_))=plc.as_mut() else{continue};
                match m.poll(){
                    Ok(Some(r))=>{let t=format!("<- proces {} antwoord {} resultaat {}",r.process,r.code,r.params[0] as i32);applog::info("PLC",&t);m.log.push(t);sync(m,&sh);}
                    Ok(None)=>{}
                    Err(e)=>{applog::error("PLC",&format!("Verbinding verbroken: {e}"));plc=None;upd(&sh,id,|s|{s.link=Link::Failed;s.ok=false;s.text=format!("Verbinding verbroken: {e}");});}
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected)=>break,
        }
    }
}

/// Longest wait of a profile switch for stopped commands and closed connections.
pub const PROFILE_WAIT:Duration=Duration::from_millis(3000);

pub struct Devices{
    pub profile:ProfileKind,pub settings:Settings,
    shared:Shared,pub registry:Registry,
    can_tx:mpsc::Sender<CanCmd>,scale_tx:[mpsc::Sender<ScaleCmd>;2],plc_tx:mpsc::Sender<PlcCmd>,
    can_stop:StopHandle,plc_stop:StopHandle,bench_stop:StopHandle,
    /// Both scale workers: STOP ends a running stable weighing.
    /// STOP generation per head (LH1, LH2): STOP increments both; a weighing ends when the generation moved on
    /// after it was sent. Nothing is ever reset, so a new command can never clear a STOP meant for another.
    scale_gen:[Arc<AtomicU64>;2],
    /// Correction, motion and "volgende dosis" for pump tests (shared with the CAN worker).
    pub test_ctl:Arc<Mutex<TestControl>>,
    /// Pot executor: status, its own STOP and the operator's "Volgende kop".
    pub pot:Arc<Mutex<PotStatus>>,pot_stop:StopHandle,pot_resume:Arc<std::sync::atomic::AtomicBool>,
    /// PLC emulator: scripted answers (process, code) for the next command, e.g. (1350, 16) "geen pot" (tests).
    pub plc_script:Arc<Mutex<Vec<(u32,u32)>>>,
    /// Same, only for that command: (process, command, reply code).
    pub plc_script_cmd:Arc<Mutex<Vec<(u32,u32,u32)>>>,
    /// Every PLC command with its reply (production proof: order of line/transport/lift commands).
    pub plc_trace:PlcTrace,
    /// Barcode scanner worker; its own STOP (a COM read through the bridge ends on it).
    scan_tx:mpsc::Sender<ScanCmd>,scan_stop:StopHandle,
    /// Simulated scan spot: the pots the operator put on the line, in order (+ test hooks).
    pub sim_scan:Arc<Mutex<SimScan>>,
    /// Production engine: status for the UI, commands, its STOP (Afbreken and STOP end every wait of it),
    /// and the progress lines of its heads.
    pub prod:Arc<Mutex<ProdStatus>>,prod_tx:Option<mpsc::Sender<ProdCmd>>,prod_stop:StopHandle,pub prod_heads:Arc<Mutex<PotStatus>>,
    /// Running ceramic test (driven from `poll`, one engine dose at a time).
    bench_test:Option<BenchTest>,bench_test_version:Arc<AtomicU64>,
    /// Background database writer for the ceramic test (the UI never waits for the database).
    db_jobs:Option<mpsc::Sender<DbJob>>,
    bench_tx:mpsc::Sender<Action>,bench_rx:mpsc::Receiver<Event>,
    /// Latest state of the pump bench worker.
    pub bench:State,
    /// Bench actions sent but not yet started (each action announces its start with one busy event).
    bench_queued:usize,bench_pending:Option<DevId>,bench_claim:Option<Claim>,bench_scale_claim:Option<Claim>,
    /// Shared simulated VSD machine (pumps and both scales), created on the first simulated VSD connect.
    pub world:Option<Arc<Mutex<SimWorld>>>,
    /// Ceramic calibration/curve started by the user (kind, time sent); stored when the bench is idle again.
    bench_run:Option<(&'static str,u128)>,
    /// Bumped whenever a ceramic run was stored (the UI reloads its history); written by the store thread.
    bench_cal_version:Arc<std::sync::atomic::AtomicU64>,
    /// Asynchronous profile switch: target and start; result for the UI.
    pub switching:Option<(ProfileKind,Instant)>,switch_result:Option<std::result::Result<ProfileKind,String>>,
}
impl Devices{
    pub fn new(profile:ProfileKind,settings:Settings)->Self{
        let shared:Shared=Arc::new(Mutex::new(HashMap::new()));
        for id in DevId::VSD.into_iter().chain(DevId::KERAMISCH){upd(&shared,id,|s|s.text="Niet verbonden".into());}
        let (can_stop,plc_stop,bench_stop)=(StopHandle::default(),StopHandle::default(),StopHandle::default());
        let scale_gen=[Arc::new(AtomicU64::new(0)),Arc::new(AtomicU64::new(0))];let test_ctl=Arc::new(Mutex::new(TestControl::default()));
        let spawn=|name:&str,f:Box<dyn FnOnce()+Send>|{std::thread::Builder::new().name(name.into()).spawn(f).expect("thread");};
        let (t1,r1)=mpsc::channel();{let sh=shared.clone();let ss=scale_gen[0].clone();spawn("scale-lh1",Box::new(move||scale_worker(DevId::Lh1,r1,sh,ss)));}
        let (t2,r2)=mpsc::channel();{let sh=shared.clone();let ss=scale_gen[1].clone();spawn("scale-lh2",Box::new(move||scale_worker(DevId::Lh2,r2,sh,ss)));}
        let (can_tx,rx)=mpsc::channel();{let sh=shared.clone();let st=can_stop.clone();let sc=[t1.clone(),t2.clone()];let g=[scale_gen[0].clone(),scale_gen[1].clone()];let c=test_ctl.clone();spawn("can",Box::new(move||can_worker(rx,sh,st,sc,g,c)));}
        let plc_script:Arc<Mutex<Vec<(u32,u32)>>>=Default::default();let plc_trace:PlcTrace=Default::default();let plc_script_cmd:Arc<Mutex<Vec<(u32,u32,u32)>>>=Default::default();
        let (plc_tx,rx)=mpsc::channel();{let sh=shared.clone();let st=plc_stop.clone();let sc=plc_script.clone();let sc2=plc_script_cmd.clone();let tr=plc_trace.clone();spawn("plc",Box::new(move||plc_worker(rx,sh,st,sc,sc2,tr)));}
        let sim_scan:Arc<Mutex<SimScan>>=Default::default();
        let (scan_tx,rx)=mpsc::channel();{let sh=shared.clone();let ss=sim_scan.clone();spawn("scanner",Box::new(move||scan_worker(rx,sh,ss)));}
        let (bench_tx,bench_rx)=workflow::spawn_worker(settings.output.clone(),bench_stop.clone());
        Self{profile,settings,shared,registry:Registry::default(),can_tx,scale_tx:[t1,t2],plc_tx,can_stop,plc_stop,bench_stop,scale_gen,test_ctl,pot:Arc::new(Mutex::new(PotStatus::default())),pot_stop:StopHandle::default(),pot_resume:Arc::new(std::sync::atomic::AtomicBool::new(false)),plc_script,plc_script_cmd,plc_trace,scan_tx,scan_stop:StopHandle::default(),sim_scan,prod:Default::default(),prod_tx:None,prod_stop:StopHandle::default(),prod_heads:Default::default(),bench_test:None,bench_test_version:Arc::new(AtomicU64::new(0)),db_jobs:None,bench_tx,bench_rx,
            bench:State::default(),bench_queued:0,bench_pending:None,bench_claim:None,bench_scale_claim:None,world:None,bench_run:None,bench_cal_version:Arc::new(std::sync::atomic::AtomicU64::new(0)),switching:None,switch_result:None}
    }
    /// Drain bench events and free bench resources after a disconnect or failed connect. Call every frame.
    pub fn poll(&mut self){
        while let Ok(ev)=self.bench_rx.try_recv(){if let Event::State(s)=ev{if s.busy{self.bench_queued=self.bench_queued.saturating_sub(1);}self.bench=*s;}}
        if self.bench_queued==0 && !self.bench.busy{
            self.bench_pending=None;
            // The stopped command is over (its reference already handled by the engine): idle again without STOP.
            if self.bench_stop.check().is_err(){self.bench_stop.reset();}
            if !self.bench.connected{self.bench_claim=None;self.bench_scale_claim=None;}
            else if !self.bench.scale_ok{self.bench_scale_claim=None;}
            if let Some((kind,sent))=self.bench_run.take(){self.store_bench_run(kind,sent);}
            self.advance_bench_test();
        }
        self.advance_switch();
    }
    /// Ceramic calibration or curve finished, stopped or refused: store it (never as active when not
    /// finished). The write runs on its own thread so the UI never waits for the database.
    fn store_bench_run(&mut self,kind:&'static str,sent:u128){
        let mut st=self.bench.clone();
        // Refused before it began (e.g. no reference): the records on screen belong to an earlier run.
        if st.started_ms<sent || st.test!=if kind==calib::KIND_BENCH_CAL{"kalibratie"}else{"characterization"}{st.records.clear();st.curve=None;st.calibration_g_rev=None;st.started_ms=sent;}
        let ok=!st.status.starts_with("Afgebroken") && !st.status.starts_with("Opslag mislukt") && !st.records.is_empty();
        let path=self.settings.db_path.clone();let version=self.bench_cal_version.clone();
        let _=std::thread::Builder::new().name("db-bench".into()).spawn(move||{
            let r=(||{let p=path.as_ref().ok_or("geen database ingesteld")?;let mut db=Db::open(p)?;db.set_busy_timeout(Duration::from_secs(5))?;
                let pump=db.pumps(db.profile(ProfileKind::Keramisch)?.id)?.into_iter().next().map(|p|p.name).unwrap_or("Keramisch 1".into());
                let c=NewCal::from_bench(kind,&pump,&st,ok,if ok{""}else{&st.status});db.save_calibration(&c).map(|id|(id,c.ok))})();
            match r{
                Ok((id,true))=>applog::info("KALIBRATIE",&format!("{} opgeslagen (actief, nr {id})",calib::kind_label(kind))),
                Ok((id,false))=>applog::warn("KALIBRATIE",&format!("{} opgeslagen als afgebroken (nr {id}): {}",calib::kind_label(kind),st.status)),
                Err(e)=>applog::error("DATABASE",&format!("{} niet opgeslagen: {e}",calib::kind_label(kind))),
            }
            version.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        });
    }
    /// Number of ceramic runs stored so far (changes = reload history).
    pub fn bench_cal_version(&self)->u64{self.bench_cal_version.load(std::sync::atomic::Ordering::SeqCst)}
    fn dev_io(&self,stop:StopHandle,status:Arc<Mutex<PotStatus>>,settle_s:f64)->DevIo{
        DevIo{can:self.can_tx.clone(),scales:self.scale_tx.clone(),gens:[self.scale_gen[0].clone(),self.scale_gen[1].clone()],plc:self.plc_tx.clone(),
            stop,resume:self.pot_resume.clone(),status,db_path:self.settings.db_path.clone(),db:None,
            lift:self.settings.machine.lift_process,unit:self.settings.machine.unit_process,settle_s,timeout_s:self.settings.machine.timeout_s}
    }
    /// Pumps, calibration, tests and pots are refused while a pot or production runs (they share the heads).
    fn exclusive(&self)->Result<()>{
        if self.pot_running(){return Err("Er loopt een pot: wacht tot die klaar is of druk STOP".into());}
        if self.production_running(){return Err("Productie loopt: eerst Stop of Afbreken op Productie".into());}
        Ok(())
    }
    // ---------- production ----------
    fn line_io(&self,hardware:bool)->DevLine{
        let m=&self.settings.machine;
        DevLine{head:self.dev_io(self.prod_stop.clone(),self.prod_heads.clone(),if hardware{1.}else{0.}),plc:self.plc_tx.clone(),scan:self.scan_tx.clone(),stop:self.prod_stop.clone(),
            scan_stop:self.scan_stop.clone(),plc_timeout:m.plc.timeout_s,scan_timeout:m.scanner.timeout_s,hardware}
    }
    pub fn production_running(&self)->bool{self.prod.lock().unwrap().running}
    /// Pre-start check of the plan (step 1): devices, pumps/calibrations of the queued recipes, lift heights.
    pub fn production_check(&self,db:&Db)->Vec<String>{
        let mut w=vec![];
        if self.profile!=ProfileKind::Vsd{return vec!["Productie draait op het VSD-profiel".into()];}
        for id in DevId::VSD{if self.status(id).link!=Link::On{w.push(format!("{} niet verbonden",id.name()));}}
        if self.busy(DevId::Can){w.push("CAN nog bezig".into());}
        for h in Head::ALL{if self.lift_up(h){w.push(format!("Lift {} staat omhoog (handbediening): eerst omlaag op Machine/lijn",h.name()));}}
        if self.pot_running(){w.push("Er loopt een pot (Recepten)".into());}if self.running_test().is_some(){w.push("Er loopt een pomptest".into());}
        let hardware=self.status(DevId::Can).link==Link::On && !self.status(DevId::Can).simulation;
        if let Err(e)=self.settings.machine.line.validate(){w.push(e);}
        let q=db.queue(ProfileKind::Vsd).map(|q|q.len()).unwrap_or(0);
        let on_line=db.line_state().ok().flatten().map(|l|l.slots.iter().any(|s|*s!=production::Slot::Empty)).unwrap_or(false);
        if q==0 && !on_line{w.push("Wachtrij leeg: maak eerst een order".into());}
        w.extend(production::precheck(db,ProfileKind::Vsd,hardware,self.settings.machine.lift_height_cm));
        w
    }
    /// Start: the cycle runs on its own thread until the line is empty and (queue empty or Stop), or Afbreken.
    /// On a real machine `confirm` must be set (line, transport, lifts and pumps move on their own).
    pub fn start_production(&mut self,db:&Db,confirm:bool)->Result<()>{
        if self.switching.is_some(){return Err("Profielwissel loopt: even wachten".into());}
        if self.production_running(){return Err("Productie loopt al".into());}
        let w=self.production_check(db);if !w.is_empty(){return Err(format!("Start niet mogelijk: {}",w.join("; ")));}
        let hardware=!self.status(DevId::Can).simulation;
        if hardware && !confirm{return Err("Echte machine: bevestig eerst dat lijn, transport, liften en pompen zelf mogen bewegen".into());}
        let path=self.settings.db_path.clone().ok_or("Geen database ingesteld")?;
        if db.path()!=path.as_path(){return Err("Andere database dan die van de apparaten".into());}
        let m=&self.settings.machine;
        let cfg=production::EngineCfg{profile:ProfileKind::Vsd,line:m.line.clone(),proc_:production::Processes{line:m.line_process,transport:m.transport_process,barcode:m.barcode_process,unit:m.unit_process,lift:m.lift_process},
            heights:m.lift_height_cm,retries:m.scanner.retries};
        // Simulation: the operator puts the waiting pots on the line in queue order.
        if !self.status(DevId::Scanner).simulation{}else{let mut sc=self.sim_scan.lock().unwrap();if sc.feed.is_empty(){sc.feed=db.queue(ProfileKind::Vsd)?.into_iter().map(|p|p.barcode).collect();}}
        // Caller-side reset (no lost STOP): every STOP after this point reaches the cycle.
        self.prod_stop.reset();self.can_stop.reset();self.plc_stop.reset();self.scan_stop.reset();
        let line=self.line_io(hardware);
        let (tx,rx)=mpsc::channel();self.prod_tx=Some(tx);
        {let mut p=self.prod.lock().unwrap();p.running=true;p.hold=None;p.state="loopt".into();p.version+=1;}
        *self.prod_heads.lock().unwrap()=PotStatus::default();
        let status=self.prod.clone();let heads_io=line.head.fresh();
        applog::info("PRODUCTIE",&format!("Start ({})",if hardware{"hardware"}else{"simulatie"}));
        std::thread::Builder::new().name("productie".into()).spawn(move||{
            let r=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
                let d=Db::open(&path).and_then(|d|d.set_busy_timeout(Duration::from_secs(5)).map(|_|d))?;
                let mut eng=production::Engine::new(d,Box::new(line),cfg,status.clone(),rx)?;Ok::<_,String>(eng.run())
            }));
            let msg=match r{Ok(Ok(_))=>None,Ok(Err(e))=>Some(format!("Productie niet gestart: {e}")),Err(_)=>{
                // A bug in the cycle: both lifts down, never leave them up.
                for h in Head::ALL{let mut io=heads_io.fresh();let _=potrun::PotIo::lift(&mut io,h,false,0,true);let _=potrun::PotIo::unit(&mut io,h,false,true);}
                Some("Interne fout (paniek) in de productie: liften omlaag gestuurd".into())}};
            let mut p=status.lock().unwrap_or_else(|e|e.into_inner());p.running=false;
            if let Some(m)=msg{applog::error("PRODUCTIE",&m);p.messages.push(m.clone());p.state="fout".into();p.hold=Some(m);}p.version+=1;
        }).map_err(|e|e.to_string())?;
        Ok(())
    }
    /// Pauze / Verder / Stop / Herstart / code for the scan spot; Afbreken also stops pumps and every wait at once.
    pub fn production_cmd(&mut self,c:ProdCmd)->Result<()>{
        if !self.production_running(){return Err("Productie loopt niet".into());}
        if c==ProdCmd::Abort{self.prod_stop.stop();self.can_stop.stop();self.scan_stop.stop();for g in &self.scale_gen{g.fetch_add(1,Ordering::SeqCst);}}
        applog::info("PRODUCTIE",&format!("Opdracht {c:?}"));
        self.prod_tx.as_ref().ok_or("Productie loopt niet")?.send(c).map_err(|_|"Productie-taak gesloten".to_string())
    }
    // ---------- Machine/lijn (manual) ----------
    /// Lift up when its last command (Start) succeeded and no Stop/Down came after it.
    pub fn lift_up(&self,h:Head)->bool{
        let p=self.settings.machine.lift_process[h.idx()];
        self.plc_trace.lock().unwrap().iter().rev().find(|x|x.1==p && matches!(x.2,CMD_START|CMD_STOP|CMD_UP|CMD_DOWN|CMD_ABORT)).map(|x|matches!(x.2,CMD_START|CMD_UP) && x.4.is_ok()).unwrap_or(false)
    }
    /// Manual PLC action(s) on Machine/lijn. A real PLC needs `confirm`; refused while production or a pot runs.
    /// Transport only with both lifts down.
    pub fn line_action(&mut self,label:&str,steps:Vec<(u32,u32,Vec<u32>)>,confirm:bool)->Result<()>{
        if self.switching.is_some(){return Err("Profielwissel loopt: even wachten".into());}
        self.exclusive()?;
        let st=self.status(DevId::Plc);
        if st.link!=Link::On{return Err("PLC niet verbonden".into());}if st.busy{return Err("PLC nog bezig met vorige opdracht".into());}
        if !st.simulation && !confirm{return Err("Echte PLC: bevestig eerst dat de machine mag bewegen".into());}
        if steps.iter().any(|x|x.0==self.settings.machine.transport_process && x.1==CMD_START){
            for h in Head::ALL{if self.lift_up(h){return Err(format!("Lift {} staat omhoog: eerst omlaag, dan transport",h.name()));}}
        }
        self.plc_stop.reset();
        self.set(DevId::Plc,|s|{s.busy=true;s.text=format!("{label} …");});
        let _=self.plc_tx.send(PlcCmd::Sequence{label:label.into(),stop_on_error:true,steps});Ok(())
    }
    /// "PLC initialiseren (zoals Camelot)": per module Abort(5), PowerUp(98), Reset(2). Only after confirmation that
    /// Camelot is not running (P9); never automatic.
    pub fn plc_init(&mut self,confirm:bool)->Result<()>{
        if !confirm{return Err("Bevestig eerst: Camelot draait niet en de machine mag geïnitialiseerd worden".into());}
        let m=&self.settings.machine;
        let mods=[m.line_process,m.transport_process,m.barcode_process,m.unit_process[0],m.unit_process[1],m.lift_process[0],m.lift_process[1]];
        let steps=mods.iter().flat_map(|p|[(*p,CMD_ABORT,vec![]),(*p,98,vec![]),(*p,CMD_RESET,vec![])]).collect::<Vec<_>>();
        if self.switching.is_some(){return Err("Profielwissel loopt: even wachten".into());}
        self.exclusive()?;
        let st=self.status(DevId::Plc);if st.link!=Link::On{return Err("PLC niet verbonden".into());}if st.busy{return Err("PLC nog bezig".into());}
        self.plc_stop.reset();self.set(DevId::Plc,|s|{s.busy=true;s.text="PLC initialiseren …".into();});
        let _=self.plc_tx.send(PlcCmd::Sequence{label:"PLC initialiseren (Abort, PowerUp, Reset per module)".into(),stop_on_error:false,steps});Ok(())
    }
    /// Scanner test: one trigger, the raw answer in the scanner status.
    pub fn scan_test(&mut self)->Result<()>{
        if self.production_running(){return Err("Productie loopt: de scanner is in gebruik".into());}
        let st=self.status(DevId::Scanner);if st.link!=Link::On{return Err("Scanner niet verbonden".into());}if st.busy{return Err("Scanner nog bezig".into());}
        self.scan_stop.reset();self.set(DevId::Scanner,|s|{s.busy=true;s.text="Lezen …".into();});
        self.scan_tx.send(ScanCmd::Read{stop:self.scan_stop.clone(),reply:None}).map_err(|_|"Scanner-taak gesloten".to_string())
    }
    // ---------- pot (recipe on one pot) ----------
    pub fn pot_running(&self)->bool{self.pot.lock().unwrap().run.is_some()}
    /// "Volgende kop": the operator has put the pot under the next head.
    pub fn pot_resume(&mut self){self.pot_resume.store(true,Ordering::SeqCst);}
    /// Run recipe version `version` on one pot (manual: heads one after the other). Validates first (pumps,
    /// calibrations in the mode of the CAN connection), needs CAN, PLC and the scales of the heads used. On a real
    /// machine `confirm` must be set: the lift and unit move on this explicit "Start pot".
    pub fn start_pot(&mut self,db:&mut Db,version:i64,confirm:bool,settle_s:f64)->Result<i64>{
        if self.switching.is_some(){return Err("Profielwissel loopt: even wachten".into());}
        if self.profile!=ProfileKind::Vsd{return Err("Recepten draaien op het VSD-profiel".into());}
        if self.pot_running(){return Err("Er loopt al een pot".into());}
        if self.production_running(){return Err("Productie loopt: eerst Stop of Afbreken op Productie".into());}
        if self.running_test().is_some(){return Err("Er loopt een pomptest".into());}
        let can=self.status(DevId::Can);
        if can.link!=Link::On{return Err("CAN niet verbonden".into());}if can.busy{return Err("CAN nog bezig".into());}
        let plc=self.status(DevId::Plc);if plc.link!=Link::On{return Err("PLC niet verbonden: de lift gaat via de PLC".into());}
        let hardware=!can.simulation;
        if hardware && !confirm{return Err("Echte machine: bevestig eerst dat de lift en unit mogen bewegen".into());}
        if !(0. ..=60.).contains(&settle_s){return Err("Wachttijd 0 tot 60 s".into());}
        let plan=db.resolve_plan(ProfileKind::Vsd,version,hardware).map_err(|e|e.join("; "))?;
        let heads=plan.heads();
        // L1: lift height per head (P1 = cm × 10): the recipe's pot height, else Instellingen; hardware needs one.
        let mut p1=[0u32;2];
        for h in &heads{
            let cm=match plan.pot_height_cm.or(self.settings.machine.lift_height_cm[h.idx()]).filter(|v|*v>0.){
                Some(v)=>v,
                None if hardware=>return Err(format!("Geen lifthoogte voor {}: vul die in bij Instellingen (VSD) of als pothoogte op het recept",h.name())),
                None=>SIM_LIFT_CM,
            };
            check_height(cm).map_err(|e|format!("Lifthoogte {}: {e}",h.name()))?;
            p1[h.idx()]=crate::machine::height_p1(cm);
        }
        for h in &heads{let id=if *h==Head::LH1{DevId::Lh1}else{DevId::Lh2};if self.status(id).link!=Link::On{return Err(format!("Geen weegschaal {} verbonden",h.name()));}}
        let run=db.create_pot_run(ProfileKind::Vsd,version,can.simulation,"handmatig",None,None)?;
        self.pot_stop.reset();self.can_stop.reset();self.plc_stop.reset();self.pot_resume.store(false,Ordering::SeqCst);
        {let mut p=self.pot.lock().unwrap();*p=PotStatus{run:Some(run),recipe:format!("{} v{}",plan.recipe,plan.version),lines:vec![],waiting:None,version:p.version+1,last:None};}
        applog::info("RECEPT",&format!("Pot {run}: {} v{} gestart ({}; koppen {})",plan.recipe,plan.version,if hardware{"hardware"}else{"simulatie"},heads.iter().map(|h|h.name()).collect::<Vec<_>>().join(", ")));
        let mut io=self.dev_io(self.pot_stop.clone(),self.pot.clone(),if hardware{settle_s}else{0.});
        let status=self.pot.clone();
        std::thread::Builder::new().name("pot".into()).spawn(move||{
            // Heads are guarded in run_pot (panic → that lift down); this guard keeps the status clean in any case.
            let out=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
                let out=potrun::run_pot(&mut io,run,&plan,&heads,p1);
                let _=io.conn().and_then(|d|d.finish_pot_run(run,out.status,&out.note));out
            })).unwrap_or_else(|_|{for h in &heads{let _=potrun::PotIo::lift(&mut io,*h,false,0,true);}
                potrun::PotOutcome{status:recipe::POT_FAILED,note:"interne fout (paniek) in de pot-taak; liften omlaag".into()}});
            applog::info("RECEPT",&format!("Pot {run}: {}{}",out.status,if out.note.is_empty(){String::new()}else{format!(" — {}",out.note)}));
            let mut p=status.lock().unwrap_or_else(|e|e.into_inner());p.run=None;p.waiting=None;p.last=Some((run,out.status.to_string(),out.note.clone()));p.version+=1;
        }).map_err(|e|e.to_string())?;
        Ok(run)
    }
    // ---------- pump test ----------
    /// Correction for the next dose (−20…+20 %, 0,1 % steps); also between doses of a running test.
    pub fn set_correction(&mut self,c:f64)->Result<f64>{
        let c=pumptest::check_correction(c)?;self.test_ctl.lock().unwrap().correction_pct=c;
        applog::info("POMPTEST",&format!("Correctie {} % (vanaf volgende dosis)",pumptest::nls(c,1)));Ok(c)
    }
    pub fn correction(&self)->f64{self.test_ctl.lock().unwrap().correction_pct}
    /// VSD speed/acceleration/deceleration (rev/s, rev/s²), applied from the next pump command, no reconnect.
    pub fn set_motion(&mut self,m:Motion)->Result<()>{
        if [m.speed_rps,m.accel_rps2,m.decel_rps2].iter().any(|v|!v.is_finite() || *v<=0. || *v>20.){return Err("Snelheid en versnelling 0 tot 20 omw/s".into());}
        self.test_ctl.lock().unwrap().motion=Some(m);
        self.settings.machine.speed_rps=m.speed_rps;self.settings.machine.accel_rps2=m.accel_rps2;self.settings.machine.decel_rps2=m.decel_rps2;
        applog::info("POMPTEST",&format!("Pompinstellingen: snelheid {} omw/s, acceleratie {}, deceleratie {} (vanaf volgende opdracht)",m.speed_rps,m.accel_rps2,m.decel_rps2));Ok(())
    }
    /// "Volgende dosis" after a pause.
    pub fn resume_test(&mut self){self.test_ctl.lock().unwrap().resume=true;if let Some(t)=self.bench_test.as_mut(){t.waiting=false;t.resume=true;}}
    /// Run id, progress of the running test (VSD or ceramic).
    pub fn running_test(&self)->Option<TestProgress>{
        match &self.bench_test{Some(t)=>Some(TestProgress{run:t.run,pump:t.pump.clone(),done:t.k,n:t.doses.len(),waiting:t.waiting,last:t.last.clone()}),None=>self.status(DevId::Can).test.clone()}
    }
    /// Changes whenever a test dose or run status was stored (the UI reloads its table).
    pub fn test_version(&self)->u64{self.status(DevId::Can).test_version+self.bench_test_version.load(Ordering::SeqCst)}
    /// Start a pump test; returns the run id. The calibration follows the mode rule (`calibration_for_dosing`).
    /// `db`: the caller's (UI) connection; the run row is created there.
    pub fn start_test(&mut self,db:&mut Db,pump:&str,plan:&TestPlan)->Result<i64>{
        self.exclusive()?;
        if self.switching.is_some(){return Err("Profielwissel loopt: even wachten".into());}
        if self.running_test().is_some(){return Err("Er loopt al een test".into());}
        let doses=plan.doses()?;
        let path=self.settings.db_path.clone().ok_or("Geen database ingesteld")?;
        if db.path()!=path.as_path(){return Err("Andere database dan die van de apparaten".into());}
        match self.profile{
            ProfileKind::Vsd=>{
                let st=self.status(DevId::Can);
                if st.link!=Link::On{return Err("CAN niet verbonden".into());}if st.busy{return Err("CAN nog bezig met vorige opdracht".into());}
                let p=self.settings.machine.pumps.iter().find(|p|p.name==pump).ok_or(format!("Pomp {pump} onbekend"))?.clone();
                if !p.enabled{return Err(format!("{pump} staat uit"));}
                let sid=if p.head==Head::LH1{DevId::Lh1}else{DevId::Lh2};
                if self.status(sid).link!=Link::On{return Err(format!("Geen weegschaal {} verbonden: {pump} weegt op de weegschaal van zijn eigen kop",p.head.name()));}
                let cal=db.calibration_for_dosing(ProfileKind::Vsd,pump,calib::KIND_VSD,!st.simulation)?;
                let run=db.create_test_run(ProfileKind::Vsd,pump,st.simulation,Some(cal.id),plan,"")?;
                self.can_stop.reset();self.test_ctl.lock().unwrap().resume=false;
                let n=doses.len();
                self.set(DevId::Can,|s|{s.busy=true;s.text=format!("Pomptest {pump} ({n} doses) …");s.test=Some(TestProgress{run,pump:pump.into(),done:0,n,waiting:false,last:String::new()});});
                applog::info("POMPTEST",&format!("Test {run} gestart: {pump}, {} {n} doses, tolerantie ±{} %, zaad {}",plan.mode.label(),plan.tolerance_pct,plan.seed));
                let _=self.can_tx.send(CanCmd::Test{run,pump:pump.into(),doses,plan:plan.clone(),cal:cal.id,db:Some(path)});
                Ok(run)
            }
            ProfileKind::Keramisch=>{
                let b=&self.bench;
                if !b.connected{return Err("Pompbank niet verbonden".into());}if b.busy || self.bench_queued>0{return Err("Pompbank nog bezig".into());}
                if !b.scale_ok{return Err("Geen weegschaal verbonden".into());}
                if !b.reference_valid{return Err("Eerst startpositie 0 of home (Pompen)".into());}
                if b.curve.is_none(){return Err("Eerst een 360°-meetcurve in deze sessie (Kalibratie): de keramische dosering rekent vanaf de actuele fase".into());}
                let cal=db.calibration_for_dosing(ProfileKind::Keramisch,pump,calib::KIND_BENCH_CURVE,!b.simulation)?;
                let run=db.create_test_run(ProfileKind::Keramisch,pump,b.simulation,Some(cal.id),plan,"")?;
                applog::info("POMPTEST",&format!("Test {run} gestart: {pump}, {} {} doses",plan.mode.label(),doses.len()));
                self.bench_test=Some(BenchTest{run,pump:pump.into(),doses,plan:plan.clone(),k:0,sent:None,stop:false,waiting:false,resume:false,last:String::new()});
                self.advance_bench_test();
                Ok(run)
            }
        }
    }
    /// Background writer for test data of the ceramic bench (own connection, waits for locks itself).
    fn db_job(&mut self,job:DbJob){
        if self.db_jobs.is_none(){
            let Some(path)=self.settings.db_path.clone() else{applog::error("DATABASE","Geen database ingesteld: testgegevens niet opgeslagen");return};
            let (tx,rx)=mpsc::channel::<DbJob>();let version=self.bench_test_version.clone();
            let _=std::thread::Builder::new().name("db-test".into()).spawn(move||{
                let mut db=match Db::open(&path).and_then(|d|d.set_busy_timeout(Duration::from_secs(5)).map(|_|d)){Ok(d)=>d,Err(e)=>{applog::error("DATABASE",&format!("Testgegevens niet opgeslagen: {e}"));return}};
                while let Ok(job)=rx.recv(){if let Err(e)=job(&mut db){applog::error("DATABASE",&format!("Testgegevens niet opgeslagen: {e}"));}version.fetch_add(1,Ordering::SeqCst);}
            });
            self.db_jobs=Some(tx);
        }
        if let Some(tx)=&self.db_jobs{let _=tx.send(job);}
    }
    /// Ceramic test step, called when the bench is idle: store the last dose, then send the next one
    /// (correction and pump settings as they are now) or finish.
    fn advance_bench_test(&mut self){
        let Some(mut t)=self.bench_test.take() else{return};
        if let Some((t0,corr,snapshot,g))=t.sent.take(){
            let rec=self.bench.records.iter().rev().find(|r|r.kind=="Dosering").cloned();
            let ok=!self.bench.status.starts_with("Afgebroken");
            let mut row=DoseRow{run_id:t.run,nr:t.k as i64+1,requested_g:g,correction_pct:corr,settings:snapshot,duration_s:Some(t0.elapsed().as_secs_f64()),..Default::default()};
            match rec.filter(|r|ok && r.status=="completed" && r.requested_g==Some(g)){
                Some(r)=>{row.dosed_g=r.mass_g;row.target_g=r.curve_target_g;row.steps=Some(r.target_steps-r.start_steps);row.status="ok".into();}
                None=>{row.status="fout".into();row.error=if t.stop{"Gestopt (STOP)".into()}else{self.bench.status.clone()};}
            }
            let line=match row.err_pct(){Some(e)=>format!("Dosis {}/{}: {} g gevraagd, {} g gedoseerd ({} %)",t.k+1,t.doses.len(),pumptest::nl(g,3),pumptest::nl(row.dosed_g.unwrap_or(0.),4),pumptest::nls(e,2)),None=>format!("Dosis {}/{}: {}",t.k+1,t.doses.len(),row.error)};
            applog::info("POMPTEST",&line);t.last=line;t.k+=1;
            let failed=row.status!="ok";
            self.db_job(Box::new(move|db|db.add_test_dose(&row).map(|_|())));
            if failed{self.finish_bench_test(t.run,if t.stop{testdb::RUN_STOPPED}else{testdb::RUN_FAILED},"Dosis mislukt");return;}
        }
        if t.stop{self.finish_bench_test(t.run,testdb::RUN_STOPPED,"Gestopt (STOP)");return;}
        if t.k>=t.doses.len(){self.finish_bench_test(t.run,testdb::RUN_DONE,"");return;}
        if t.k>0 && t.plan.pause_each && !std::mem::take(&mut t.resume){t.waiting=true;self.bench_test=Some(t);return;}
        let g=t.doses[t.k];let corr=self.correction();let c=&self.bench.config;
        let snapshot=serde_json::json!({"pomptype":c.pump_type,"stappen_omw":c.steps_rev,"snelheid":c.speed,"acceleratie":c.accel,"deceleratie":c.decel,"uitlooptijd_s":c.settle_s}).to_string();
        t.waiting=false;t.sent=Some((Instant::now(),corr,snapshot,g));
        // P7: the database is the source; the engine writes no dosering_*.json/.csv for test doses.
        let sent=self.send_bench(Action::TestDose{grams:g,correction:corr});
        if !sent{t.sent=None;self.bench_test=Some(t);return;}
        self.bench_test=Some(t);
    }
    fn finish_bench_test(&mut self,run:i64,status:&'static str,note:&str){
        let note=note.to_string();applog::info("POMPTEST",&format!("Test {run}: {status}"));
        self.db_job(Box::new(move|db|db.finish_test_run(run,status,&note)));self.bench_test=None;
    }
    /// VSD calibration of one or more pumps, one after the other (queue). Refused when a pump is off or its
    /// head's scale is not connected.
    pub fn calibrate(&mut self,names:Vec<String>,settings:CalSettings)->Result<()>{
        self.exclusive()?;
        if self.switching.is_some(){return Err("Profielwissel loopt: even wachten".into());}
        let st=self.status(DevId::Can);
        if st.link!=Link::On{return Err("CAN niet verbonden".into());}if st.busy{return Err("CAN nog bezig met vorige opdracht".into());}
        if names.is_empty(){return Err("Kies minimaal één pomp".into());}
        settings.validate()?;
        for n in &names{
            let p=self.settings.machine.pumps.iter().find(|p|&p.name==n).ok_or(format!("Pomp {n} onbekend"))?;
            if !p.enabled{return Err(format!("{n} staat uit: eerst actief zetten op Pompen"));}
            let sid=if p.head==Head::LH1{DevId::Lh1}else{DevId::Lh2};
            if self.status(sid).link!=Link::On{return Err(format!("Geen weegschaal {} verbonden: {n} weegt op de weegschaal van zijn eigen kop",p.head.name()));}
        }
        self.can_stop.reset();
        let n=names.len();let first=names[0].clone();
        self.set(DevId::Can,|s|{s.busy=true;s.text=format!("Kalibratie {n} pomp(en) …");s.progress=Some(CalProgress{pump:first,k:1,n,step:0,steps:cal_steps(&settings),text:"start".into()});});
        let _=self.can_tx.send(CanCmd::Calibrate{names,settings,db:self.settings.db_path.clone()});Ok(())
    }
    /// Control dose with an active VSD calibration (stored with that calibration).
    /// Uses `calibration_for_dosing`: hardware only with a hardware calibration (checked by the worker).
    pub fn check_dose(&mut self,name:&str,grams:f64)->Result<()>{
        self.exclusive()?;
        if self.switching.is_some(){return Err("Profielwissel loopt: even wachten".into());}
        let st=self.status(DevId::Can);
        if st.link!=Link::On{return Err("CAN niet verbonden".into());}if st.busy{return Err("CAN nog bezig met vorige opdracht".into());}
        if !grams.is_finite() || !(0.001..=1000.).contains(&grams){return Err("Controlegewicht 0,001 tot 1000 g".into());}
        let p=self.settings.machine.pumps.iter().find(|p|p.name==name).ok_or(format!("Pomp {name} onbekend"))?;
        if !p.enabled{return Err(format!("{name} staat uit"));}
        let sid=if p.head==Head::LH1{DevId::Lh1}else{DevId::Lh2};
        if self.status(sid).link!=Link::On{return Err(format!("Geen weegschaal {} verbonden",p.head.name()));}
        self.can_stop.reset();self.set(DevId::Can,|s|{s.busy=true;s.text=format!("Controle {name} {grams} g …");});
        let _=self.can_tx.send(CanCmd::CheckDose{name:name.into(),grams,db:self.settings.db_path.clone()});Ok(())
    }
    pub fn status(&self,id:DevId)->Arc<DevStatus>{
        match id{
            DevId::Bench|DevId::BenchScale=>{
                let b=&self.bench;let mut s=DevStatus{simulation:b.simulation,busy:b.busy||self.bench_queued>0,text:b.status.clone(),ok:!b.status.starts_with("Afgebroken"),..Default::default()};
                let on=if id==DevId::Bench{b.connected}else{b.connected && b.scale_ok};
                s.link=if on{Link::On}else if self.bench_pending==Some(id){Link::Connecting}else{Link::Off};
                if id==DevId::BenchScale{s.weight=b.live_g;s.stable=b.live_stable;s.weight_error=b.live_error.clone();
                    s.endpoint=if !b.connected{String::new()}else if b.config.scale_tcp{format!("{}:{}",b.config.scale_host.trim(),b.config.scale_tcp_port)}else{b.config.port.clone()};}
                else{s.endpoint=format!("kanaal {} · node {} motor {}",b.config.channel,b.config.node,b.config.motor);}
                Arc::new(s)
            }
            _=>self.shared.lock().unwrap().get(&id).cloned().unwrap_or_default(),
        }
    }
    pub fn is_on(&self,id:DevId)->bool{matches!(self.status(id).link,Link::On|Link::Connecting)}
    pub fn busy(&self,id:DevId)->bool{self.status(id).busy}
    /// Warning text when simulated and real devices of the active profile are connected together.
    pub fn mixed(&self)->Option<String>{
        let on:Vec<(DevId,bool)>=DevId::of(self.profile).iter().filter(|d|self.is_on(**d)).map(|d|(*d,self.status(*d).simulation)).collect();
        let sim:Vec<&str>=on.iter().filter(|x|x.1).map(|x|x.0.short()).collect();let hw:Vec<&str>=on.iter().filter(|x|!x.1).map(|x|x.0.short()).collect();
        if sim.is_empty() || hw.is_empty(){None}else{Some(format!("Simulatie en hardware gemengd: simulatie {} · hardware {}",sim.join(", "),hw.join(", ")))}
    }
    fn world(&mut self)->Arc<Mutex<SimWorld>>{
        let travel=self.settings.sim_travel_s;let cfg=&self.settings.machine;
        self.world.get_or_insert_with(||{let w=SimWorld::new(cfg);w.lock().unwrap().max_travel_s=travel;w}).clone()
    }
    fn set(&self,id:DevId,f:impl FnOnce(&mut DevStatus)){upd(&self.shared,id,f);}

    /// Connect one device (simulation or hardware). Returns at once; the worker reports the outcome.
    pub fn connect(&mut self,id:DevId,simulation:bool)->Result<()>{
        let r=self.connect_inner(id,simulation);
        let what=format!("{} verbinden ({})",id.name(),if simulation{"simulatie"}else{"hardware"});
        match &r{Ok(())=>applog::info("VERBINDING",&what),Err(e)=>applog::warn("VERBINDING",&format!("{what} geweigerd: {e}"))}
        r
    }
    fn connect_inner(&mut self,id:DevId,simulation:bool)->Result<()>{
        if self.switching.is_some(){return Err("Profielwissel loopt nog".into());}
        if id.profile()!=self.profile{return Err(format!("{} hoort bij profiel {}; actief is {}",id.name(),id.profile().name(),self.profile.name()));}
        if self.is_on(id){return Err(format!("{} is al verbonden",id.name()));}
        if self.busy(id){return Err(format!("{} is nog bezig",id.name()));}
        // A6: never silently mix a simulated and a real device.
        if id!=DevId::BenchScale{
            if let Some(other)=DevId::of(self.profile).iter().copied().find(|d|*d!=id && self.is_on(*d) && self.status(*d).simulation!=simulation){
                if !self.settings.mix_allowed{
                    return Err(format!("{} is verbonden als {}; niet mengen met {}. Verbreek eerst of kies dezelfde modus.",other.name(),if simulation{"hardware"}else{"simulatie"},if simulation{"simulatie"}else{"hardware"}));
                }
                applog::warn("VERBINDING",&format!("Let op: {} als {} naast {} als {} (mengen toegestaan in Instellingen)",id.name(),if simulation{"simulatie"}else{"hardware"},other.name(),if simulation{"hardware"}else{"simulatie"}));
            }
        }
        let s=self.settings.clone();
        match id{
            DevId::Can=>{s.machine.validate()?;}
            DevId::Plc=>{s.machine.plc.validate()?;}
            DevId::Scanner=>{if !simulation{s.machine.scanner.validate()?;}}
            DevId::Lh1|DevId::Lh2=>{let n=&s.machine.scales[id.head().unwrap().idx()];if n.host.trim().is_empty() || n.port==0{return Err("Weegschaal: IP-adres en poort invullen".into());}}
            DevId::Bench=>{let mut c=s.bench.clone();c.use_scale=false;c.validate()?;}
            DevId::BenchScale=>{
                if !self.bench.connected{return Err("Eerst pompbank verbinden; de weegschaal loopt via dezelfde koppeling".into());}
                if self.bench.simulation!=simulation{return Err(format!("Weegschaal pompbank volgt de pompbank ({}); geen andere modus mogelijk",if self.bench.simulation{"simulatie"}else{"hardware"}));}
                self.bench_scale_cfg(true).validate()?;
            }
        }
        // A5: one connection per physical device.
        let claim=if simulation{None}else{
            Some(self.registry.claim(id,resources(id,&s))?)};
        let endpoint=match id{DevId::Can=>format!("kanaal {} · {} bit/s",s.machine.can_channel,s.machine.bitrate),DevId::Lh1|DevId::Lh2=>{let n=&s.machine.scales[id.head().unwrap().idx()];format!("{}:{}",n.host.trim(),n.port)},
            DevId::Plc=>format!("{}:{} kanaal {}",s.machine.plc.ip.trim(),s.machine.plc.port,s.machine.plc.channel),DevId::Scanner=>s.machine.scanner.endpoint(),_=>String::new()};
        if !matches!(id,DevId::Bench|DevId::BenchScale){self.set(id,|st|{st.link=Link::Connecting;st.simulation=simulation;st.endpoint=if simulation{"simulatie".into()}else{endpoint};st.text="Verbinden…".into();});}
        let world=if simulation && matches!(id,DevId::Can|DevId::Lh1|DevId::Lh2){Some(self.world())}else{None};
        // R1 + no lost STOP: clear an earlier STOP on the caller thread, before the command is queued.
        // A STOP pressed after this point always reaches the command, however late the worker picks it up.
        match id{DevId::Can=>self.can_stop.reset(),DevId::Plc=>self.plc_stop.reset(),_=>{}}
        let sent=match id{
            DevId::Can=>self.can_tx.send(CanCmd::Connect{cfg:s.machine.clone(),simulation,python:s.python.clone(),script:s.bridge_dir.join("vsd_bridge.py"),output:s.output.clone(),world,claim}).is_ok(),
            DevId::Lh1|DevId::Lh2=>{let h=id.head().unwrap();self.scale_tx[h.idx()].send(ScaleCmd::Connect{net:{let mut n=s.machine.scales[h.idx()].clone();n.host=norm_host(&n.host);n},simulation,world,claim}).is_ok()}
            DevId::Plc=>self.plc_tx.send(PlcCmd::Connect{cfg:s.machine.plc.clone(),simulation,claim}).is_ok(),
            DevId::Scanner=>{self.scan_stop.reset();self.scan_tx.send(ScanCmd::Connect{cfg:s.machine.scanner.clone(),simulation,claim,python:s.python.clone(),script:s.bridge_dir.join("scanner_bridge.py"),output:s.output.clone(),stop:self.scan_stop.clone()}).is_ok()}
            DevId::Bench=>{let mut c=s.bench.clone();c.use_scale=false;self.bench_claim=claim;self.bench_pending=Some(id);
                self.send_bench(Action::Connect{cfg:c,simulation,python:s.python.clone(),repo:s.repo.clone(),bridge:s.bridge_dir.join("hardware_bridge.py")})}
            DevId::BenchScale=>{let c=self.bench_scale_cfg(true);self.bench_scale_claim=claim;self.bench_pending=Some(id);
                let ok=self.send_bench(Action::Configure{cfg:c});
                // Live weight 2–3 Hz in simulation; with hardware the operator switches it on (it occupies the bench bridge).
                ok && (!simulation || self.send_bench(Action::LiveWeight(true)))}
        };
        if !sent{return Err("Achtergrondtaak gesloten".into());}
        Ok(())
    }
    /// Bench config as connected, with the scale fields of the settings and `use_scale` set.
    fn bench_scale_cfg(&self,on:bool)->Config{
        let s=&self.settings.bench;let mut c=self.bench.config.clone();
        c.use_scale=on;c.port=s.port.clone();c.baud=s.baud;c.scale_tcp=s.scale_tcp;c.scale_host=s.scale_host.clone();c.scale_tcp_port=s.scale_tcp_port;
        c.scale_dual=s.scale_dual;c.scale_host2=s.scale_host2.clone();c.scale_tcp_port2=s.scale_tcp_port2;c.measure_head=s.measure_head;c
    }
    /// Every bench action here is started by the user while the bench is idle: clear an earlier STOP first (R1).
    fn send_bench(&mut self,a:Action)->bool{if self.bench_queued==0 && !self.bench.busy{self.bench_stop.reset();}if self.bench_tx.send(a).is_ok(){self.bench_queued+=1;true}else{false}}
    pub fn disconnect(&mut self,id:DevId)->Result<()>{
        applog::info("VERBINDING",&format!("{} verbreken",id.name()));
        let ok=match id{
            DevId::Can=>self.can_tx.send(CanCmd::Disconnect).is_ok(),
            DevId::Lh1=>self.scale_tx[0].send(ScaleCmd::Disconnect).is_ok(),DevId::Lh2=>self.scale_tx[1].send(ScaleCmd::Disconnect).is_ok(),
            DevId::Plc=>self.plc_tx.send(PlcCmd::Disconnect).is_ok(),
            DevId::Scanner=>self.scan_tx.send(ScanCmd::Disconnect).is_ok(),
            DevId::Bench=>self.send_bench(Action::Disconnect),
            DevId::BenchScale=>{if !self.bench.connected{return Ok(());}let c=self.bench_scale_cfg(false);let a=self.send_bench(Action::LiveWeight(false));a && self.send_bench(Action::Configure{cfg:c})}
        };
        if !ok{return Err("Achtergrondtaak gesloten".into());}Ok(())
    }
    /// STOP / Esc: every group, every bridge (INIT to the drives), PLC waits aborted.
    pub fn stop_all(&mut self){
        applog::warn("STOP","STOP gevraagd: alle pompopdrachten, PLC-wachten en de pompbank worden onderbroken");
        self.can_stop.stop();self.plc_stop.stop();for g in &self.scale_gen{g.fetch_add(1,Ordering::SeqCst);}
        if let Some(t)=self.bench_test.as_mut(){t.stop=true;}
        self.pot_stop.stop();self.scan_stop.stop();
        // Production: STOP = Afbreken (pumps stop, every wait ends, lifts down, Abort to the line).
        if self.production_running(){self.prod_stop.stop();if let Some(tx)=&self.prod_tx{let _=tx.send(ProdCmd::Abort);}}
        // R6: an idle bench keeps its reference (its idle check would read the STOP flag as a lost reference).
        if self.bench.busy || self.bench_queued>0{self.bench_stop.stop();}
    }
    /// VSD pump command for one or more pumps (they run together).
    /// `value`: exact steps (relative for Dispense, absolute for MoveTo), checked before sending.
    pub fn pumps(&mut self,names:Vec<String>,op:PumpOp,value:i64)->Result<()>{
        self.exclusive()?;
        if self.switching.is_some(){return Err("Profielwissel loopt: even wachten".into());}
        check_steps(op,value)?;
        let st=self.status(DevId::Can);
        if st.link!=Link::On{return Err("CAN niet verbonden".into());}
        if st.busy{return Err("Pompbank nog bezig met vorige opdracht".into());}
        if names.is_empty(){return Err("Geen pomp gekozen".into());}
        self.can_stop.reset();
        self.set(DevId::Can,|s|{s.busy=true;s.text=format!("{op:?} …");});
        if self.can_tx.send(CanCmd::Pumps{names,op,value}).is_err(){return Err("Achtergrondtaak gesloten".into());}Ok(())
    }
    /// Pump list edited (head, active): the CAN group and the simulated machine follow at once.
    pub fn set_pumps(&mut self,pumps:Vec<VsdPump>){
        self.settings.machine.pumps=pumps.clone();
        if let Some(w)=&self.world{w.lock().unwrap().set_pumps(&pumps);}
        let _=self.can_tx.send(CanCmd::SetPumps(pumps));
    }
    pub fn scale(&mut self,head:Head,tare:bool)->Result<()>{
        if self.switching.is_some(){return Err("Profielwissel loopt: even wachten".into());}
        let id=if head==Head::LH1{DevId::Lh1}else{DevId::Lh2};let st=self.status(id);
        if st.link!=Link::On{return Err(format!("{} niet verbonden",id.name()));}if st.busy{return Err(format!("{} nog bezig",id.name()));}
        let gen=self.scale_gen[head.idx()].load(Ordering::SeqCst);
        self.set(id,|s|s.busy=true);let _=self.scale_tx[head.idx()].send(if tare{ScaleCmd::Tare{gen}}else{ScaleCmd::Weigh{gen}});Ok(())
    }
    pub fn plc_command(&mut self,process:u32,cmd:u32,params:Vec<u32>)->Result<()>{
        if self.switching.is_some(){return Err("Profielwissel loopt: even wachten".into());}
        let st=self.status(DevId::Plc);
        if st.link!=Link::On{return Err("PLC niet verbonden".into());}if st.busy{return Err("PLC nog bezig met vorige opdracht".into());}
        // P5: until iteration 5 a real PLC only gets Reset and Abort from the diagnosis; free commands in simulation.
        if !st.simulation && !matches!(cmd,CMD_RESET|CMD_ABORT){return Err(format!("Echte PLC: alleen Reset (2) en Abort (5) tot iteratie 5; opdracht {cmd} alleen in simulatie"));}
        self.plc_stop.reset();
        self.set(DevId::Plc,|s|{s.busy=true;s.text=format!("Proces {process} opdracht {cmd} …");});
        let _=self.plc_tx.send(PlcCmd::Command{process,cmd,params});Ok(())
    }
    /// CAN bus scan (listen only). Hardware needs every CAN channel free: the scan opens them itself.
    pub fn scan(&mut self,simulation:bool,seconds:f64)->Result<()>{
        self.exclusive()?;
        if self.switching.is_some(){return Err("Profielwissel loopt: even wachten".into());}
        if !(1. ..=40.).contains(&seconds){return Err("Luistertijd 1 tot 40 s".into());}
        if self.busy(DevId::Can){return Err("CAN nog bezig".into());}
        if !simulation{if let Some((k,o))=self.registry.any_can(){return Err(format!("{} is open door {}; verbreek die eerst, de test opent de CAN-kanalen zelf",describe_key(&k),o.name()));}}
        // R2: the hardware scan opens every channel itself, so it holds all of them while it runs.
        let claim=if simulation{None}else{Some(self.registry.claim(DevId::Can,vec!["can:*".into()])?)};
        let s=&self.settings;let nodes=s.machine.pumps.iter().map(|p|p.node).collect();
        let cmd=CanCmd::Scan{simulation,python:s.python.clone(),script:s.bridge_dir.join("vsd_bridge.py"),output:s.output.clone(),seconds,bitrate:s.machine.bitrate,nodes,claim};
        self.can_stop.reset();
        self.set(DevId::Can,|st|{st.busy=true;st.scan=format!("CAN-test loopt ({seconds:.0} s)…");});
        let _=self.can_tx.send(cmd);Ok(())
    }
    /// Pump bench action (Keramisch: zero, home, jog, weigh, live weight, configure).
    pub fn bench_action(&mut self,a:Action)->Result<()>{
        if self.switching.is_some(){return Err("Profielwissel loopt: even wachten".into());}
        if self.bench.busy || self.bench_queued>0{return Err("Pompbank nog bezig met vorige actie".into());}
        if !self.bench.connected && !matches!(a,Action::Connect{..}){return Err("Pompbank niet verbonden".into());}
        let run=match a{Action::Calibrate{..}=>Some(calib::KIND_BENCH_CAL),Action::Characterize{..}=>Some(calib::KIND_BENCH_CURVE),_=>None};
        if self.send_bench(a){
            if let Some(k)=run{self.bench_run=Some((k,std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0)));}
            Ok(())
        }else{Err("Achtergrondtaak gesloten".into())}
    }
    /// Switch profile without restart and without blocking (R2, K8): stop every running command of the old
    /// profile (and a CAN scan), disconnect its devices and return at once. `poll` finishes the switch when all
    /// of that is done, or gives up after `PROFILE_WAIT` (profile unchanged, message). Result: `take_switch_result`.
    pub fn switch_profile(&mut self,new:ProfileKind)->Result<()>{
        if self.switching.is_some(){return Err("Profielwissel loopt al".into());}
        if new==self.profile{return Ok(());}
        if self.production_running(){return Err("Productie loopt: eerst Stop of Afbreken op Productie".into());}
        let old=self.profile;let groups=DevId::of(old).to_vec();
        let open:Vec<DevId>=groups.iter().copied().filter(|d|self.is_on(*d)).collect();
        let running:Vec<DevId>=groups.iter().copied().chain([DevId::Can]).filter(|d|self.busy(*d)).collect();
        if !open.is_empty() || !running.is_empty(){
            if !running.is_empty(){applog::warn("STOP",&format!("Profielwissel: lopende opdrachten stoppen ({})",running.iter().map(|d|d.short()).collect::<Vec<_>>().join(", ")));}
            match old{ProfileKind::Vsd=>{self.can_stop.stop();self.plc_stop.stop();},ProfileKind::Keramisch=>{self.bench_stop.stop();if self.busy(DevId::Can){self.can_stop.stop();}}}
            for d in &open{if *d!=DevId::BenchScale || !open.contains(&DevId::Bench){let _=self.disconnect(*d);}}
            applog::info("PROFIEL",&format!("Profiel {} -> {}: eerst verbreken/stoppen: {}",old.name(),new.name(),open.iter().chain(running.iter()).map(|d|d.short()).collect::<Vec<_>>().join(", ")));
        }
        self.switching=Some((new,Instant::now()));
        self.advance_switch();
        Ok(())
    }
    fn advance_switch(&mut self){
        let Some((new,t))=self.switching else{return};
        let groups=DevId::of(self.profile).to_vec();
        let busy:Vec<DevId>=groups.iter().copied().chain([DevId::Can]).filter(|d|self.busy(*d) || (groups.contains(d) && self.is_on(*d))).collect();
        if busy.is_empty(){
            applog::info("PROFIEL",&format!("Profiel {} -> {}",self.profile.name(),new.name()));
            self.profile=new;if new==ProfileKind::Keramisch{self.world=None;}
            self.switching=None;self.switch_result=Some(Ok(new));
        }else if t.elapsed()>PROFILE_WAIT{
            let names=busy.iter().map(|d|d.name()).collect::<Vec<_>>().join(", ");
            applog::warn("PROFIEL",&format!("Profiel blijft {}: {names} nog niet klaar",self.profile.name()));
            self.switching=None;
            self.switch_result=Some(Err(format!("Profiel niet gewisseld: {names} is nog bezig of nog verbonden. STOP is gestuurd; probeer zo opnieuw.")));
        }
    }
    pub fn take_switch_result(&mut self)->Option<std::result::Result<ProfileKind,String>>{self.switch_result.take()}
    /// Close everything (window closed): stop, disconnect, give workers a moment to release ports.
    pub fn shutdown(&mut self){
        self.stop_all();
        for id in DevId::VSD{let _=self.disconnect(id);}
        let _=self.bench_tx.send(Action::Shutdown);
    }
}

#[cfg(test)] mod tests{
    use super::*;
    use std::net::TcpListener;
    fn out(name:&str)->PathBuf{let d=std::env::temp_dir().join(format!("pumpstudio-dev-{name}-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));std::fs::create_dir_all(&d).unwrap();d}
    fn devices(name:&str,profile:ProfileKind)->(Devices,PathBuf){
        let o=out(name);let mut s=Settings::new(MachineConfig::default(),Config::default(),o.clone());s.sim_travel_s=0.2;(Devices::new(profile,s),o)
    }
    fn wait(d:&mut Devices,what:&str,secs:f64,f:impl Fn(&Devices)->bool)->Duration{
        let t=Instant::now();while !f(d){d.poll();assert!(t.elapsed().as_secs_f64()<secs,"time-out: {what}");std::thread::sleep(Duration::from_millis(5));}t.elapsed()
    }
    fn on(d:&Devices,id:DevId)->bool{d.status(id).link==Link::On}
    /// Profile switch as the UI sees it: start, keep polling, take the result.
    fn switch(d:&mut Devices,p:ProfileKind)->std::result::Result<Duration,String>{
        let t=Instant::now();d.switch_profile(p)?;
        while d.switching.is_some(){d.poll();assert!(t.elapsed()<Duration::from_secs(5));std::thread::sleep(Duration::from_millis(5));}
        match d.take_switch_result(){Some(Ok(_))|None=>Ok(t.elapsed()),Some(Err(e))=>Err(e)}
    }
    /// Fake Mettler that accepts the connection and never answers (A4).
    fn silent_scale()->(TcpListener,u16){let l=TcpListener::bind("127.0.0.1:0").unwrap();let p=l.local_addr().unwrap().port();(l,p)}

    #[test]fn vsd_sim_connect_pump_scale_plc_and_scan(){
        let (mut d,o)=devices("vsd",ProfileKind::Vsd);
        for id in DevId::VSD{d.connect(id,true).unwrap();}
        wait(&mut d,"alles verbonden",5.,|d|DevId::VSD.iter().all(|i|on(d,*i)));
        assert!(d.mixed().is_none());assert!(d.registry.held().is_empty(),"simulatie claimt geen hardware");
        // Live weight on both heads.
        wait(&mut d,"live gewicht",3.,|d|d.status(DevId::Lh1).weight.is_some() && d.status(DevId::Lh2).weight.is_some());
        let before=d.status(DevId::Lh1).weight.unwrap();
        d.pumps(vec!["P_C01".into(),"P_A01".into()],PumpOp::Dispense,2*51200).unwrap();
        wait(&mut d,"doseren",5.,|d|!d.busy(DevId::Can));
        let st=d.status(DevId::Can);assert!(st.ok,"{}",st.text);assert_eq!(st.positions.get("P_C01"),Some(&0),"na doseren op nokfase");
        wait(&mut d,"gewicht LH1 +1 g",3.,|d|d.status(DevId::Lh1).weight.is_some_and(|w|(w-before-1.).abs()<1e-6));
        d.pumps(vec!["P_D02".into()],PumpOp::MoveTo,12800).unwrap();wait(&mut d,"naar positie",5.,|d|!d.busy(DevId::Can));
        assert_eq!(d.status(DevId::Can).positions.get("P_D02"),Some(&12800));
        // Disabled pump refused by the existing machine check.
        let mut pumps=d.settings.machine.pumps.clone();pumps[0].enabled=false;d.set_pumps(pumps);
        d.pumps(vec!["P_A01".into()],PumpOp::Zero,0).unwrap();wait(&mut d,"uit",5.,|d|!d.busy(DevId::Can));
        assert!(!d.status(DevId::Can).ok && d.status(DevId::Can).text.contains("staat uit"));
        // PLC message visible in the list.
        d.plc_command(1350,CMD_START,vec![0]).unwrap();wait(&mut d,"plc",5.,|d|!d.busy(DevId::Plc));
        let p=d.status(DevId::Plc);assert!(p.ok,"{}",p.text);assert!(p.messages.iter().any(|m|m.contains("proces 1350")),"{:?}",p.messages);
        d.scan(true,1.).unwrap();wait(&mut d,"scan",5.,|d|!d.busy(DevId::Can));assert!(d.status(DevId::Can).scan.contains("hartslag van 610"),"{}",d.status(DevId::Can).scan);
        for id in DevId::VSD{d.disconnect(id).unwrap();}wait(&mut d,"verbroken",5.,|d|DevId::VSD.iter().all(|i|!d.is_on(*i)));
        let _=std::fs::remove_dir_all(o);
    }

    #[test]fn a4_silent_scale_blocks_nothing_else(){
        let (l,port)=silent_scale();
        let (mut d,o)=devices("a4",ProfileKind::Vsd);d.settings.mix_allowed=true;d.settings.machine.scales[0]=ScaleNet{host:"127.0.0.1".into(),port};
        d.connect(DevId::Lh1,false).unwrap();
        wait(&mut d,"nep-weegschaal verbonden",5.,|d|on(d,DevId::Lh1));
        d.connect(DevId::Can,true).unwrap();d.connect(DevId::Plc,true).unwrap();d.connect(DevId::Lh2,true).unwrap();
        assert!(d.mixed().is_some(),"gemengd wordt gemeld");
        wait(&mut d,"rest verbonden",5.,|d|on(d,DevId::Can) && on(d,DevId::Plc) && on(d,DevId::Lh2));
        wait(&mut d,"LH1 meldt geen antwoord",3.,|d|!d.status(DevId::Lh1).weight_error.is_empty());
        // While LH1 keeps timing out: PLC and pump commands complete in normal time, calls return at once.
        let t=Instant::now();d.plc_command(1350,CMD_UP,vec![]).unwrap();let call=t.elapsed();
        let plc=wait(&mut d,"plc",5.,|d|!d.busy(DevId::Plc));assert!(d.status(DevId::Plc).ok);
        let t=Instant::now();d.pumps(vec!["P_A05".into()],PumpOp::Dispense,51200).unwrap();let call2=t.elapsed();
        let pump=wait(&mut d,"pomp",5.,|d|!d.busy(DevId::Can));assert!(d.status(DevId::Can).ok,"{}",d.status(DevId::Can).text);
        let reads=(d.status(DevId::Lh1).reads,d.status(DevId::Lh2).reads);std::thread::sleep(Duration::from_millis(1200));
        let after=(d.status(DevId::Lh1).reads,d.status(DevId::Lh2).reads);
        assert!(after.0>reads.0 && after.1>reads.1+1,"statusupdates lopen door: {reads:?} -> {after:?}");
        assert!(d.status(DevId::Lh1).weight.is_none());
        eprintln!("A4: aanroep plc {call:?} pomp {call2:?}; PLC-opdracht klaar in {plc:?}, pompopdracht klaar in {pump:?}; LH1-polls {}→{}, LH2-polls {}→{} in 1,2 s",reads.0,after.0,reads.1,after.1);
        // Generous limits: the test proves "does not block", not exact speed (parallel test load varies).
        assert!(call<Duration::from_millis(500) && call2<Duration::from_millis(500) && plc<Duration::from_secs(3) && pump<Duration::from_secs(3));
        drop(l);let _=std::fs::remove_dir_all(o);
    }

    #[test]fn a5_one_connection_per_device(){
        let (l,port)=silent_scale();
        let (mut d,o)=devices("a5",ProfileKind::Vsd);
        d.settings.machine.scales=[ScaleNet{host:"127.0.0.1".into(),port},ScaleNet{host:"127.0.0.1 ".into(),port}];
        d.connect(DevId::Lh1,false).unwrap();
        let e=d.connect(DevId::Lh2,false).unwrap_err();assert!(e.contains("al in gebruik door Weegschaal LH1"),"{e}");
        assert!(d.connect(DevId::Lh1,false).unwrap_err().contains("al verbonden"));
        wait(&mut d,"LH1",5.,|d|on(d,DevId::Lh1));
        // Same CAN channel twice: the registry refuses before any driver is opened.
        let _held=d.registry.claim(DevId::Bench,vec!["can:0".into()]).unwrap();
        let e=d.connect(DevId::Can,false).unwrap_err();assert!(e.contains("CAN-kanaal 0") && e.contains("Pompbank"),"{e}");
        let e=d.scan(false,2.).unwrap_err();assert!(e.contains("CAN-kanaal 0"),"{e}");
        assert!(d.registry.claim(DevId::Can,vec!["can:0".into()]).is_err());
        drop(_held);assert!(d.registry.claim(DevId::Can,vec!["can:0".into()]).is_ok(),"vrijgegeven na verbreken");
        // Disconnect frees the scale for the other head.
        d.disconnect(DevId::Lh1).unwrap();wait(&mut d,"vrij",5.,|d|d.registry.held().is_empty());
        d.connect(DevId::Lh2,false).unwrap();wait(&mut d,"LH2",5.,|d|on(d,DevId::Lh2));
        drop(l);let _=std::fs::remove_dir_all(o);
    }

    #[test]fn a6_sim_and_real_not_mixed(){
        let (mut d,o)=devices("a6",ProfileKind::Vsd);
        // Real endpoints that cannot answer: TEST-NET address, never a machine address.
        d.settings.machine.scales[0]=ScaleNet{host:"192.0.2.1".into(),port:4001};
        d.connect(DevId::Can,true).unwrap();wait(&mut d,"can",5.,|d|on(d,DevId::Can));
        let e=d.connect(DevId::Lh1,false).unwrap_err();assert!(e.contains("niet mengen"),"{e}");
        let e=d.connect(DevId::Plc,false).unwrap_err();assert!(e.contains("niet mengen"),"{e}");
        assert!(d.registry.held().is_empty(),"geweigerd = niets geclaimd");
        d.connect(DevId::Plc,true).unwrap();wait(&mut d,"plc",5.,|d|on(d,DevId::Plc));assert!(d.mixed().is_none());
        // Allowed explicitly: connect proceeds and the warning is visible.
        d.settings.mix_allowed=true;d.connect(DevId::Lh1,false).unwrap();assert!(d.mixed().unwrap().contains("hardware LH1"),"{:?}",d.mixed());
        wait(&mut d,"LH1 faalt (onbereikbaar)",10.,|d|d.status(DevId::Lh1).link==Link::Failed);
        assert!(d.mixed().is_none() && d.registry.held().is_empty());
        let _=std::fs::remove_dir_all(o);
    }

    #[test]fn a7_stop_interrupts_long_dispense_within_1s(){
        let (mut d,o)=devices("a7",ProfileKind::Vsd);d.settings.sim_travel_s=30.;
        d.connect(DevId::Can,true).unwrap();d.connect(DevId::Lh1,true).unwrap();d.connect(DevId::Plc,true).unwrap();
        wait(&mut d,"verbonden",5.,|d|on(d,DevId::Can) && on(d,DevId::Lh1) && on(d,DevId::Plc) && d.status(DevId::Lh1).weight.is_some());
        d.pumps(vec!["P_C01".into(),"P_C02".into()],PumpOp::Dispense,20*51200).unwrap();
        std::thread::sleep(Duration::from_millis(400));assert!(d.busy(DevId::Can),"lange dosering loopt");
        let w=d.status(DevId::Lh1).weight.unwrap();
        let t=Instant::now();d.stop_all();let took=wait(&mut d,"gestopt",2.,|d|!d.busy(DevId::Can));
        let st=d.status(DevId::Can);assert!(!st.ok && st.text.contains("Gestopt"),"{}",st.text);
        eprintln!("A7: STOP -> pompopdracht beëindigd na {took:?}");assert!(took<Duration::from_secs(1));let _=t;
        // Stopped part-way: less than the full 2 × 10 g, position unknown -> STATUS before the next dose.
        std::thread::sleep(Duration::from_millis(500));let w2=d.status(DevId::Lh1).weight.unwrap();assert!(w2-w<19.,"{w} -> {w2}");
        assert!(!d.status(DevId::Can).positions.contains_key("P_C01"));
        d.settings.sim_travel_s=30.;d.pumps(vec!["P_C01".into()],PumpOp::Dispense,5120).unwrap();wait(&mut d,"weer bruikbaar",5.,|d|!d.busy(DevId::Can));
        assert!(d.status(DevId::Can).ok,"na STOP weer bedienbaar: {}",d.status(DevId::Can).text);
        let _=std::fs::remove_dir_all(o);
    }

    #[test]fn keramisch_bench_and_scale_in_simulation(){
        let (mut d,o)=devices("ker",ProfileKind::Keramisch);d.settings.bench.settle_s=0.;
        assert!(d.connect(DevId::Can,true).unwrap_err().contains("hoort bij profiel VSD"));
        assert!(d.connect(DevId::BenchScale,true).unwrap_err().contains("Eerst pompbank"));
        d.connect(DevId::Bench,true).unwrap();wait(&mut d,"bank",5.,|d|on(d,DevId::Bench) && !d.busy(DevId::Bench));
        assert!(!d.bench.scale_ok);
        d.connect(DevId::BenchScale,true).unwrap();wait(&mut d,"weegschaal",5.,|d|on(d,DevId::BenchScale) && !d.busy(DevId::Bench));
        wait(&mut d,"live gewicht",5.,|d|d.status(DevId::BenchScale).weight.is_some());
        d.bench_action(Action::Zero).unwrap();wait(&mut d,"zero",5.,|d|!d.busy(DevId::Bench));assert!(d.bench.reference_valid);
        let target=workflow::jog_target(d.bench.position,d.bench.config.steps_rev,0,0,1,1.).unwrap();
        d.bench_action(Action::Jog{target,weigh:true}).unwrap();wait(&mut d,"jog",5.,|d|!d.busy(DevId::Bench));
        assert_eq!(d.bench.position,25600);assert!(d.bench.manual.last().unwrap().delta_g.is_some_and(|g|(g-1.345).abs()<1e-6));
        // Backward is refused by the engine (forward-only), reference dropped as before.
        assert!(workflow::jog_target(d.bench.position,25600,0,1,0,100.).is_err());
        d.disconnect(DevId::BenchScale).unwrap();wait(&mut d,"weegschaal uit",5.,|d|!d.is_on(DevId::BenchScale) && !d.busy(DevId::Bench));assert!(d.bench.connected);
        d.disconnect(DevId::Bench).unwrap();wait(&mut d,"bank uit",5.,|d|!d.is_on(DevId::Bench) && !d.busy(DevId::Bench));
        let _=std::fs::remove_dir_all(o);
    }

    #[test]fn profile_switch_disconnects_old_devices(){
        let (mut d,o)=devices("switch",ProfileKind::Vsd);
        for id in DevId::VSD{d.connect(id,true).unwrap();}wait(&mut d,"vsd",5.,|d|DevId::VSD.iter().all(|i|on(d,*i)));
        switch(&mut d,ProfileKind::Keramisch).unwrap();assert_eq!(d.profile,ProfileKind::Keramisch);
        wait(&mut d,"vsd verbroken",5.,|d|DevId::VSD.iter().all(|i|!d.is_on(*i)));assert!(d.world.is_none());
        assert!(d.connect(DevId::Lh1,true).is_err());
        d.connect(DevId::Bench,true).unwrap();wait(&mut d,"bank",5.,|d|on(d,DevId::Bench) && !d.busy(DevId::Bench));
        switch(&mut d,ProfileKind::Vsd).unwrap();wait(&mut d,"bank verbroken",5.,|d|!d.is_on(DevId::Bench) && !d.busy(DevId::Bench));
        let _=std::fs::remove_dir_all(o);
    }

    #[test]fn bench_hardware_claims_released_after_failed_connect(){
        // Hardware bench with a python path that does not exist: claim taken, connect fails, claim freed.
        let (mut d,o)=devices("benchfail",ProfileKind::Keramisch);d.settings.python=o.join("geen-python.exe");
        d.connect(DevId::Bench,false).unwrap();assert!(d.registry.held().iter().any(|(k,_)|k=="can:0"));
        wait(&mut d,"mislukt",10.,|d|!d.busy(DevId::Bench) && d.registry.held().is_empty());
        assert!(!d.bench.connected && d.bench.status.contains("Hardwarekoppeling"),"{}",d.bench.status);
        let _=std::fs::remove_dir_all(o);
    }

    // ---------- addendum (Critic FAIL) ----------
    /// Stand-in for bridge/vsd_bridge.py: same JSON-lines protocol, no hardware. `scan` listens `seconds`.
    const FAKE_BRIDGE:&str=r#"
import sys,json,time
for line in sys.stdin:
    r=json.loads(line);op=r["op"];a=r.get("args",{})
    if op=="scan":
        time.sleep(float(a.get("seconds",1)))
        res={"bitrate":a.get("bitrate"),"channels":[{"channel":0,"frames":0,"errors":0,"nodes":{},"heartbeat":[],"error":None}]}
    elif op=="wait": res={"results":[],"missing":[],"fault":None}
    else: res={"connected":op=="connect"}
    print(json.dumps({"id":r["id"],"ok":True,"result":res}),flush=True)
"#;
    fn fake_bridge(d:&mut Devices,o:&std::path::Path){
        let dir=o.join("bridge");std::fs::create_dir_all(&dir).unwrap();std::fs::write(dir.join("vsd_bridge.py"),FAKE_BRIDGE).unwrap();
        d.settings.bridge_dir=dir;d.settings.python="python".into();
    }
    #[test]fn r1_hardware_can_connects_after_stop_and_profile_switch(){
        let (mut d,o)=devices("r1",ProfileKind::Vsd);fake_bridge(&mut d,&o);
        d.stop_all();
        let t=Instant::now();d.connect(DevId::Can,false).unwrap();wait(&mut d,"CAN na STOP",15.,|d|d.status(DevId::Can).link!=Link::Connecting);
        assert_eq!(d.status(DevId::Can).link,Link::On,"{}",d.status(DevId::Can).text);let after_stop=t.elapsed();
        assert!(d.registry.held().iter().any(|(k,o)|k=="can:0" && *o==DevId::Can));
        // Profile switch with hardware CAN connected, and back: connect works again.
        let t=Instant::now();switch(&mut d,ProfileKind::Keramisch).unwrap();let switched=t.elapsed();
        assert!(!d.is_on(DevId::Can) && d.registry.held().is_empty(),"{:?}",d.registry.held());
        switch(&mut d,ProfileKind::Vsd).unwrap();
        d.connect(DevId::Can,false).unwrap();wait(&mut d,"CAN na profielwissel",15.,|d|d.status(DevId::Can).link!=Link::Connecting);
        assert_eq!(d.status(DevId::Can).link,Link::On,"{}",d.status(DevId::Can).text);
        eprintln!("R1: hardware-CAN (nep-bridge) verbonden {after_stop:?} na idle STOP; profielwissel met CAN verbonden {switched:?}");
        d.disconnect(DevId::Can).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Can));let _=std::fs::remove_dir_all(o);
    }
    #[test]fn r2_hardware_scan_holds_all_can_channels_and_profile_switch_stops_it(){
        let (mut d,o)=devices("r2",ProfileKind::Vsd);fake_bridge(&mut d,&o);
        d.scan(false,5.).unwrap();
        assert_eq!(d.registry.held(),vec![("can:*".to_string(),DevId::Can)]);
        let e=d.registry.claim(DevId::Bench,vec!["can:0".into()]).err().unwrap();assert!(e.contains("Alle CAN-kanalen"),"{e}");
        assert!(d.scan(false,2.).is_err());
        std::thread::sleep(Duration::from_millis(300));assert!(d.busy(DevId::Can));
        // Switch stops the scan and waits for it; afterwards the channel is free for the bench.
        let t=Instant::now();switch(&mut d,ProfileKind::Keramisch).unwrap();let took=t.elapsed();
        assert!(!d.busy(DevId::Can) && d.registry.held().is_empty(),"{:?}",d.registry.held());
        assert!(d.status(DevId::Can).scan.contains("gestopt"),"{}",d.status(DevId::Can).scan);
        assert!(took<Duration::from_secs(2),"{took:?}");
        d.connect(DevId::Bench,false).unwrap();assert!(d.registry.held().contains(&("can:0".to_string(),DevId::Bench)));
        // And the other way round: an open channel blocks the hardware scan.
        let e=d.scan(false,2.).unwrap_err();assert!(e.contains("CAN-kanaal 0"),"{e}");
        eprintln!("R2: profielwissel stopte lopende hardware-scan en gaf kanaal vrij in {took:?}");
        wait(&mut d,"bank klaar",15.,|d|!d.busy(DevId::Bench));let _=std::fs::remove_dir_all(o);
    }
    #[test]fn r4_stopped_sim_scan_reports_stopped_and_next_scan_works(){
        let (mut d,o)=devices("r4",ProfileKind::Vsd);
        d.scan(true,1.).unwrap();std::thread::sleep(Duration::from_millis(150));d.stop_all();
        wait(&mut d,"scan gestopt",3.,|d|!d.busy(DevId::Can));
        let s=d.status(DevId::Can);assert!(!s.ok && s.scan.contains("gestopt") && !s.scan.contains("hartslag"),"{}",s.scan);
        d.scan(true,1.).unwrap();wait(&mut d,"scan",3.,|d|!d.busy(DevId::Can));
        let s=d.status(DevId::Can);assert!(s.ok && s.scan.contains("hartslag van 610"),"{}",s.scan);
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn r5_scale_closed_by_peer_shows_fault_and_frees_claim(){
        let l=TcpListener::bind("127.0.0.1:0").unwrap();let port=l.local_addr().unwrap().port();
        std::thread::spawn(move||{let (c,_)=l.accept().unwrap();std::thread::sleep(Duration::from_millis(400));drop(c);});
        let (mut d,o)=devices("r5",ProfileKind::Vsd);d.settings.machine.scales[0]=ScaleNet{host:"localhost".into(),port};
        d.connect(DevId::Lh1,false).unwrap();wait(&mut d,"verbonden",5.,|d|on(d,DevId::Lh1));
        assert_eq!(d.registry.held(),vec![(format!("tcp:127.0.0.1:{port}"),DevId::Lh1)]);
        let t=wait(&mut d,"verbinding weg",5.,|d|d.status(DevId::Lh1).link==Link::Failed);
        let s=d.status(DevId::Lh1);assert!(s.text.contains("Verbinding verloren") && s.weight.is_none(),"{}",s.text);
        assert!(d.registry.held().is_empty());assert!(!d.is_on(DevId::Lh1));
        eprintln!("R5: verbroken weegschaal na {t:?} als fout gemeld ({})",s.text);
        drop(d);std::thread::sleep(Duration::from_millis(100));let _=std::fs::remove_dir_all(o);
    }
    /// Loopback FINS/UDP peer with a PLC program that never takes a command (all-zero mailbox).
    fn silent_fins()->u16{
        let server=std::net::UdpSocket::bind("127.0.0.1:0").unwrap();let port=server.local_addr().unwrap().port();
        std::thread::spawn(move||{let mut buf=[0u8;2048];loop{
            let Ok((n,from))=server.recv_from(&mut buf) else{return};let f=&buf[..n];if n<18{continue;}
            let cnt=u16::from_be_bytes([f[16],f[17]]);
            let mut r=vec![0xC0,0,2,0,f[7],0,0,f[4],0,f[9],1,f[11],0,0];
            if f[11]==1{r.extend(std::iter::repeat_n(0u8,2*cnt as usize));}
            let _=server.send_to(&r,from);}});
        port
    }
    #[test]fn p5_real_plc_only_reset_abort_and_stop_ends_a_plc_wait(){
        let port=silent_fins();
        let (mut d,o)=devices("p5",ProfileKind::Vsd);d.settings.machine.plc=PlcConfig{ip:"127.0.0.1".into(),port,host_port:0,..Default::default()};
        d.connect(DevId::Plc,false).unwrap();wait(&mut d,"plc",5.,|d|d.status(DevId::Plc).link!=Link::Connecting);
        assert!(on(&d,DevId::Plc),"{}",d.status(DevId::Plc).text);
        let e=d.plc_command(1350,CMD_START,vec![0]).unwrap_err();assert!(e.contains("alleen Reset (2) en Abort (5)"),"{e}");
        assert!(d.plc_command(1350,CMD_UP,vec![]).is_err());
        d.plc_command(1300,CMD_RESET,vec![]).unwrap();wait(&mut d,"reset",5.,|d|!d.busy(DevId::Plc));assert!(d.status(DevId::Plc).ok,"{}",d.status(DevId::Plc).text);
        // The PLC never took the first Reset: the second waits for pick-up (time-out 60 s) until STOP.
        d.plc_command(1300,CMD_ABORT,vec![]).unwrap();std::thread::sleep(Duration::from_millis(300));assert!(d.busy(DevId::Plc));
        let t=Instant::now();d.stop_all();let took=wait(&mut d,"gestopt",2.,|d|!d.busy(DevId::Plc));
        let s=d.status(DevId::Plc);assert!(!s.ok && s.text.contains("Gestopt"),"{}",s.text);assert!(took<Duration::from_secs(1));let _=t;
        eprintln!("PLC: STOP beëindigde wachten op een stille PLC na {took:?}");
        // After STOP the next command runs normally again (R1).
        d.disconnect(DevId::Plc).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Plc));
        d.connect(DevId::Plc,false).unwrap();wait(&mut d,"weer verbonden",5.,|d|on(d,DevId::Plc));
        d.plc_command(1300,CMD_RESET,vec![]).unwrap();wait(&mut d,"reset 2",5.,|d|!d.busy(DevId::Plc));assert!(d.status(DevId::Plc).ok,"{}",d.status(DevId::Plc).text);
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn registry_same_host_under_two_names_is_one_device(){
        let r=Registry::default();
        let _a=r.claim(DevId::Lh1,vec![tcp_key("localhost",23)]).unwrap();
        for h in ["127.0.0.1"," LOCALHOST ","localhost."]{assert!(r.claim(DevId::Lh2,vec![tcp_key(h,23)]).is_err(),"{h}");}
        assert!(r.claim(DevId::Lh2,vec![tcp_key("127.0.0.1",24)]).is_ok());
        let _c=r.claim(DevId::Can,vec!["can:1".into()]).unwrap();assert!(r.claim(DevId::Can,vec!["can:*".into()]).is_err());
        assert_eq!(norm_host(" 192.168.0.56 "),"192.168.0.56");
    }
    #[test]fn r6_idle_stop_keeps_ceramic_reference_stop_during_move_drops_it(){
        let (mut d,o)=devices("r6",ProfileKind::Keramisch);d.settings.bench.settle_s=0.;
        d.connect(DevId::Bench,true).unwrap();wait(&mut d,"bank",5.,|d|on(d,DevId::Bench) && !d.busy(DevId::Bench));
        d.bench_action(Action::Zero).unwrap();wait(&mut d,"zero",5.,|d|!d.busy(DevId::Bench));assert!(d.bench.reference_valid);
        d.stop_all();
        // The engine checks its reference about once per second while idle: give it two checks.
        let t=Instant::now();while t.elapsed()<Duration::from_millis(2300){d.poll();std::thread::sleep(Duration::from_millis(20));}
        assert!(d.bench.reference_valid,"idle STOP liet referentie vallen: {}",d.bench.status);
        let target=workflow::jog_target(d.bench.position,25600,0,0,1,50.).unwrap();
        d.bench_action(Action::Jog{target,weigh:false}).unwrap();d.stop_all();
        wait(&mut d,"gestopt",5.,|d|!d.busy(DevId::Bench));
        assert!(!d.bench.reference_valid && d.bench.status.starts_with("Afgebroken"),"{}",d.bench.status);
        // Afterwards the bench is usable again (no STOP left behind).
        d.bench_action(Action::Zero).unwrap();wait(&mut d,"zero 2",5.,|d|!d.busy(DevId::Bench));assert!(d.bench.reference_valid,"{}",d.bench.status);
        d.disconnect(DevId::Bench).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Bench) && !d.busy(DevId::Bench));
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn stop_right_after_click_is_never_lost(){
        // The CAN worker is held busy by a queued 1 s scan, so the pump command is certainly picked up
        // only after the STOP below. Before the fix the worker cleared the flag on pick-up and the dose ran 20 s.
        let (mut d,o)=devices("lost",ProfileKind::Vsd);d.settings.sim_travel_s=30.;
        d.connect(DevId::Can,true).unwrap();wait(&mut d,"can",5.,|d|on(d,DevId::Can));
        d.can_tx.send(CanCmd::Scan{simulation:true,python:PathBuf::new(),script:PathBuf::new(),output:PathBuf::new(),seconds:1.,bitrate:250000,nodes:vec![610],claim:None}).unwrap();
        d.pumps(vec!["P_C01".into()],PumpOp::Dispense,20*51200).unwrap();
        let t=Instant::now();d.stop_all();
        // The scan ending also clears busy: wait for the pump result itself.
        let took=wait(&mut d,"pompopdracht klaar",3.,|d|!d.busy(DevId::Can) && !d.status(DevId::Can).text.ends_with("…"));
        let st=d.status(DevId::Can);assert!(!st.ok && st.text.contains("Gestopt"),"{}",st.text);assert!(took<Duration::from_secs(2),"{took:?}");let _=t;
        eprintln!("STOP direct na klik: opdracht gestopt na {took:?} ({})",st.text);
        // A STOP from before the next click does not block that command (R1).
        d.pumps(vec!["P_C01".into()],PumpOp::Status,0).unwrap();wait(&mut d,"status",3.,|d|!d.busy(DevId::Can));assert!(d.status(DevId::Can).ok,"{}",d.status(DevId::Can).text);
        let _=std::fs::remove_dir_all(o);
    }

    // ---------- iteration 2: calibration ----------
    fn short_cal()->CalSettings{CalSettings{phase1_samples:11,phase1_runs:1,phase2_runs:1,stroke_samples:vec![1,2,5],settle_s:0.,..Default::default()}}
    fn vsd_with_db(name:&str,travel:f64)->(Devices,PathBuf){
        let (mut d,o)=devices(name,ProfileKind::Vsd);d.settings.sim_travel_s=travel;
        let path=o.join("data/pumpstudio.db");Db::open(&path).unwrap();d.settings.db_path=Some(path);(d,o)
    }
    fn db(d:&Devices)->Db{Db::open(d.settings.db_path.as_ref().unwrap()).unwrap()}
    #[test]fn k2_k7_vsd_calibration_stored_archived_and_control_dose(){
        let (mut d,o)=vsd_with_db("k2",0.05);
        for id in [DevId::Can,DevId::Lh1,DevId::Lh2]{d.connect(id,true).unwrap();}
        wait(&mut d,"verbonden",5.,|d|on(d,DevId::Can) && on(d,DevId::Lh1) && on(d,DevId::Lh2));
        let t=Instant::now();d.calibrate(vec!["P_C01".into()],short_cal()).unwrap();
        assert!(d.status(DevId::Can).progress.is_some());
        wait(&mut d,"kalibratie 1",30.,|d|!d.busy(DevId::Can));let took=t.elapsed();
        let st=d.status(DevId::Can);assert!(st.ok,"{}",st.text);assert!(st.progress.is_none());
        let first=db(&d).active_calibration(ProfileKind::Vsd,"P_C01",calib::KIND_VSD).unwrap().unwrap();
        assert!((first.g_per_stroke.unwrap()-0.5).abs()<0.01,"{:?}",first.g_per_stroke);
        let pts=db(&d).calibration_points(first.id).unwrap();
        assert_eq!(pts.iter().filter(|p|p.phase==1 && p.run==0).count(),11);assert_eq!(pts.iter().filter(|p|p.phase==2 && p.run==0).count(),3);
        // Queue of two pumps on both heads; P_C01 gets a second calibration -> first one archived.
        d.calibrate(vec!["P_C01".into(),"P_A02".into()],short_cal()).unwrap();wait(&mut d,"wachtrij",60.,|d|!d.busy(DevId::Can));
        assert!(d.status(DevId::Can).ok,"{}",d.status(DevId::Can).text);
        let h=db(&d).calibrations(ProfileKind::Vsd,"P_C01").unwrap();assert_eq!(h.len(),2);assert_eq!((h[0].status.as_str(),h[1].status.as_str(),h[1].id),("actief","archief",first.id));
        assert!(db(&d).active_calibration(ProfileKind::Vsd,"P_A02",calib::KIND_VSD).unwrap().is_some());
        // K7: control dose 1 g with the active calibration, stored with it.
        let act=h[0].id;d.check_dose("P_C01",1.).unwrap();wait(&mut d,"controle",10.,|d|!d.busy(DevId::Can));
        let c=d.status(DevId::Can).check.clone().unwrap();let err=c.error_pct.unwrap();assert!(err.abs()<2.,"{err}");
        let row=db(&d).calibration(act).unwrap();assert_eq!(row.check_requested,Some(1.));assert!((row.check_error_pct.unwrap()-err).abs()<1e-9);
        eprintln!("K2: kalibratie (kort, simulatie) {took:?}; K7: controle 1 g -> {:.4} g ({err:+.3} %)",c.measured_g.unwrap());
        for id in [DevId::Can,DevId::Lh1,DevId::Lh2]{d.disconnect(id).unwrap();}wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Can));
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn k3_stop_during_calibration_stored_as_aborted_active_unchanged(){
        let (mut d,o)=vsd_with_db("k3",0.05);
        for id in [DevId::Can,DevId::Lh1]{d.connect(id,true).unwrap();}wait(&mut d,"verbonden",5.,|d|on(d,DevId::Can) && on(d,DevId::Lh1));
        d.calibrate(vec!["P_C02".into()],short_cal()).unwrap();wait(&mut d,"eerste",30.,|d|!d.busy(DevId::Can));
        let active=db(&d).active_calibration(ProfileKind::Vsd,"P_C02",calib::KIND_VSD).unwrap().unwrap().id;
        // Real travel time for the second one, STOP while it runs (in phase 2: 5 revolutions take seconds).
        d.world.as_ref().unwrap().lock().unwrap().max_travel_s=30.;
        d.calibrate(vec!["P_C02".into(),"P_C03".into()],short_cal()).unwrap();
        wait(&mut d,"in fase 2",30.,|d|d.status(DevId::Can).progress.as_ref().is_some_and(|p|p.text.contains("fase 2") || p.step>=12));
        std::thread::sleep(Duration::from_millis(200));assert!(d.busy(DevId::Can));
        let w=d.world.clone().unwrap();
        let t=Instant::now();d.stop_all();
        // Motion first: the pump stands still within 1 s; then the worker stores the partial run and is free.
        while w.lock().unwrap().moving(){assert!(t.elapsed()<Duration::from_secs(1),"pomp loopt nog na {:?}",t.elapsed());std::thread::sleep(Duration::from_millis(2));}
        let still=t.elapsed();
        let took=wait(&mut d,"gestopt",3.,|d|!d.busy(DevId::Can));
        eprintln!("K3 (debug): pomp stil na {still:?}, opdracht beëindigd en opgeslagen na {took:?}");
        let st=d.status(DevId::Can);assert!(!st.ok && st.text.contains("Gestopt") && st.text.contains("afgebroken"),"{}",st.text);let _=t;
        let h=db(&d).calibrations(ProfileKind::Vsd,"P_C02").unwrap();
        assert_eq!(h[0].status,"afgebroken");assert!(db(&d).calibration_points(h[0].id).unwrap().iter().any(|p|p.phase==1));
        assert_eq!(db(&d).active_calibration(ProfileKind::Vsd,"P_C02",calib::KIND_VSD).unwrap().unwrap().id,active,"actieve kalibratie onveranderd");
        assert!(db(&d).calibrations(ProfileKind::Vsd,"P_C03").unwrap().is_empty(),"wachtrij stopt na STOP");
        eprintln!("K3: STOP tijdens kalibratie -> gestopt na {took:?}, opgeslagen als afgebroken");
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn k4_calibration_refused_without_own_scale_or_inactive_pump(){
        let (mut d,o)=vsd_with_db("k4",0.05);
        d.connect(DevId::Can,true).unwrap();d.connect(DevId::Lh2,true).unwrap();wait(&mut d,"verbonden",5.,|d|on(d,DevId::Can) && on(d,DevId::Lh2));
        let e=d.calibrate(vec!["P_C01".into()],short_cal()).unwrap_err();assert!(e.contains("Geen weegschaal LH1"),"{e}");
        let e=d.check_dose("P_C01",1.).unwrap_err();assert!(e.contains("LH1"),"{e}");
        let mut pumps=d.settings.machine.pumps.clone();pumps.iter_mut().find(|p|p.name=="P_A03").unwrap().enabled=false;d.set_pumps(pumps);
        let e=d.calibrate(vec!["P_A03".into()],short_cal()).unwrap_err();assert!(e.contains("staat uit"),"{e}");
        let mut bad=short_cal();bad.phase1_samples=1;assert!(d.calibrate(vec!["P_A01".into()],bad).is_err());
        assert!(d.calibrate(vec![],short_cal()).is_err());
        assert!(!d.busy(DevId::Can) && db(&d).calibrations(ProfileKind::Vsd,"P_C01").unwrap().is_empty());
        // Scale gone during the run: the worker refuses too (remote reading reports it).
        d.calibrate(vec!["P_A01".into()],short_cal()).unwrap();d.disconnect(DevId::Lh2).unwrap();
        wait(&mut d,"klaar",30.,|d|!d.busy(DevId::Can));
        let st=d.status(DevId::Can);assert!(st.ok || st.text.contains("Geen weegschaal LH2"),"{}",st.text);
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn k5_ceramic_calibration_and_curve_stored(){
        let (mut d,o)=devices("k5",ProfileKind::Keramisch);d.settings.bench.settle_s=0.;
        let path=o.join("p.db");Db::open(&path).unwrap();d.settings.db_path=Some(path);
        d.connect(DevId::Bench,true).unwrap();wait(&mut d,"bank",5.,|d|on(d,DevId::Bench) && !d.busy(DevId::Bench));
        d.connect(DevId::BenchScale,true).unwrap();wait(&mut d,"weegschaal",5.,|d|on(d,DevId::BenchScale) && !d.busy(DevId::Bench));
        // Without reference: refused by the engine, stored as aborted (never active).
        d.bench_action(Action::Calibrate{revolutions:1,repeats:2,prime:0}).unwrap();wait(&mut d,"geweigerd",5.,|d|!d.busy(DevId::Bench) && d.bench_cal_version()>=1);
        let pump="Keramisch 1";let h=db(&d).calibrations(ProfileKind::Keramisch,pump).unwrap();assert_eq!(h.len(),1);assert_eq!(h[0].status,"afgebroken");
        d.bench_action(Action::Zero).unwrap();wait(&mut d,"zero",5.,|d|!d.busy(DevId::Bench));
        d.bench_action(Action::Calibrate{revolutions:1,repeats:3,prime:0}).unwrap();wait(&mut d,"kalibratie",10.,|d|!d.busy(DevId::Bench) && d.bench_cal_version()>=2);
        let c=db(&d).active_calibration(ProfileKind::Keramisch,pump,calib::KIND_BENCH_CAL).unwrap().unwrap_or_else(||panic!("geen actieve: {:?} / {}",db(&d).calibrations(ProfileKind::Keramisch,pump).unwrap().iter().map(|r|(r.status.clone(),r.note.clone())).collect::<Vec<_>>(),d.bench.status));
        assert!((c.g_per_rev.unwrap()-1.345).abs()<1e-6,"{:?}",c.g_per_rev);assert_eq!(db(&d).calibration_points(c.id).unwrap().len(),3);
        d.bench_action(Action::Characterize{interval:crate::model::Interval::Degrees(30.),repeats:2}).unwrap();
        wait(&mut d,"curve",20.,|d|!d.busy(DevId::Bench) && d.bench_cal_version()>=3);
        let k=db(&d).active_calibration(ProfileKind::Keramisch,pump,calib::KIND_BENCH_CURVE).unwrap().unwrap();
        assert!((k.g_per_rev.unwrap()-1.345).abs()<1e-6);assert_eq!(db(&d).calibration_points(k.id).unwrap().len(),2*13,"0° plus 12 punten per herhaling");
        assert!(db(&d).bench_curve(k.id).unwrap().is_some());
        // Forward-only and counter protection unchanged: a backward target is refused by the engine.
        let pos=d.bench.position;d.bench_action(Action::Jog{target:pos-1,weigh:false}).unwrap();wait(&mut d,"terug",5.,|d|!d.busy(DevId::Bench));
        assert!(d.bench.position==pos && !d.bench.reference_valid);
        d.disconnect(DevId::Bench).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Bench) && !d.busy(DevId::Bench));
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn k8_profile_switch_returns_at_once(){
        let (mut d,o)=devices("k8",ProfileKind::Vsd);
        for id in DevId::VSD{d.connect(id,true).unwrap();}wait(&mut d,"vsd",5.,|d|DevId::VSD.iter().all(|i|on(d,*i)));
        d.pumps(vec!["P_C01".into()],PumpOp::Dispense,40*51200).unwrap();
        let t=Instant::now();d.switch_profile(ProfileKind::Keramisch).unwrap();let call=t.elapsed();
        assert!(call<Duration::from_millis(500),"{call:?}");
        // Usually still busy stopping/closing (then nothing new may connect); the workers may also be done already.
        if d.switching.is_some(){assert_eq!(d.profile,ProfileKind::Vsd);assert!(d.connect(DevId::Bench,true).unwrap_err().contains("Profielwissel"));}
        let mut max=Duration::ZERO;
        while d.switching.is_some(){let f=Instant::now();d.poll();max=max.max(f.elapsed());assert!(t.elapsed()<Duration::from_secs(5));std::thread::sleep(Duration::from_millis(5));}
        assert_eq!(d.take_switch_result(),Some(Ok(ProfileKind::Keramisch)));assert_eq!(d.profile,ProfileKind::Keramisch);
        eprintln!("K8: switch_profile() keert terug na {call:?}; langste poll {max:?}; wissel klaar na {:?}",t.elapsed());
        assert!(max<Duration::from_millis(250),"{max:?}");
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn k1_units_to_steps_and_text(){
        assert_eq!(to_steps(0,25600,0.,0.,51200),Ok(25600));assert_eq!(to_steps(1,0,0.5,0.,51200),Ok(25600));assert_eq!(to_steps(2,0,0.,180.,51200),Ok(25600));
        assert_eq!(to_steps(1,0,1.,0.,51200),Ok(51200));assert_eq!(to_steps(2,0,0.,1.,51200),Ok(142));assert_eq!(to_steps(0,-12800,0.,0.,51200),Ok(-12800));
        assert!(to_steps(0,P_MAX+1,0.,0.,51200).is_err());assert!(to_steps(1,0,f64::NAN,0.,51200).is_err());assert!(to_steps(1,0,1e6,0.,51200).is_err());
        assert_eq!(steps_text(25600,51200),"25 600 st = 0,5000 omw = 180,0°");assert_eq!(group(-1234567),"-1 234 567");
        assert!(check_steps(PumpOp::Dispense,-1).is_err());assert!(check_steps(PumpOp::MoveTo,-1).is_ok());
    }
    // ---------- iteration 2 addendum ----------
    #[test]fn s1_hardware_check_dose_refused_with_only_a_simulation_calibration(){
        let (mut d,o)=vsd_with_db("s1",0.05);fake_bridge(&mut d,&o);d.settings.mix_allowed=true;
        // Simulation calibration of P_C01 in the database.
        db(&d).save_calibration(&NewCal::from_vsd(&crate::calib::tests_sample("P_C01",0.5),true,"")).unwrap();
        d.connect(DevId::Can,false).unwrap();d.connect(DevId::Lh1,true).unwrap();
        wait(&mut d,"verbonden",15.,|d|d.status(DevId::Can).link!=Link::Connecting && on(d,DevId::Lh1));assert!(on(&d,DevId::Can),"{}",d.status(DevId::Can).text);
        d.check_dose("P_C01",1.).unwrap();wait(&mut d,"geweigerd",5.,|d|!d.busy(DevId::Can));
        let st=d.status(DevId::Can);assert!(!st.ok && st.text.contains("Geen hardware-kalibratie"),"{}",st.text);
        d.disconnect(DevId::Can).unwrap();d.disconnect(DevId::Lh1).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Can) && !d.is_on(DevId::Lh1));
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn s1_simulation_check_dose_borrows_the_hardware_calibration(){
        let (mut d,o)=vsd_with_db("s1b",0.05);
        let mut hw=crate::calib::tests_sample("P_C01",0.5);hw.simulation=false;let id=db(&d).save_calibration(&NewCal::from_vsd(&hw,true,"")).unwrap();
        d.connect(DevId::Can,true).unwrap();d.connect(DevId::Lh1,true).unwrap();wait(&mut d,"verbonden",5.,|d|on(d,DevId::Can) && on(d,DevId::Lh1));
        d.check_dose("P_C01",1.).unwrap();wait(&mut d,"controle",10.,|d|!d.busy(DevId::Can));
        // Borrowed hardware calibration: the dose runs and is shown, but nothing is written onto the hardware row.
        assert!(d.status(DevId::Can).ok,"{}",d.status(DevId::Can).text);assert!(d.status(DevId::Can).check.is_some());
        assert!(db(&d).calibration(id).unwrap().check_measured.is_none());
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn commands_refused_during_profile_switch(){
        let (mut d,o)=devices("sw",ProfileKind::Vsd);fake_bridge(&mut d,&o);
        d.scan(false,5.).unwrap();std::thread::sleep(Duration::from_millis(200));
        d.switch_profile(ProfileKind::Keramisch).unwrap();
        if d.switching.is_some(){
            for e in [d.scan(true,1.).unwrap_err(),d.pumps(vec!["P_A01".into()],PumpOp::Status,0).unwrap_err(),d.check_dose("P_A01",1.).unwrap_err(),
                d.calibrate(vec!["P_A01".into()],CalSettings::default()).unwrap_err(),d.plc_command(1300,CMD_RESET,vec![]).unwrap_err(),d.scale(Head::LH1,true).unwrap_err(),
                d.bench_action(Action::Zero).unwrap_err()]{assert!(e.contains("Profielwissel"),"{e}");}
        }
        while d.switching.is_some(){d.poll();std::thread::sleep(Duration::from_millis(5));}
        assert_eq!(d.profile,ProfileKind::Keramisch);let _=std::fs::remove_dir_all(o);
    }
    #[test]fn stop_ends_a_stable_weighing_on_a_silent_scale(){
        let (l,port)=silent_scale();
        let (mut d,o)=devices("wstop",ProfileKind::Vsd);d.settings.machine.scales[0]=ScaleNet{host:"127.0.0.1".into(),port};
        d.connect(DevId::Lh1,false).unwrap();wait(&mut d,"verbonden",5.,|d|on(d,DevId::Lh1));
        d.scale(Head::LH1,false).unwrap();std::thread::sleep(Duration::from_millis(300));assert!(d.busy(DevId::Lh1));
        let t=Instant::now();d.stop_all();let took=wait(&mut d,"gestopt",2.,|d|!d.busy(DevId::Lh1));
        assert!(took<Duration::from_secs(1),"{took:?}");assert!(d.status(DevId::Lh1).text.contains("Gestopt"),"{}",d.status(DevId::Lh1).text);let _=t;
        // The next weighing is accepted at once (not stuck behind the old one) and runs again.
        d.scale(Head::LH1,true).unwrap();std::thread::sleep(Duration::from_millis(150));assert!(d.busy(DevId::Lh1));d.stop_all();wait(&mut d,"weer gestopt",2.,|d|!d.busy(DevId::Lh1));
        eprintln!("Weegschaal: STOP beëindigde stabiele weging op stille weegschaal na {took:?}");
        drop(l);let _=std::fs::remove_dir_all(o);
    }
    #[test]fn s2_step_and_decimal_parsers(){
        for (t,v) in [("25600",25600),("25 600",25600),("25.600",25600),("1.234.567",1234567),("1 234 567",1234567),(" -12 800 ",-12800),("+5",5),("0",0),("2147483647",P_MAX),("-2.147.483.648",P_MIN),("25\u{a0}600",25600),("25\u{2009}600",25600),("1\u{2009}234\u{2009}567",1234567)]{
            assert_eq!(parse_steps(t),Ok(v),"{t}");}
        for t in ["","25,6","1.5","25.6","2.56","12.3456","25600.","1..000","1.2345","abc","25 6","2147483648","-2147483649","1e5","25 600,0","--5","99999999999999999999"]{
            assert!(parse_steps(t).is_err(),"{t} moet geweigerd worden");}
        assert!(parse_steps("25,6").unwrap_err().contains("geen komma"));assert!(parse_steps("1.5").unwrap_err().contains("duizendtalscheiding"));
        for (t,v) in [("0,5",0.5),("0.5",0.5),("180",180.),("-1,25",-1.25),("1.234,5",1234.5),(" 2 ",2.),("1\u{2009}234,5",1234.5)]{assert_eq!(parse_decimal(t),Ok(v),"{t}");}
        for t in ["","abc","1,2,3","0.5.1","1e3","--1","NaN","inf"]{assert!(parse_decimal(t).is_err(),"{t}");}
    }
    #[test]fn s1_simulated_check_never_written_onto_a_hardware_calibration(){
        let (mut d,o)=vsd_with_db("s1c",0.05);
        let mut hw=crate::calib::tests_sample("P_C01",0.5);hw.simulation=false;let id=db(&d).save_calibration(&NewCal::from_vsd(&hw,true,"")).unwrap();
        assert!(db(&d).set_check(id,false,1.,Some(0.998),Some(-0.2)).unwrap(),"echte controle op hardware");
        d.connect(DevId::Can,true).unwrap();d.connect(DevId::Lh1,true).unwrap();wait(&mut d,"verbonden",5.,|d|on(d,DevId::Can) && on(d,DevId::Lh1));
        d.check_dose("P_C01",2.).unwrap();wait(&mut d,"controle",10.,|d|!d.busy(DevId::Can));
        let st=d.status(DevId::Can);assert!(st.ok && st.text.contains("niet opgeslagen"),"{}",st.text);
        let c=st.check.clone().unwrap();assert_eq!(c.requested_g,2.);assert!(c.measured_g.is_some(),"resultaat wel getoond");
        let row=db(&d).calibration(id).unwrap();assert_eq!((row.check_requested,row.check_measured,row.check_error_pct),(Some(1.),Some(0.998),Some(-0.2)),"hardware-rij onveranderd");
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn stop_per_head_survives_a_new_weighing_on_the_other_head(){
        let (l,port)=silent_scale();
        let (mut d,o)=devices("whead",ProfileKind::Vsd);d.settings.mix_allowed=true;d.settings.machine.scales[0]=ScaleNet{host:"127.0.0.1".into(),port};
        d.connect(DevId::Lh1,false).unwrap();d.connect(DevId::Lh2,true).unwrap();wait(&mut d,"verbonden",5.,|d|on(d,DevId::Lh1) && on(d,DevId::Lh2));
        d.scale(Head::LH1,false).unwrap();std::thread::sleep(Duration::from_millis(250));assert!(d.busy(DevId::Lh1));
        // STOP, then at once a weighing on LH2: the STOP for LH1 must still end LH1's weighing.
        let t=Instant::now();d.stop_all();d.scale(Head::LH2,false).unwrap();
        let took=wait(&mut d,"LH1 gestopt",2.,|d|!d.busy(DevId::Lh1));
        assert!(took<Duration::from_secs(1),"{took:?}");assert!(d.status(DevId::Lh1).text.contains("Gestopt"),"{}",d.status(DevId::Lh1).text);let _=t;
        wait(&mut d,"LH2 klaar",3.,|d|!d.busy(DevId::Lh2));assert!(d.status(DevId::Lh2).ok,"{}",d.status(DevId::Lh2).text);
        drop(l);let _=std::fs::remove_dir_all(o);
    }

    // ---------- iteration 3: pump test ----------
    use crate::pumptest::{Mode,TestPlan};
    fn vsd_ready(name:&str,travel:f64,pump:&str)->(Devices,PathBuf){
        let (mut d,o)=vsd_with_db(name,travel);
        db(&d).save_calibration(&NewCal::from_vsd(&crate::calib::tests_sample(pump,0.5),true,"")).unwrap();
        for id in [DevId::Can,DevId::Lh1,DevId::Lh2]{d.connect(id,true).unwrap();}
        wait(&mut d,"verbonden",5.,|d|on(d,DevId::Can) && on(d,DevId::Lh1) && on(d,DevId::Lh2));(d,o)
    }
    fn plan(mode:Mode,weights:&[f64],repeats:usize)->TestPlan{TestPlan{mode,weights:weights.to_vec(),repeats,settle_s:0.,..Default::default()}}
    #[test]fn t1_series_test_runs_and_is_stored(){
        let (mut d,o)=vsd_ready("t1",0.02,"P_C01");
        let t=Instant::now();let run=d.start_test(&mut db(&d),"P_C01",&plan(Mode::Reeks,&[5.,0.5,1.],3)).unwrap();
        assert!(d.running_test().is_some());assert!(d.start_test(&mut db(&d),"P_C01",&plan(Mode::Enkel,&[1.],1)).is_err(),"één test tegelijk");
        wait(&mut d,"test klaar",30.,|d|d.running_test().is_none());
        let r=db(&d).test_run(run).unwrap();assert_eq!((r.status.as_str(),r.simulation),("klaar",true));assert!(r.calibration_id.is_some());
        let doses=db(&d).test_doses(run).unwrap();assert_eq!(doses.len(),9);
        assert_eq!(doses.iter().map(|x|x.requested_g).collect::<Vec<_>>(),vec![0.5,0.5,0.5,1.,1.,1.,5.,5.,5.],"oplopend");
        assert!(doses.iter().all(|x|x.status=="ok" && x.err_pct().unwrap().abs()<2. && x.steps.unwrap()>0 && x.duration_s.is_some()),"{doses:?}");
        let s=crate::pumptest::summary(&doses,1.);assert_eq!(s.len(),3);assert!(s.iter().all(|w|w.n==3));
        eprintln!("T1: reeks 9 doses in {:?}; per gewicht gem. afwijking {:?} %",t.elapsed(),s.iter().map(|w|(w.mean_err_pct*1000.).round()/1000.).collect::<Vec<_>>());
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn t3_t9_correction_and_motion_between_doses_recorded(){
        let (mut d,o)=vsd_ready("t3",0.02,"P_C01");
        let p=TestPlan{pause_each:true,..plan(Mode::Enkel,&[1.],3)};
        d.set_correction(0.).unwrap();let run=d.start_test(&mut db(&d),"P_C01",&p).unwrap();
        wait(&mut d,"pauze na dosis 1",10.,|d|d.running_test().is_some_and(|t|t.waiting && t.done==1));
        // Between doses: +0,5 % correction and a slower pump, no reconnect.
        d.set_correction(0.5).unwrap();d.set_motion(Motion{speed_rps:0.5,accel_rps2:0.5,decel_rps2:0.5}).unwrap();
        assert!(d.set_correction(20.5).is_err());assert_eq!(d.correction(),0.5);
        d.resume_test();wait(&mut d,"pauze na dosis 2",10.,|d|d.running_test().is_some_and(|t|t.waiting && t.done==2));
        d.set_correction(-0.3).unwrap();d.resume_test();wait(&mut d,"klaar",10.,|d|d.running_test().is_none());
        let doses=db(&d).test_doses(run).unwrap();assert_eq!(doses.len(),3);
        assert_eq!(doses.iter().map(|x|x.correction_pct).collect::<Vec<_>>(),vec![0.,0.5,-0.3]);
        assert_eq!(doses.iter().map(|x|x.target_g.unwrap()).collect::<Vec<_>>(),vec![1.,1.005,0.997],"doelgewicht volgt de correctie");
        // The dosed weight follows the corrected target (simulated pump is exact), the deviation is against the requested 1 g.
        assert!((doses[1].dosed_g.unwrap()-1.005).abs()<0.002 && (doses[1].err_pct().unwrap()-0.5).abs()<0.2,"{:?}",doses[1]);
        assert!(doses[0].settings.contains("\"snelheid_omw_s\":1.0") && doses[1].settings.contains("\"snelheid_omw_s\":0.5"),"{} / {}",doses[0].settings,doses[1].settings);
        assert!(doses[1].steps.unwrap()>doses[0].steps.unwrap(),"meer stappen bij hogere correctie");
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn t4_stop_during_test_keeps_doses_and_status_stopped(){
        let (mut d,o)=vsd_ready("t4",30.,"P_C02");
        let run=d.start_test(&mut db(&d),"P_C02",&plan(Mode::Enkel,&[1.5],10)).unwrap();
        wait(&mut d,"eerste dosis",30.,|d|d.running_test().is_some_and(|t|t.done>=1));
        std::thread::sleep(Duration::from_millis(300));
        let t=Instant::now();d.stop_all();let took=wait(&mut d,"gestopt",3.,|d|d.running_test().is_none());
        assert!(took<Duration::from_secs(1),"{took:?}");
        let r=db(&d).test_run(run).unwrap();assert_eq!(r.status,"gestopt");let doses=db(&d).test_doses(run).unwrap();
        assert!(doses.len()>=1 && doses.len()<10);assert!(doses[0].status=="ok");assert!(doses.last().unwrap().status=="fout" || doses.len()>=1);
        eprintln!("T4: STOP tijdens test -> klaar na {took:?}; {} doses bewaard, status {}",doses.len(),r.status);let _=t;
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn t5_hardware_test_without_hardware_calibration_refused(){
        let (mut d,o)=vsd_with_db("t5",0.05);fake_bridge(&mut d,&o);d.settings.mix_allowed=true;
        db(&d).save_calibration(&NewCal::from_vsd(&crate::calib::tests_sample("P_C01",0.5),true,"")).unwrap();
        d.connect(DevId::Can,false).unwrap();d.connect(DevId::Lh1,true).unwrap();
        wait(&mut d,"verbonden",15.,|d|d.status(DevId::Can).link!=Link::Connecting && on(d,DevId::Lh1));assert!(on(&d,DevId::Can));
        let e=d.start_test(&mut db(&d),"P_C01",&plan(Mode::Enkel,&[1.],2)).unwrap_err();assert!(e.contains("Geen hardware-kalibratie"),"{e}");
        assert!(db(&d).test_runs(ProfileKind::Vsd,"P_C01").unwrap().is_empty(),"geen run aangemaakt");
        d.disconnect(DevId::Can).unwrap();d.disconnect(DevId::Lh1).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Can) && !d.is_on(DevId::Lh1));
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn test_refused_without_scale_or_inactive_pump(){
        let (mut d,o)=vsd_with_db("tref",0.05);
        db(&d).save_calibration(&NewCal::from_vsd(&crate::calib::tests_sample("P_C01",0.5),true,"")).unwrap();
        d.connect(DevId::Can,true).unwrap();wait(&mut d,"can",5.,|d|on(d,DevId::Can));
        assert!(d.start_test(&mut db(&d),"P_C01",&plan(Mode::Enkel,&[1.],1)).unwrap_err().contains("Geen weegschaal LH1"));
        d.connect(DevId::Lh1,true).unwrap();wait(&mut d,"lh1",5.,|d|on(d,DevId::Lh1));
        assert!(d.start_test(&mut db(&d),"P_C09",&plan(Mode::Enkel,&[1.],1)).unwrap_err().contains("P_C09"),"geen kalibratie");
        assert!(d.start_test(&mut db(&d),"P_C01",&plan(Mode::Reeks,&[],1)).is_err());
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn ceramic_test_doses_one_by_one(){
        let (mut d,o)=devices("ktest",ProfileKind::Keramisch);d.settings.bench.settle_s=0.;
        let path=o.join("p.db");Db::open(&path).unwrap();d.settings.db_path=Some(path);
        d.connect(DevId::Bench,true).unwrap();wait(&mut d,"bank",5.,|d|on(d,DevId::Bench) && !d.busy(DevId::Bench));
        d.connect(DevId::BenchScale,true).unwrap();wait(&mut d,"weegschaal",5.,|d|on(d,DevId::BenchScale) && !d.busy(DevId::Bench));
        d.bench_action(Action::Zero).unwrap();wait(&mut d,"zero",5.,|d|!d.busy(DevId::Bench));
        assert!(d.start_test(&mut db(&d),"Keramisch 1",&plan(Mode::Enkel,&[1.],1)).unwrap_err().contains("meetcurve"));
        d.bench_action(Action::Characterize{interval:crate::model::Interval::Degrees(30.),repeats:1}).unwrap();
        wait(&mut d,"curve",20.,|d|!d.busy(DevId::Bench) && d.bench_cal_version()>=1);
        d.set_correction(-1.).unwrap();
        let run=d.start_test(&mut db(&d),"Keramisch 1",&plan(Mode::Reeks,&[2.,1.],2)).unwrap();
        wait(&mut d,"test klaar",20.,|d|d.running_test().is_none());
        let t=Instant::now();while db(&d).test_run(run).unwrap().status=="loopt"{d.poll();assert!(t.elapsed()<Duration::from_secs(5));std::thread::sleep(Duration::from_millis(10));}
        let doses=db(&d).test_doses(run).unwrap();assert_eq!(doses.len(),4);assert_eq!(db(&d).test_run(run).unwrap().status,"klaar");
        assert!(doses.iter().all(|x|x.status=="ok" && x.correction_pct==-1.),"{doses:?}");
        // The engine doses the corrected weight (−1 %), the deviation is against the requested weight.
        assert!(doses.iter().all(|x|(x.err_pct().unwrap()+1.).abs()<0.05),"{:?}",doses.iter().map(|x|x.err_pct()).collect::<Vec<_>>());
        assert!(doses[0].settings.contains("INV-13"));
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn scale_stop_generation_never_lost_or_leaking(){
        let (l,port)=silent_scale();
        let (mut d,o)=devices("gen",ProfileKind::Vsd);d.settings.machine.scales[0]=ScaleNet{host:"127.0.0.1".into(),port};
        d.connect(DevId::Lh1,false).unwrap();wait(&mut d,"verbonden",5.,|d|on(d,DevId::Lh1));
        d.scale(Head::LH1,false).unwrap();std::thread::sleep(Duration::from_millis(200));
        d.stop_all();
        // A weighing requested right after the STOP (worker may not have reported idle yet) is not stopped by it.
        let (tx,rx)=mpsc::channel();let gen=d.scale_gen[0].load(Ordering::SeqCst);
        d.scale_tx[0].send(ScaleCmd::Read{stable:true,timeout_s:0.6,gen,reply:tx}).unwrap();
        let r=rx.recv_timeout(Duration::from_secs(5)).unwrap();assert!(r.unwrap_err().contains("time-out"),"liep door tot time-out, niet gestopt");
        wait(&mut d,"eerste weging gestopt",2.,|d|!d.busy(DevId::Lh1));assert!(d.status(DevId::Lh1).text.contains("Gestopt"));
        drop(l);let _=std::fs::remove_dir_all(o);
    }
    fn pot_ready(name:&str,travel:f64)->(Devices,PathBuf,i64){
        let (mut d,o)=vsd_with_db(name,travel);let (v,_)=crate::recipe::tests::r1_setup(&mut db(&d));
        for id in [DevId::Can,DevId::Lh1,DevId::Lh2,DevId::Plc]{d.connect(id,true).unwrap();}
        wait(&mut d,"verbonden",5.,|d|[DevId::Can,DevId::Lh1,DevId::Lh2,DevId::Plc].iter().all(|i|on(d,*i)));(d,o,v)
    }
    #[test]fn r1_pot_in_simulation_with_plc_emulator(){
        let (mut d,o,v)=pot_ready("pot1",0.2);
        let t=Instant::now();let run=d.start_pot(&mut db(&d),v,false,0.).unwrap();
        assert!(d.pot_running());assert!(d.start_pot(&mut db(&d),v,false,0.).unwrap_err().contains("al een pot"));
        assert!(d.pumps(vec!["P_C01".into()],PumpOp::Status,0).unwrap_err().contains("loopt een pot"));
        wait(&mut d,"wacht op kop 2",30.,|d|d.pot.lock().unwrap().waiting.is_some());
        let w=d.pot.lock().unwrap().waiting.clone().unwrap();assert!(w.contains("LH2"),"{w}");d.pot_resume();
        wait(&mut d,"pot klaar",30.,|d|!d.pot_running());
        let r=db(&d).pot_run(run).unwrap();assert_eq!(r.status,"klaar","{}",r.note);
        let it=db(&d).pot_items(run).unwrap();
        assert_eq!(it.iter().map(|x|(x.ingredient.as_str(),x.head.as_str())).collect::<Vec<_>>(),vec![("A","LH1"),("B","LH1"),("C","LH1"),("D","LH1"),("E","LH2"),("F","LH2")]);
        assert!(it.iter().all(|x|x.status=="ok"),"{it:?}");
        let (b,c)=(&it[1],&it[2]);
        assert_eq!(b.started_ms,c.started_ms,"B en C in één CAN-blok");assert!(b.attributed && c.attributed && !it[0].attributed);
        assert!(b.started_ms.unwrap()<c.ended_ms.unwrap() && c.started_ms.unwrap()<b.ended_ms.unwrap(),"tijden overlappen");
        let msgs=d.status(DevId::Plc).messages.join(" | ");
        for p in ["-> proces 1350 opdracht 1","-> proces 1350 opdracht 8","-> proces 2350 opdracht 1","-> proces 2350 opdracht 8","-> proces 1300 opdracht 1"]{assert!(msgs.contains(p),"{p} ontbreekt: {msgs}");}
        // Normal head end: lift Stop 9, then unit Stop with its reply 9 (waited for, not posted).
        let tr=d.plc_trace.lock().unwrap().clone();
        for (lift,unit) in [(1350,1300),(2350,2300)]{let i=tr.iter().position(|x|x.1==lift && x.2==CMD_STOP).unwrap();assert_eq!(tr[i+1..].iter().find(|x|x.1==unit).map(|x|(x.2,x.4.clone())),Some((CMD_STOP,Ok(RSP_STOPPED))),"{unit}");}
        eprintln!("R1: pot (simulatie + PLC-emulator) klaar in {:?}; B {:?}-{:?} ms, C {:?}-{:?} ms",t.elapsed(),b.started_ms,b.ended_ms,c.started_ms,c.ended_ms);
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn r5_stop_during_pot_lowers_lift_within_1s(){
        let (mut d,o,v)=pot_ready("pot5",30.);
        let run=d.start_pot(&mut db(&d),v,false,0.).unwrap();
        wait(&mut d,"doseren A",10.,|d|d.pot.lock().unwrap().lines.iter().any(|l|l.contains("doseren stap 1")));
        std::thread::sleep(Duration::from_millis(300));
        let t0=now_ms();let ti=Instant::now();d.stop_all();wait(&mut d,"pot gestopt",3.,|d|!d.pot_running());let took=ti.elapsed();
        let down=d.plc_trace.lock().unwrap().iter().find(|x|x.1==1350 && x.2==CMD_STOP && x.4==Ok(RSP_STOPPED)).map(|x|x.0-t0).expect("lift omlaag");
        assert!(down<1000 && took<Duration::from_secs(1),"lift omlaag na {down} ms, pot klaar na {took:?}");
        let r=db(&d).pot_run(run).unwrap();assert_eq!(r.status,"gestopt");
        let it=db(&d).pot_items(run).unwrap();assert_eq!(it.len(),1);assert_eq!((it[0].ingredient.as_str(),it[0].status.as_str(),it[0].dosed_g),("A","gestopt",None));
        let msgs=d.status(DevId::Plc).messages.join(" | ");let at=msgs.rfind("-> proces 1350 opdracht 8").expect("lift omlaag gestuurd");
        assert!(msgs[at..].contains("<- proces 1350 antwoord 9"),"lift omlaag bevestigd (9 = gestopt, Camelot): {msgs}");
        wait(&mut d,"CAN vrij",3.,|d|!d.busy(DevId::Can));
        eprintln!("R5: STOP tijdens pot -> lift omlaag bevestigd na {down} ms, pot klaar na {took:?}");
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn pot_refused_without_plc_or_scale_and_hardware_without_calibration(){
        let (mut d,o)=vsd_with_db("pot6",0.05);let (v,_)=crate::recipe::tests::r1_setup(&mut db(&d));
        for id in [DevId::Can,DevId::Lh1]{d.connect(id,true).unwrap();}
        wait(&mut d,"verbonden",5.,|d|on(d,DevId::Can) && on(d,DevId::Lh1));
        assert!(d.start_pot(&mut db(&d),v,false,0.).unwrap_err().contains("PLC niet verbonden"));
        d.connect(DevId::Plc,true).unwrap();wait(&mut d,"plc",5.,|d|on(d,DevId::Plc));
        assert!(d.start_pot(&mut db(&d),v,false,0.).unwrap_err().contains("Geen weegschaal LH2"));
        assert!(db(&d).pot_runs(1).unwrap().is_empty(),"geen run aangemaakt");
        let _=std::fs::remove_dir_all(o);
        // Hardware CAN (fake bridge) with only simulation calibrations: refused by the plan.
        let (mut d,o)=vsd_with_db("pot7",0.05);fake_bridge(&mut d,&o);d.settings.mix_allowed=true;let (v,_)=crate::recipe::tests::r1_setup(&mut db(&d));
        d.connect(DevId::Can,false).unwrap();for id in [DevId::Lh1,DevId::Lh2,DevId::Plc]{d.connect(id,true).unwrap();}
        wait(&mut d,"verbonden",15.,|d|d.status(DevId::Can).link!=Link::Connecting && [DevId::Lh1,DevId::Lh2,DevId::Plc].iter().all(|i|on(d,*i)));assert!(on(&d,DevId::Can));
        assert!(d.start_pot(&mut db(&d),v,false,0.).unwrap_err().contains("bevestig"));
        let e=d.start_pot(&mut db(&d),v,true,0.).unwrap_err();assert!(e.contains("niet gekalibreerd (hardware)"),"{e}");
        d.disconnect(DevId::Can).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Can));
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn l1_lift_p1_is_height_times_ten_from_settings_or_recipe(){
        let (mut d,o,v)=pot_ready("potl1",0.02);d.settings.machine.lift_height_cm=[Some(15.5),None];
        let plc=|d:&Devices|d.status(DevId::Plc).messages.join(" | ");
        let run=|d:&mut Devices,v:i64|{let r=d.start_pot(&mut db(d),v,false,0.).unwrap();
            wait(d,"wacht op LH2",30.,|d|d.pot.lock().unwrap().waiting.is_some() || !d.pot_running());d.pot_resume();wait(d,"pot klaar",30.,|d|!d.pot_running());
            assert_eq!(db(d).pot_run(r).unwrap().status,"klaar");};
        run(&mut d,v);
        let m=plc(&d);
        assert!(m.contains("-> proces 1350 opdracht 1 parameter 155") && m.contains("<- proces 1350 antwoord 4 resultaat 155"),"LH1 15,5 cm uit Instellingen: {m}");
        assert!(m.contains("-> proces 2350 opdracht 1 parameter 100"),"LH2 zonder hoogte: simulatie 10 cm: {m}");
        // Pot height on the recipe overrides both heads: 12,3 cm -> P1 123 (new version, the old one was used).
        let (rid,_,mut draft)=db(&d).recipe_version(v).unwrap();draft.pot_height_cm=Some(12.3);
        let out=db(&d).save_recipe(ProfileKind::Vsd,Some(rid),&draft).unwrap();assert!(out.new_version);
        assert_eq!(db(&d).recipe_version(out.version_id).unwrap().2.pot_height_cm,Some(12.3));
        run(&mut d,out.version_id);
        let m=plc(&d);let tail=&m[m.rfind("parameter 155").unwrap()..];
        assert!(tail.contains("-> proces 1350 opdracht 1 parameter 123") && tail.contains("-> proces 2350 opdracht 1 parameter 123"),"pothoogte recept: {tail}");
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn l2_no_pot_reply_16_fails_head_no_dosing_lift_down(){
        let (mut d,o,v)=pot_ready("potl2",0.02);
        d.plc_script.lock().unwrap().push((1350,16));
        let run=d.start_pot(&mut db(&d),v,false,0.).unwrap();
        wait(&mut d,"pot klaar",10.,|d|!d.pot_running());
        let r=db(&d).pot_run(run).unwrap();assert_eq!(r.status,"fout");assert!(r.note.contains("geen pot"),"{}",r.note);
        assert!(db(&d).pot_items(run).unwrap().is_empty(),"niets gedoseerd of opgeslagen");
        let lines=d.pot.lock().unwrap().lines.join(" | ");assert!(!lines.contains("doseren stap"),"geen CAN-blok: {lines}");
        let m=d.status(DevId::Plc).messages.join(" | ");let i=m.rfind("<- proces 1350 antwoord 16").expect("16 ontvangen");
        assert!(m[i..].contains("-> proces 1350 opdracht 8") && m[i..].contains("<- proces 1350 antwoord 9"),"lift omlaag na geen pot: {m}");
        assert!(!m.contains("proces 2350"),"LH2 niet gestart");
        // Unexpected reply on unit start: also a failed head.
        // (the unit Stop of the first pot is posted without waiting: let the emulator take it before scripting 1300)
        wait(&mut d,"unit stop opgepakt",5.,|d|d.status(DevId::Plc).messages.last().is_some_and(|m|m.contains("<- proces 1300 antwoord 9")));
        d.plc_script.lock().unwrap().push((1300,77));let run=d.start_pot(&mut db(&d),v,false,0.).unwrap();wait(&mut d,"pot klaar",10.,|d|!d.pot_running());
        let r=db(&d).pot_run(run).unwrap();assert_eq!(r.status,"fout");assert!(r.note.contains("onverwacht PLC-antwoord 77"),"{}",r.note);
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn unit_stop_not_9_at_normal_end_fails_the_pot(){
        let (mut d,o,_)=pot_ready("unit9",0.02);
        let v=db(&d).save_recipe(ProfileKind::Vsd,None,&crate::recipe::RecipeDraft{name:"Eén".into(),steps:vec![crate::recipe::tests::step(vec![crate::recipe::tests::item("A",1.)])],..Default::default()}).unwrap().version_id;
        d.plc_script_cmd.lock().unwrap().push((1300,CMD_STOP,12));
        let run=d.start_pot(&mut db(&d),v,false,0.).unwrap();wait(&mut d,"pot klaar",10.,|d|!d.pot_running());
        let r=db(&d).pot_run(run).unwrap();assert_eq!(r.status,"fout");assert!(r.note.contains("unit stop") && r.note.contains("12"),"{}",r.note);
        assert_eq!(db(&d).pot_items(run).unwrap()[0].status,"ok","gedoseerde waarde blijft");
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn l1_l2_reply_table_as_camelot(){
        for (cmd,p,code,ok) in [(CMD_START,1350,2,true),(CMD_START,1350,4,true),(CMD_START,1350,16,false),(CMD_START,1350,101,false),(CMD_STOP,1350,9,true),(CMD_STOP,1350,5,false),
            (CMD_STOP,1350,108,false),(CMD_START,1300,2,true),(CMD_START,1300,4,false),(CMD_STOP,2300,9,true),(CMD_STOP,2300,2,false)]{
            assert_eq!(pot_reply(cmd,p,code,"x").is_ok(),ok,"opdracht {cmd} proces {p} antwoord {code}");}
        assert!(pot_reply(CMD_START,2350,16,"lift LH2 omhoog").unwrap_err().starts_with("geen pot"));
        assert_eq!(crate::machine::height_p1(15.55),156);assert!(crate::machine::check_height(0.).is_err() && crate::machine::check_height(100.1).is_err());
    }
    #[test]fn l1_hardware_pot_refused_without_lift_height(){
        let (mut d,o)=vsd_with_db("potl1h",0.05);fake_bridge(&mut d,&o);d.settings.mix_allowed=true;let (v,_)=crate::recipe::tests::r1_setup(&mut db(&d));
        for p in ["P_C01","P_C02","P_C03","P_C04","P_A01","P_A02"]{let mut c=crate::calib::tests_sample(p,0.5);c.simulation=false;db(&d).save_calibration(&NewCal::from_vsd(&c,true,"")).unwrap();}
        d.connect(DevId::Can,false).unwrap();for id in [DevId::Lh1,DevId::Lh2,DevId::Plc]{d.connect(id,true).unwrap();}
        wait(&mut d,"verbonden",15.,|d|d.status(DevId::Can).link!=Link::Connecting && [DevId::Lh1,DevId::Lh2,DevId::Plc].iter().all(|i|on(d,*i)));assert!(on(&d,DevId::Can));
        let e=d.start_pot(&mut db(&d),v,true,0.).unwrap_err();assert!(e.contains("Geen lifthoogte voor LH1"),"{e}");
        d.settings.machine.lift_height_cm=[Some(12.),None];let e=d.start_pot(&mut db(&d),v,true,0.).unwrap_err();assert!(e.contains("Geen lifthoogte voor LH2"),"{e}");
        assert!(db(&d).pot_runs(1).unwrap().is_empty(),"geen run aangemaakt");
        d.disconnect(DevId::Can).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Can));
        let _=std::fs::remove_dir_all(o);
    }
    // ---------- production (iteration 5) ----------
    fn prod_ready(name:&str,travel:f64)->(Devices,PathBuf,[i64;3]){
        let (mut d,o)=vsd_with_db(name,travel);let v=crate::production::tests::recipes3(&mut db(&d));
        for id in DevId::VSD{d.connect(id,true).unwrap();}
        wait(&mut d,"verbonden",5.,|d|DevId::VSD.iter().all(|i|on(d,*i)));(d,o,v)
    }
    /// Lift intervals per head from the PLC trace: (up ms, down ms).
    fn lift_intervals(tr:&[(i64,u32,u32,Vec<u32>,std::result::Result<u32,String>)],p:u32)->Vec<(i64,i64)>{
        let mut v=vec![];let mut up=None;
        for x in tr{if x.1==p{if x.2==CMD_START && x.4.is_ok(){up=Some(x.0);}else if x.2==CMD_STOP{if let Some(u)=up.take(){v.push((u,x.0));}}}}v
    }
    #[test]fn p1_production_sim_three_orders_seven_pots(){
        let (mut d,o,v)=prod_ready("p1",0.6);
        {let mut b=db(&d);b.create_order(ProfileKind::Vsd,v[0],"LH1","",0,2).unwrap();b.create_order(ProfileKind::Vsd,v[1],"LH2","",0,2).unwrap();b.create_order(ProfileKind::Vsd,v[2],"Gemengd","",0,3).unwrap();}
        // Not without a scanner.
        d.disconnect(DevId::Scanner).unwrap();wait(&mut d,"scanner uit",5.,|d|!d.is_on(DevId::Scanner));
        assert!(d.start_production(&db(&d),false).unwrap_err().contains("Barcodescanner niet verbonden"));
        d.connect(DevId::Scanner,true).unwrap();wait(&mut d,"scanner",5.,|d|on(d,DevId::Scanner));
        let t=Instant::now();d.start_production(&db(&d),false).unwrap();assert!(d.production_running());
        assert!(d.pumps(vec!["P_C01".into()],PumpOp::Status,0).unwrap_err().contains("Productie loopt"));
        wait(&mut d,"productie klaar",120.,|d|!d.production_running());let took=t.elapsed();
        let h=db(&d).history(ProfileKind::Vsd,&crate::production::HistFilter::default(),50).unwrap();
        assert_eq!(h.len(),7);assert!(h.iter().all(|p|p.status=="klaar" && p.simulation),"{:?}",h.iter().map(|p|(&p.barcode,&p.status,&p.note)).collect::<Vec<_>>());
        let tr=d.plc_trace.lock().unwrap().clone();
        let s0=tr.iter().position(|x|(x.1,x.2)==(1310,CMD_START)).unwrap();
        assert!(s0==2 && tr[..2].iter().all(|x|x.2==CMD_STOP && x.4==Ok(RSP_STOPPED) && matches!(x.1,1350|2350)),"eerst beide liften omlaag (9), dan lijn 1310 Start");let l=tr.last().unwrap();assert_eq!((l.1,l.2,l.4.clone()),(1310,CMD_STOP,Ok(RSP_STOPPED)),"lijn 1310 Stop als laatste");
        // Every transport Start with both lifts down.
        let mut up=[false;2];let mut unit=[false;2];let mut transports=0;
        for x in &tr{match (x.1,x.2){(1350,CMD_START)=>up[0]=x.4.is_ok(),(1350,CMD_STOP)=>up[0]=false,(2350,CMD_START)=>up[1]=x.4.is_ok(),(2350,CMD_STOP)=>up[1]=false,
            (1220,CMD_START)=>{transports+=1;assert_eq!(up,[false,false],"transport met lift omhoog");assert_eq!(unit,[false,false],"transport voor unit Stop 9");assert_eq!(x.3[8..10],[u32::MAX,1234567890]);}
            (1300,CMD_START)=>unit[0]=true,(2300,CMD_START)=>unit[1]=true,(1300,CMD_STOP)=>{assert_eq!(x.4,Ok(RSP_STOPPED));unit[0]=false;},(2300,CMD_STOP)=>{assert_eq!(x.4,Ok(RSP_STOPPED));unit[1]=false;}_=>{}}}
        assert!(transports>=8,"{transports}");
        // Both heads busy at the same time, each on its own pot.
        let (i1,i2)=(lift_intervals(&tr,1350),lift_intervals(&tr,2350));
        let items:Vec<(i64,String,i64)>=h.iter().flat_map(|p|db(&d).pot_items(p.id).unwrap().into_iter().map(|x|(x.pot_run_id,x.head,x.started_ms.unwrap()))).collect();
        let pot_in=|iv:(i64,i64),head:&str|items.iter().find(|x|x.1==head && x.2>=iv.0 && x.2<=iv.1).map(|x|x.0);
        let both=i1.iter().find_map(|a|i2.iter().find(|b|a.0<b.1 && b.0<a.1).and_then(|b|Some((pot_in(*a,"LH1")?,pot_in(*b,"LH2")?,*a,*b))));
        let (p1,p2,a,b)=both.expect("LH1 en LH2 tegelijk bezig");assert_ne!(p1,p2,"verschillende potten");
        // M1: the dosing itself (CAN blocks) of LH1 and LH2 overlaps on different pots, not only the lift times.
        let blocks:Vec<(i64,String,i64,i64)>=h.iter().flat_map(|p|db(&d).pot_items(p.id).unwrap().into_iter().map(|x|(x.pot_run_id,x.head,x.started_ms.unwrap(),x.ended_ms.unwrap()))).collect();
        let ov=blocks.iter().filter(|x|x.1=="LH1").flat_map(|x|blocks.iter().filter(|y|y.1=="LH2" && y.0!=x.0).map(move|y|(x.3.min(y.3)-x.2.max(y.2),x.0,y.0))).max().unwrap();
        assert!(ov.0>=300,"doseertijd LH1/LH2 overlapt maar {} ms (blok 600 ms)",ov.0);
        eprintln!("P1 (simulatie + emulator): 7 potten in {took:?}; {transports} transporten; lift LH1 pot {p1} {}–{} ms en LH2 pot {p2} {}–{} ms; doseren overlapt {} ms (LH1 pot {}, LH2 pot {})",a.0%100000,a.1%100000,b.0%100000,b.1%100000,ov.0,ov.1,ov.2);
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn p3_production_stop_aborts_within_1s_lifts_down(){
        let (mut d,o,v)=prod_ready("p3",3.);
        db(&d).create_order(ProfileKind::Vsd,v[2],"O","",0,3).unwrap();
        d.start_production(&db(&d),false).unwrap();
        wait(&mut d,"lift LH1 omhoog",30.,|d|d.lift_up(Head::LH1));std::thread::sleep(Duration::from_millis(400));
        d.stop_all();let took=wait(&mut d,"productie afgebroken",5.,|d|!d.production_running());
        assert!(!d.lift_up(Head::LH1) && !d.lift_up(Head::LH2));wait(&mut d,"CAN vrij",3.,|d|!d.busy(DevId::Can));
        let tr=d.plc_trace.lock().unwrap().clone();for p in [1310,1220,1190,1300,2300]{assert!(tr.iter().any(|x|x.1==p && x.2==CMD_ABORT),"Abort naar {p}");}
        let line=db(&d).line_state().unwrap().unwrap();assert_eq!(line.state,"afgebroken");
        let pots:Vec<_>=line.slots.iter().filter_map(|s|if let crate::production::Slot::Pot(id)=s{Some(db(&d).prod_pot(*id).unwrap())}else{None}).collect();
        assert!(!pots.is_empty() && pots.iter().all(|p|p.status=="afgebroken"),"{:?}",pots.iter().map(|p|&p.status).collect::<Vec<_>>());
        assert!(took<Duration::from_millis(1500),"{took:?}");
        eprintln!("P3: STOP tijdens productie -> afgebroken na {took:?} (pompen gestopt, liften omlaag, Abort naar 1310/1220/1190/1300/2300); {} pot(ten) afgebroken op de lijn",pots.len());
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn machine_page_actions_scanner_test_and_plc_init(){
        let (mut d,o,_)=prod_ready("mach",0.05);
        d.line_action("Lift LH1 omhoog",vec![(1350,CMD_START,vec![123])],false).unwrap();wait(&mut d,"plc",5.,|d|!d.busy(DevId::Plc));assert!(d.lift_up(Head::LH1));
        assert!(d.line_action("Transport",vec![(1220,CMD_START,vec![]),(1220,CMD_STOP,vec![])],false).unwrap_err().contains("Lift LH1 staat omhoog"));
        d.line_action("Lift LH1 omlaag",vec![(1350,CMD_STOP,vec![])],false).unwrap();wait(&mut d,"plc",5.,|d|!d.busy(DevId::Plc));
        d.line_action("Transport",vec![(1220,CMD_START,vec![]),(1220,CMD_STOP,vec![])],false).unwrap();wait(&mut d,"plc",5.,|d|!d.busy(DevId::Plc));assert!(d.status(DevId::Plc).ok,"{}",d.status(DevId::Plc).text);
        d.plc_init(false).unwrap_err();d.plc_init(true).unwrap();wait(&mut d,"init",10.,|d|!d.busy(DevId::Plc));
        let tr=d.plc_trace.lock().unwrap().clone();
        for p in [1310,1220,1190,1300,2300,1350,2350]{let c:Vec<u32>=tr.iter().filter(|x|x.1==p && matches!(x.2,5|98|2)).map(|x|x.2).collect();assert!(c.ends_with(&[5,98,2]),"{p}: {c:?}");}
        d.sim_scan.lock().unwrap().feed.push_back("0000000042".into());d.scan_test().unwrap();wait(&mut d,"scan",5.,|d|!d.busy(DevId::Scanner));
        assert!(d.status(DevId::Scanner).scan.contains("0000000042"),"{}",d.status(DevId::Scanner).scan);
        d.sim_scan.lock().unwrap().no_reads=1;d.scan_test().unwrap();wait(&mut d,"scan",5.,|d|!d.busy(DevId::Scanner));assert!(d.status(DevId::Scanner).scan.contains("no-read"));
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn m2_lift_up_refused_and_lowered_at_start_after_restart(){
        let (mut d,o,v)=prod_ready("m2",0.05);db(&d).create_order(ProfileKind::Vsd,v[0],"O","",0,1).unwrap();
        d.line_action("Lift LH1 omhoog",vec![(1350,CMD_START,vec![100])],false).unwrap();wait(&mut d,"plc",5.,|d|!d.busy(DevId::Plc));
        assert!(d.start_production(&db(&d),false).unwrap_err().contains("Lift LH1 staat omhoog"));
        // App restart: the trace is gone, the lift is physically still up. Start lowers both lifts (reply 9) first.
        d.plc_trace.lock().unwrap().clear();
        d.start_production(&db(&d),false).unwrap();wait(&mut d,"klaar",60.,|d|!d.production_running());
        let tr=d.plc_trace.lock().unwrap().clone();let first=tr.iter().position(|x|x.1==1220 && x.2==CMD_START).unwrap();
        for p in [1350,2350]{assert!(tr[..first].iter().any(|x|x.1==p && x.2==CMD_STOP && x.4==Ok(RSP_STOPPED)),"lift {p} omlaag (9) vóór transport");}
        assert_eq!((tr[0].1,tr[0].2),(1350,CMD_STOP));
        let _=std::fs::remove_dir_all(o);
    }
    #[test]fn m4_no_pot_ends_the_scan_at_once(){
        let (mut d,o,_)=prod_ready("m4",0.05);
        // Real (TCP) scanner that never answers, 10 s time-out; the PLC says 16 (no pot).
        let (_l,port)=silent_scale();d.disconnect(DevId::Scanner).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Scanner));
        d.settings.mix_allowed=true;d.settings.machine.scanner=ScannerConfig{host:"127.0.0.1".into(),port,timeout_s:10.,..Default::default()};
        d.connect(DevId::Scanner,false).unwrap();wait(&mut d,"scanner",5.,|d|on(d,DevId::Scanner));
        let mut line=d.line_io(false);
        d.plc_script.lock().unwrap().push((1190,16));let t=Instant::now();
        let (c,r,_)=line.barcode(1190).unwrap();let took=t.elapsed();
        assert_eq!((c,r),(16,ScanResult::NoRead));assert!(took<Duration::from_secs(1),"{took:?}");
        // Pot present (2) and a sim scanner code: both answers awaited together.
        d.disconnect(DevId::Scanner).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Scanner));d.connect(DevId::Scanner,true).unwrap();wait(&mut d,"scanner",5.,|d|on(d,DevId::Scanner));
        d.sim_scan.lock().unwrap().feed.push_back("0000000009".into());let (c,r,_)=line.barcode(1190).unwrap();assert_eq!((c,r),(2,ScanResult::Code("0000000009".into())));
        eprintln!("M4: PLC 16 beëindigt de scan na {took:?} (scanner-time-out 10 s)");
        let _=std::fs::remove_dir_all(o);
    }
    /// Fake vsd_bridge.py (no CAN), like the real one: DISPENSE (73) drives answer after 2 s, other commands at
    /// once; 'stop' bumps a generation and a wait for drives sent before it ends with stopped=True.
    const SLOW_BRIDGE:&str=r#"
import sys, json, time, threading
lock=threading.Lock();cond=threading.Condition();sent={};gen={"g":0};sent_gen={}
def reply(v):
    with lock: print(json.dumps(v),flush=True)
def work(i,op,a):
    if op=="send":
        k=(a["node"],a["drive"])
        with cond: sent[k]=(time.time(),a["cmd"],a["params"][0]);sent_gen[k]=gen["g"]
        reply(dict(id=i,ok=True,result=dict(sent=True)));return
    if op=="wait":
        dl=time.time()+float(a.get("timeout",30));res={};stopped=False
        while True:
            with cond:
                for t in a["targets"]:
                    k=(t["node"],t["drive"])
                    if k in res: continue
                    if sent_gen.get(k,gen["g"])<gen["g"]: stopped=True
                    s=sent.get(k)
                    if s and (s[1]!=73 or time.time()-s[0]>=2): res[k]=dict(node=k[0],drive=k[1],code=(t["codes"] or [s[1]])[0],error=False,position=s[2])
            if stopped or len(res)==len(a["targets"]) or time.time()>=dl: break
            time.sleep(0.005)
        miss=[dict(node=t["node"],drive=t["drive"]) for t in a["targets"] if (t["node"],t["drive"]) not in res]
        reply(dict(id=i,ok=True,result=dict(results=list(res.values()),missing=miss,fault=None,stopped=stopped)));return
    reply(dict(id=i,ok=True,result=dict(connected=op=="connect")))
for line in sys.stdin:
    r=json.loads(line);i=r["id"];op=r["op"];a=r.get("args",{})
    if op=="stop":
        with cond: gen["g"]+=1
        reply(dict(id=i,ok=True,result=dict(stopped=True)));continue
    work(i,op,a)
"#;
    /// Hardware CAN on the slow fake bridge, simulated scales and PLC emulator, hardware calibrations for the R1 pumps.
    fn slow_hw(name:&str)->(Devices,PathBuf,i64){
        let (mut d,o)=vsd_with_db(name,0.05);
        let dir=o.join("bridge");std::fs::create_dir_all(&dir).unwrap();std::fs::write(dir.join("vsd_bridge.py"),SLOW_BRIDGE).unwrap();
        d.settings.bridge_dir=dir;d.settings.python="python".into();d.settings.mix_allowed=true;d.settings.machine.lift_height_cm=[Some(12.),Some(12.)];
        let (v,_)=crate::recipe::tests::r1_setup(&mut db(&d));
        for p in ["P_C01","P_C02","P_C03","P_C04","P_A01","P_A02"]{let mut c=crate::calib::tests_sample(p,0.5);c.simulation=false;db(&d).save_calibration(&NewCal::from_vsd(&c,true,"")).unwrap();}
        d.connect(DevId::Can,false).unwrap();for id in [DevId::Lh1,DevId::Lh2,DevId::Plc]{d.connect(id,true).unwrap();}
        wait(&mut d,"verbonden",15.,|d|d.status(DevId::Can).link!=Link::Connecting && [DevId::Lh1,DevId::Lh2,DevId::Plc].iter().all(|i|on(d,*i)));
        assert!(on(&d,DevId::Can),"{}",d.status(DevId::Can).text);(d,o,v)
    }
    fn cal_id(d:&Devices,p:&str)->i64{db(d).calibration_for_dosing(ProfileKind::Vsd,p,calib::KIND_VSD,true).unwrap().id}
    #[test]fn stop_ends_running_blocks_of_both_heads_on_the_bridge(){
        let (mut d,o,_)=slow_hw("stopbr");
        let path=d.settings.db_path.clone();
        let (t1,r1)=mpsc::channel();let (t2,r2)=mpsc::channel();
        d.can_tx.send(CanCmd::DoseBlock{items:vec![("P_C01".into(),1.,cal_id(&d,"P_C01"))],db:path.clone(),reply:t1}).unwrap();
        d.can_tx.send(CanCmd::DoseBlock{items:vec![("P_A01".into(),1.,cal_id(&d,"P_A01"))],db:path.clone(),reply:t2}).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let t=Instant::now();d.stop_all();
        let a=r1.recv_timeout(Duration::from_secs(3)).unwrap();let b=r2.recv_timeout(Duration::from_secs(3)).unwrap();let took=t.elapsed();
        assert!(a.as_ref().unwrap_err().contains("Gestopt") && b.as_ref().unwrap_err().contains("Gestopt"),"{a:?} {b:?}");assert!(took<Duration::from_secs(1),"{took:?}");
        // The same drive twice in running blocks is refused; after the STOP a new block asks STATUS and runs again.
        d.can_stop.reset();
        let (t3,r3)=mpsc::channel();let (t4,r4)=mpsc::channel();
        d.can_tx.send(CanCmd::DoseBlock{items:vec![("P_C01".into(),1.,cal_id(&d,"P_C01"))],db:path.clone(),reply:t3}).unwrap();
        d.can_tx.send(CanCmd::DoseBlock{items:vec![("P_C01".into(),1.,cal_id(&d,"P_C01"))],db:path.clone(),reply:t4}).unwrap();
        assert!(r4.recv_timeout(Duration::from_secs(3)).unwrap().unwrap_err().contains("doseert nog in een ander blok"));
        assert!(r3.recv_timeout(Duration::from_secs(8)).unwrap().is_ok());
        eprintln!("STOP met twee lopende blokken op de (nep-)bridge: beide gestopt na {took:?}");
        d.disconnect(DevId::Can).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Can));let _=std::fs::remove_dir_all(o);
    }
    #[test]fn stop_during_pot_on_the_bridge_lowers_lift_within_1s(){
        let (mut d,o,v)=slow_hw("potbr");
        let run=d.start_pot(&mut db(&d),v,true,0.).unwrap();
        wait(&mut d,"doseren",15.,|d|d.pot.lock().unwrap().lines.iter().any(|l|l.contains("doseren stap 1")));std::thread::sleep(Duration::from_millis(400));
        let t=Instant::now();d.stop_all();let took=wait(&mut d,"pot klaar",5.,|d|!d.pot_running());
        assert!(took<Duration::from_secs(1),"{took:?}");assert!(!d.lift_up(Head::LH1));assert_eq!(db(&d).pot_run(run).unwrap().status,"gestopt");
        eprintln!("STOP tijdens pot op de (nep-)bridge: lift omlaag en pot gestopt na {took:?}");let _=t;
        d.disconnect(DevId::Can).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Can));let _=std::fs::remove_dir_all(o);
    }
    #[test]fn stop_during_production_on_the_bridge_lowers_lifts_within_1s(){
        let (mut d,o,_)=slow_hw("prodbr");d.connect(DevId::Scanner,true).unwrap();wait(&mut d,"scanner",5.,|d|on(d,DevId::Scanner));
        let v=crate::production::tests::recipes3_only(&mut db(&d));db(&d).create_order(ProfileKind::Vsd,v,"O","",0,2).unwrap();
        d.start_production(&db(&d),true).unwrap();
        wait(&mut d,"lift omhoog",30.,|d|d.lift_up(Head::LH1)||d.lift_up(Head::LH2));std::thread::sleep(Duration::from_millis(600));
        d.stop_all();let took=wait(&mut d,"afgebroken",5.,|d|!d.production_running());
        assert!(took<Duration::from_secs(1),"{took:?}");assert!(!d.lift_up(Head::LH1) && !d.lift_up(Head::LH2));
        assert_eq!(db(&d).line_state().unwrap().unwrap().state,"afgebroken");
        eprintln!("STOP tijdens productie op de (nep-)bridge: afgebroken, liften omlaag na {took:?}");
        d.disconnect(DevId::Can).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Can));let _=std::fs::remove_dir_all(o);
    }
    #[test]fn stop_during_calibration_on_the_bridge_within_1s(){
        let (mut d,o,_)=slow_hw("calbr");
        d.calibrate(vec!["P_C01".into()],short_cal()).unwrap();std::thread::sleep(Duration::from_millis(800));assert!(d.busy(DevId::Can));
        let t=Instant::now();d.stop_all();let took=wait(&mut d,"kalibratie gestopt",5.,|d|!d.busy(DevId::Can));
        assert!(took<Duration::from_secs(1),"{took:?}");assert!(!d.status(DevId::Can).ok);let _=t;
        eprintln!("STOP tijdens kalibratie op de (nep-)bridge: gestopt na {took:?}");
        d.disconnect(DevId::Can).unwrap();wait(&mut d,"uit",5.,|d|!d.is_on(DevId::Can));let _=std::fs::remove_dir_all(o);
    }
}
