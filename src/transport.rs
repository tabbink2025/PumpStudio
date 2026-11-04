use crate::model::{Config,Result};
use serde_json::{json,Value};
use std::{io::{BufRead,BufReader,Write},path::Path,process::{Child,ChildStdin,Command,Stdio},sync::{Arc,Mutex,atomic::{AtomicBool,Ordering},mpsc},time::{Duration,Instant}};

pub trait Device:Send {
    fn call(&mut self,op:&str,args:Value)->Result<Value>;
}
/// Every live bridge (pump bench and VSD machine) gets the out-of-band stop request.
pub type StopWriter=Arc<Mutex<Vec<(u64,Arc<Mutex<ChildStdin>>)>>>;
static BRIDGE_KEY:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(1);
#[derive(Clone,Default)]
pub struct StopHandle {pub cancelled:Arc<AtomicBool>,pub writer:StopWriter}
impl StopHandle {
    pub fn stop(&self) {
        self.cancelled.store(true,Ordering::SeqCst);
        // Separate thread: UI never waits for a pipe write or a driver response.
        let writer=self.writer.clone();
        std::thread::spawn(move||{let all:Vec<_>=writer.lock().unwrap().iter().map(|(_,s)|s.clone()).collect();for stdin in all{let mut w=stdin.lock().unwrap();let _=w.write_all(b"{\"id\":0,\"op\":\"stop\",\"args\":{}}\n");let _=w.flush();}});
    }
    pub fn check(&self)->Result<()> {if self.cancelled.load(Ordering::SeqCst){Err("Test geannuleerd; referentie ongeldig".into())}else{Ok(())}}
    pub fn reset(&self){self.cancelled.store(false,Ordering::SeqCst);}
}
pub struct Bridge {child:Child,stdin:Arc<Mutex<ChildStdin>>,rx:mpsc::Receiver<std::result::Result<Value,String>>,id:u64,stop:StopHandle,timeout:Duration,key:u64}
impl Bridge {
    pub fn spawn(python:&Path,script:&Path,repo:&Path,output:&Path,stop:StopHandle)->Result<Self> {
        let mut cmd=Command::new(python);
        cmd.args(["-u"]).arg(script).arg("--repo").arg(repo).arg("--output").arg(output)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)] {use std::os::windows::process::CommandExt;cmd.creation_flags(0x08000000);}
        let mut child=cmd.spawn().map_err(|e|format!("Hardwarekoppeling starten: {e}"))?;
        let stdin=Arc::new(Mutex::new(child.stdin.take().unwrap()));
        let stdout=child.stdout.take().unwrap();let(tx,rx)=mpsc::channel();
        // Python warnings and tracebacks go to the application log instead of nowhere.
        if let Some(err)=child.stderr.take(){let name=script.file_stem().map(|s|s.to_string_lossy().to_string()).unwrap_or_default();
            std::thread::spawn(move||{for line in BufReader::new(err).lines().map_while(|l|l.ok()){// python-can IXXAT reports harmless controller info frames on stderr; keep the log readable.
                if !line.trim().is_empty() && !line.contains("Unexpected message info type"){crate::applog::warn("BRIDGE",&format!("{name}: {line}"));}}});}
        crate::applog::info("BRIDGE",&format!("Gestart: {} {}",python.display(),script.display()));
        std::thread::spawn(move||{for line in BufReader::new(stdout).lines(){let response=line.map_err(|e|e.to_string()).and_then(|s|serde_json::from_str(&s).map_err(|e|format!("Ongeldig antwoord hardwarekoppeling: {e}")));if tx.send(response).is_err(){break;}}});
        let key=BRIDGE_KEY.fetch_add(1,Ordering::SeqCst);stop.writer.lock().unwrap().push((key,stdin.clone()));
        Ok(Self{child,stdin,rx,id:0,stop,timeout:Duration::from_secs(120),key})
    }
}
impl Device for Bridge {
    fn call(&mut self,op:&str,args:Value)->Result<Value>{
        self.id+=1;let id=self.id;
        if op=="connect" {if let Some(s)=args["timeout_s"].as_f64(){self.timeout=Duration::from_secs_f64((s+330.).max(120.));}}
        let request=json!({"id":id,"op":op,"args":args});
        {let mut w=self.stdin.lock().map_err(|_|"Hardwarekoppeling vergrendeld")?;writeln!(w,"{request}").and_then(|_|w.flush()).map_err(|e|e.to_string())?;}
        let deadline=Instant::now()+self.timeout;
        loop{
            // Let bridge return failed-motion position before propagating cancellation.
            let wait=deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(100));
            if wait.is_zero(){return Err("Hardwarekoppeling reageert niet; referentie ongeldig".into());}
            match self.rx.recv_timeout(wait){
                Ok(Ok(value))=>{if value["id"].as_u64()!=Some(id){continue;}
                    // STOP pressed while this call ran: the answer does not count (a short wait slice must still end on STOP).
                    // An error reply keeps its own text (it may carry the position reached).
                    if value["ok"]!=true{return Err(value["error"].as_str().unwrap_or("Hardwarefout").to_owned());}
                    if self.stop.cancelled.load(Ordering::SeqCst) && op!="disconnect" && op!="status" && op!="stop"{return Err("Gestopt (STOP)".into());}
                    return Ok(value["result"].clone());}
                Ok(Err(e))=>return Err(e),Err(mpsc::RecvTimeoutError::Disconnected)=>return Err("Hardwarekoppeling gesloten".into()),
                Err(mpsc::RecvTimeoutError::Timeout)=>{
                    if self.stop.cancelled.load(Ordering::SeqCst) && op!="disconnect" && op!="status" {return Err("Test geannuleerd; referentie ongeldig".into());}
                }
            }
        }
    }
}
impl Drop for Bridge {
    fn drop(&mut self){let key=self.key;self.stop.writer.lock().unwrap().retain(|(k,_)|*k!=key);let _=self.child.kill();let _=self.child.wait();}
}

/// `position` is the controller counter (reset by ZERO); `travel` is physical shaft travel that drives the simulated flow.
pub struct Mock {pub cfg:Config,pub position:i64,pub travel:i64,pub reference:bool,pub offset:f64,pub gain:f64,pub fail_move:bool,pub fail_scale:bool,pub stop:StopHandle}
impl Mock {
    pub fn new(cfg:Config,stop:StopHandle)->Self {Self{cfg,position:0,travel:0,reference:false,offset:0.,gain:1.,fail_move:false,fail_scale:false,stop}}
    fn cumulative(&self)->f64{
        let cycles=self.travel.div_euclid(self.cfg.steps_rev) as f64;
        let phase=self.travel.rem_euclid(self.cfg.steps_rev) as f64/self.cfg.steps_rev as f64;
        // Illustrative fill plateau followed by discharge, not a measured INV-13 curve.
        let discharge=((phase-0.5)*2.).clamp(0.,1.);
        self.gain*1.345*(cycles+discharge)
    }
}
impl Device for Mock {
    fn call(&mut self,op:&str,args:Value)->Result<Value>{
        if op!="disconnect" && op!="status" && op!="configure"{self.stop.check()?;}
        match op {
            "connect"=>{if let Ok(cfg)=serde_json::from_value::<Config>(args){self.cfg=cfg;}Ok(json!({"connected":true,"simulation":true}))},
            "configure"=>{let cfg:Config=serde_json::from_value(args).map_err(|e|e.to_string())?;
                if !cfg.same_pump(&self.cfg){self.reference=false;}
                self.cfg=cfg;Ok(json!({"position":self.position,"reference_valid":self.reference,"scale_connected":self.cfg.use_scale}))},
            "rebase"=>{if !self.reference{return Err("Referentie verloren".into());}self.position=0;Ok(json!({"position":0,"reference_valid":true,"homing_status":"simulated_rebase"}))},
            "zero"|"home"=>{self.position=0;self.reference=true;Ok(json!({"position":0,"reference_valid":true,"homing_status":if op=="home"{"simulated_home"}else{"software_origin"}}))},
            "move"=>{if !self.reference{return Err("Referentie verloren".into());}if self.fail_move{self.reference=false;return Err("Mock CAN verloren".into());}
                let target=args["target"].as_i64().ok_or("Doel ontbreekt")?;
                if target<=self.position || target>self.cfg.controller_limit{return Err("Ongeldig vooruitdoel of buiten controllerbereik".into());}
                self.travel+=target-self.position;self.position=target;std::thread::sleep(Duration::from_millis(8));self.stop.check()?;
                Ok(json!({"position":target,"reference_valid":true,"physical_position_verified":false,"motion_acknowledged":true,"simulation":true}))},
            "weight"=>{if !self.cfg.use_scale{return Err("Geen weegschaal verbonden".into());}if self.fail_scale{return Err("Mock weegschaal verloren".into());}let weight=self.cumulative()-self.offset;Ok(json!({"grams":weight,"scale_status":"stable_simulated","samples":[{"g":weight,"stable":true,"raw":"SIMULATION"}]}))},
            "tare"=>{self.offset=self.cumulative();Ok(json!({"scale_status":"tare_simulated"}))},
            "status"=>Ok(json!({"position":self.position,"reference_valid":self.reference,"homing_status":"simulation"})),
            "disconnect"=>{self.reference=false;Ok(json!({"connected":false}))},
            _=>Err(format!("Onbekende operatie {op}")),
        }
    }
}
