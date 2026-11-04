use crate::{model::*,transport::*};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::{path::PathBuf,sync::mpsc,time::{SystemTime,UNIX_EPOCH,Duration}};

#[derive(Clone,Debug,Serialize,Deserialize,Default)]
pub struct Record {
    pub kind:String,pub repeat:usize,pub status:String,pub start_steps:i64,pub target_steps:i64,
    pub end_steps:Option<i64>,pub angle_deg:Option<f64>,pub requested_g:Option<f64>,
    pub curve_target_g:Option<f64>,pub correction_pct:Option<f64>,pub mass_g:Option<f64>,
    pub cumulative_g:Option<f64>,pub error_g:Option<f64>,pub error_pct:Option<f64>,
    pub reference_valid:bool,pub scale_status:String,pub error:String,
    pub before_raw_json:String,pub after_raw_json:String,pub motion_raw_json:String,
}
/// Manual command (Handbediening); kept apart from test records so the last test and its report stay intact.
#[derive(Clone,Debug,Serialize,Deserialize,Default)]
pub struct ManualMove {
    pub time_ms:u128,pub kind:String,pub start_steps:i64,pub target_steps:i64,pub end_steps:i64,
    pub scale_g:Option<f64>,pub delta_g:Option<f64>,pub status:String,pub error:String,
}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct State {
    pub connected:bool,pub simulation:bool,pub busy:bool,pub position:i64,pub reference_valid:bool,
    pub homing_status:String,pub status:String,pub config:Config,pub curve:Option<Curve>,
    pub calibration_g_rev:Option<f64>,pub records:Vec<Record>,pub path:String,
    pub reference_origin:i64,pub started_ms:u128,pub test:String,
    pub interpretation:String,pub physical_position_verified:bool,
    #[serde(default)] pub manual:Vec<ManualMove>,
    /// Continuous counter = controller counter + offset; offset grows at each automatic ZERO (tellerreset).
    #[serde(default)] pub controller_offset:i64,#[serde(default)] pub rebases:u32,
    /// Scale connected and usable; false when connected without scale.
    #[serde(default)] pub scale_ok:bool,
    /// Live bench weight for the Monitors page (polled while idle when switched on).
    #[serde(default)] pub live_on:bool,#[serde(default)] pub live_g:Option<f64>,#[serde(default)] pub live_stable:bool,#[serde(default)] pub live_error:String,
}
impl Default for State {
    fn default()->Self{Self{connected:false,simulation:true,busy:false,position:0,reference_valid:false,
        homing_status:"Niet gerefereerd".into(),status:"Verbind simulatie of hardware".into(),config:Config::default(),
        curve:None,calibration_g_rev:None,records:vec![],path:String::new(),reference_origin:0,started_ms:0,test:String::new(),
        interpretation:"Gemeten uitstroom; geen directe interne plunjerverplaatsing. Teller = verzonden doelen, geen fysieke hoekmeting.".into(),physical_position_verified:false,manual:vec![],controller_offset:0,rebases:0,scale_ok:false,
        live_on:false,live_g:None,live_stable:false,live_error:String::new()}}
}
pub enum Action {
    Connect{cfg:Config,simulation:bool,python:PathBuf,repo:PathBuf,bridge:PathBuf},
    Zero,Home,Calibrate{revolutions:u32,repeats:usize,prime:u32},Characterize{interval:Interval,repeats:usize},
    /// One dose per entry, in order; repeats of one weight are simply repeated entries.
    Dose{grams:Vec<f64>,correction:f64},
    /// One dose for the Pomptest: same dosing and checks as `Dose`, but no result files (the database is the source).
    TestDose{grams:f64,correction:f64},
    /// Manual forward move to absolute counter target; optionally weigh before and after.
    Jog{target:i64,weigh:bool},Weigh,
    /// Live settings change while connected (no reconnect); connection fields must stay equal.
    Configure{cfg:Config},
    /// Listen on every IXXAT channel (nothing sent) and report boards with heartbeat.
    CanScan{python:PathBuf,bridge:PathBuf,seconds:f64,bitrate:u32},
    /// Live weight of the bench scale on/off (Monitors page); idle polls only, never during a command.
    LiveWeight(bool),
    Disconnect,Shutdown,
}
pub enum Event {State(Box<State>),Closed}
pub struct Engine {pub state:State,pub device:Option<Box<dyn Device>>,pub stop:StopHandle,pub output:PathBuf,pub events:Option<mpsc::Sender<Event>>,recording:bool,
    /// Set while a Pomptest dose runs: no result files (P7).
    no_files:bool}
fn now()->u128{SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()}
impl Engine {
    pub fn new(output:PathBuf,stop:StopHandle)->Self{Self{state:State::default(),device:None,stop,output,events:None,recording:false,no_files:false}}
    fn publish(&self){if let Some(tx)=&self.events{let _=tx.send(Event::State(Box::new(self.state.clone())));}}
    fn call(&mut self,op:&str,args:Value)->Result<Value>{
        self.stop.check()?;
        self.device.as_mut().ok_or("Eerst verbinden")?.call(op,args)
    }
    fn guard(&mut self)->Result<()> {
        self.stop.check()?;
        if !self.state.reference_valid{return Err("Kies eerst startpositie of home".into());}
        let s=self.call("status",json!({}))?;
        if s["reference_valid"]!=true || s["position"].as_i64().map(|p|p+self.state.controller_offset)!=Some(self.state.position){return Err("Referentie verloren of teller onverwacht gewijzigd".into());}
        Ok(())
    }
    fn weight(&mut self)->Result<(f64,Value)> {
        if !self.state.scale_ok{return Err(NO_SCALE.into());}
        self.guard()?;
        let raw=self.call("weight",json!({}))?;
        let grams=raw["grams"].as_f64().filter(|v|v.is_finite()).ok_or("Geen geldige schaalmeting")?;
        self.guard()?;Ok((grams,raw))
    }
    fn move_to(&mut self,target:i64)->Result<Value>{
        self.guard()?;
        if target<=self.state.position || target>MAX_POSITION{return Err("Vooruitdoel buiten bereik".into());}
        let limit=self.state.config.controller_limit;
        if target-self.state.position>limit{return Err(format!("Beweging groter dan controllerbereik ({limit} stappen); kleinere stap kiezen"));}
        if target-self.state.controller_offset>limit{self.rebase()?;}
        let value=self.call("move",json!({"target":target-self.state.controller_offset}))?;
        // Count sent absolute targets; never sum absolute positions.
        self.state.position=value["position"].as_i64().ok_or("Bewegingsantwoord mist teller")?+self.state.controller_offset;
        if self.state.position!=target || value["reference_valid"]!=true{return Err("Beweging onzeker; referentie ongeldig".into());}
        self.guard()?;Ok(value)
    }
    /// Controller counter near its limit: confirmed ZERO at standstill, continuous count kept as offset.
    /// No motion, so pump phase and active curve stay valid.
    fn rebase(&mut self)->Result<()>{
        self.guard()?;let pos=self.state.position;
        let mut m=ManualMove{time_ms:now(),kind:"Tellerreset (ZERO)".into(),start_steps:pos,target_steps:pos,end_steps:pos,..Default::default()};
        let result=self.call("rebase",json!({})).and_then(|r|{
            if r["reference_valid"]!=true || r["position"].as_i64()!=Some(0){Err("Tellerreset niet bevestigd".to_string())}else{Ok(())}});
        match &result{Ok(())=>m.status="completed".into(),Err(e)=>{m.status="failed".into();m.error=e.clone();}}
        self.log_manual(m);result?;
        self.state.controller_offset=pos;self.state.rebases+=1;
        self.guard()?;Ok(())
    }
    fn begin(&mut self,kind:&str)->Result<()> {
        self.recording=true;
        self.state.test=kind.into();self.state.records.clear();self.state.started_ms=now();
        if self.no_files{self.state.path.clear();self.state.status=format!("{kind} gestart");return Ok(());}
        std::fs::create_dir_all(&self.output).map_err(|e|e.to_string())?;
        self.state.path=self.output.join(format!("{}_{}_{}.json",kind,self.state.started_ms,std::process::id())).display().to_string();
        self.state.status=format!("{kind} gestart");self.save()
    }
    fn save(&self)->Result<()> {
        if self.state.path.is_empty(){return Ok(());}
        let path=PathBuf::from(&self.state.path);let temp=path.with_extension("json.tmp");
        std::fs::write(&temp,serde_json::to_vec_pretty(&self.state).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
        std::fs::rename(temp,&path).map_err(|e|e.to_string())?;
        let csv_path=path.with_extension("csv");let csv_temp=path.with_extension("csv.tmp");
        let mut writer=csv::Writer::from_path(&csv_temp).map_err(|e|e.to_string())?;
        for row in &self.state.records{writer.serialize(row).map_err(|e|e.to_string())?;}
        writer.flush().map_err(|e|e.to_string())?;drop(writer);
        std::fs::rename(csv_temp,csv_path).map_err(|e|e.to_string())?;Ok(())
    }
    fn measure_move(&mut self,mut row:Record)->Result<f64>{
        row.status="running".into();row.reference_valid=true;
        self.state.records.push(row);let i=self.state.records.len()-1;self.save()?;self.publish();
        let result:Result<f64>=(||{
            let (before,raw)=self.weight()?;self.state.records[i].before_raw_json=raw.to_string();
            self.save()?;
            let target=self.state.records[i].target_steps;
            let motion=self.move_to(target)?;self.state.records[i].motion_raw_json=motion.to_string();
            self.state.records[i].end_steps=Some(self.state.position);self.save()?;
            let (after,raw)=self.weight()?;let mass=after-before;
            let r=&mut self.state.records[i];r.after_raw_json=raw.to_string();r.mass_g=Some(mass);r.status="completed".into();r.scale_status="stable".into();
            if let Some(target)=r.requested_g{r.error_g=Some(mass-target);r.error_pct=Some(100.*(mass-target)/target);}
            Ok(mass)
        })();
        if let Err(e)=&result{let r=&mut self.state.records[i];r.status="failed".into();r.error=e.clone();r.reference_valid=false;r.scale_status="unconfirmed".into();}
        // On transport error query software counter when possible; never claim a physical position.
        if result.is_err(){let offset=self.state.controller_offset;if let Some(d)=&mut self.device{if let Ok(s)=d.call("status",json!({})){if let Some(p)=s["position"].as_i64(){self.state.position=p+offset;}}}}
        self.state.records[i].end_steps=Some(self.state.position);
        self.save()?;self.publish();result
    }
    fn log_manual(&mut self,m:ManualMove){
        // Append-only CSV next to test results; a log failure never hides the move result.
        let path=self.output.join("handbediening.csv");let new=!path.exists();
        let _=std::fs::create_dir_all(&self.output);
        if let Ok(f)=std::fs::OpenOptions::new().create(true).append(true).open(&path){
            let mut w=csv::WriterBuilder::new().has_headers(new).from_writer(f);let _=w.serialize(&m);let _=w.flush();
        }
        self.state.manual.push(m);
        if self.state.manual.len()>MAX_MANUAL{self.state.manual.remove(0);}
    }
    fn configure(&mut self,mut cfg:Config)->Result<()>{
        cfg.validate()?;
        let mut scale_note:Option<String>=None;let mut scale_ok:Option<bool>=None;
        let old=self.state.config.clone();
        if let Some(device)=&mut self.device{
            if !cfg.same_connection(&old){return Err("CAN-kanaal en bitrate alleen wijzigen na verbinding verbreken".into());}
            let reply=device.call("configure",serde_json::to_value(&cfg).map_err(|e|e.to_string())?);
            if let Ok(r)=&reply{
                // Scale (re)connect failure is reported, not raised: pump reference stays valid.
                scale_note=r["scale_error"].as_str().map(|e|format!("; weegschaal niet verbonden: {e}"));
                scale_ok=r["scale_connected"].as_bool();
            }
            if let Err(e)=reply{
                // Bridge drops its reference on any failed request; mirror that.
                self.state.reference_valid=false;self.state.curve=None;self.state.homing_status="Ongeldig na instelfout".into();
                return Err(e);
            }
        }
        let mut changes=vec![];
        macro_rules! diff{($($f:ident,$label:expr);*)=>{$(if old.$f!=cfg.$f{changes.push(format!("{} {} -> {}",$label,old.$f,cfg.$f));})*}}
        diff!(node,"CAN-node";motor,"motor";pump_type,"pomptype";steps_rev,"stappen/omw";speed,"snelheid";accel,"acceleratie";decel,"deceleratie";
            stroke_setting,"slag";fluid,"vloeistof";window_s,"venster s";range_g,"variatie g";timeout_s,"timeout s";
            settle_s,"uitloop s";require_feedback,"MOVE_TO_DONE";controller_limit,"controllergrens";
            use_scale,"weegschaal";port,"weegschaalpoort";baud,"baud";scale_tcp,"weegschaal via netwerk";scale_host,"weegschaal-IP";scale_tcp_port,"weegschaal-TCP-poort";
            scale_dual,"LH1+LH2 tegelijk";scale_host2,"weegschaal-IP LH2";scale_tcp_port2,"weegschaal-TCP-poort LH2";measure_head,"meetkop (0=LH1,1=LH2)");
        let pump=!old.same_pump(&cfg);
        let flow=old.stroke_setting.trim()!=cfg.stroke_setting.trim() || old.fluid.trim()!=cfg.fluid.trim();
        let mut effect=String::new();
        if pump && self.state.connected{
            self.state.reference_valid=false;self.state.curve=None;self.state.calibration_g_rev=None;
            self.state.homing_status="Pomp gewijzigd; opnieuw refereren".into();
            effect="; pomp gewijzigd: opnieuw startpositie kiezen, kalibreren en curve meten".into();
        }else if (pump || flow) && (self.state.curve.is_some() || self.state.calibration_g_rev.is_some()){
            self.state.curve=None;self.state.calibration_g_rev=None;
            effect="; slag/vloeistof gewijzigd: kalibratie en meetcurve opnieuw meten".into();
        }
        if let Some(ok)=scale_ok{self.state.scale_ok=ok;if !ok{cfg.use_scale=false;}}
        if let Some(note)=&scale_note{effect.push_str(note);}
        self.state.config=cfg;
        if changes.is_empty(){self.state.status="Geen instellingen gewijzigd".into();return Ok(());}
        if self.state.connected{
            let pos=self.state.position;
            self.log_manual(ManualMove{time_ms:now(),kind:format!("Instellingen: {}",changes.join(", ")),start_steps:pos,target_steps:pos,end_steps:pos,status:"completed".into(),..Default::default()});
        }
        self.state.status=format!("Instellingen toegepast: {}{effect}",changes.join(", "));
        Ok(())
    }
    fn can_scan(&mut self,python:&std::path::Path,bridge:&std::path::Path,seconds:f64,bitrate:u32)->Result<String>{
        if self.device.is_some() && !self.state.simulation{
            return Err("Eerst pompbank verbreken: de test opent de CAN-kanalen zelf".into());}
        if !(1. ..=40.).contains(&seconds){return Err("Luistertijd 1 tot 40 s".into());}
        let mut dev=Bridge::spawn(python,bridge,&self.output,&self.output,StopHandle::default())?;
        let r=dev.call("scan",json!({"seconds":seconds,"bitrate":bitrate}))?;
        format_scan(&r,seconds,bitrate)
    }
    /// Readable result of a bus scan (`vsd_bridge.py scan` answer, or the simulated one).
    pub fn scan_text(r:&Value,seconds:f64,bitrate:u32)->Result<String>{
        let mut parts=vec![];
        for c in r["channels"].as_array().into_iter().flatten(){
            let ch=c["channel"].as_i64().unwrap_or(-1);
            let line=if let Some(e)=c["error"].as_str(){format!("kanaal {ch}: niet te openen ({e})")}else{
                let hb:Vec<String>=c["heartbeat"].as_array().into_iter().flatten().map(|n|n.to_string()).collect();
                let nodes:Vec<String>=c["nodes"].as_object().into_iter().flatten().map(|(k,v)|format!("{k}x{v}")).collect();
                format!("kanaal {ch}: {} frames, {} foutframes, hartslag van {}, nodes {}",c["frames"],c["errors"],if hb.is_empty(){"niemand".into()}else{hb.join(", ")},if nodes.is_empty(){"geen".into()}else{nodes.join(" ")})};
            crate::applog::info("CAN-TEST",&line);parts.push(line);
        }
        if parts.is_empty(){return Err("Geen IXXAT-kanaal gevonden".into());}
        let silent=r["channels"].as_array().into_iter().flatten().all(|c|c["frames"].as_u64().unwrap_or(0)==0);
        let advice=if silent{format!(" — bus stil bij {bitrate} bit/s: kabel, voeding van de borden of bitrate controleren")}else{String::new()};
        Ok(format!("CAN-test {seconds:.0} s: {}{advice}",parts.join("; ")))
    }
    /// Idle poll of the bench scale for the live display; errors are shown, never raised.
    pub fn bench_poll(&mut self){
        if !self.state.live_on || !self.state.connected || !self.state.scale_ok || self.state.busy{return;}
        let Some(d)=self.device.as_mut() else{return};
        match d.call("weight",json!({})){
            Ok(r)=>{let g=r["grams"].as_f64();let st=r["scale_status"].as_str().unwrap_or("").contains("stable");
                if g!=self.state.live_g || st!=self.state.live_stable || !self.state.live_error.is_empty(){self.state.live_g=g;self.state.live_stable=st;self.state.live_error.clear();self.publish();}}
            Err(e)=>{if self.state.live_error!=e{self.state.live_error=e;self.publish();}}
        }
    }
    fn validate_repeats(n:usize)->Result<()>{if !(1..=100).contains(&n){Err("Herhalingen: 1 tot 100".into())}else{Ok(())}}
    /// Every action and its outcome goes to the application log; known CAN-bus faults get a hint.
    pub fn execute(&mut self,action:Action)->Result<()> {
        let what=describe(&action);
        crate::applog::info("ACTIE",&what);
        let result=self.execute_logged(action);
        match &result{
            Ok(())=>crate::applog::info("OK",&format!("{what}: {}",self.state.status)),
            Err(e)=>{
                let hint=explain(e);
                crate::applog::error("FOUT",&format!("{what}: {e}{hint}"));
                if !hint.is_empty(){self.state.status=format!("{}{hint}",self.state.status);self.publish();}
            }
        }
        result
    }
    fn execute_logged(&mut self,action:Action)->Result<()> {
        // Settings: a rejected change must not invalidate reference or curve like a failed measurement does.
        if let Action::LiveWeight(on)=action{
            self.state.live_on=on;if !on{self.state.live_g=None;self.state.live_error.clear();}
            self.state.busy=false;self.publish();return Ok(());
        }
        if let Action::CanScan{python,bridge,seconds,bitrate}=&action{
            let result=self.can_scan(python,bridge,*seconds,*bitrate);
            match &result{Ok(s)=>self.state.status=s.clone(),Err(e)=>self.state.status=format!("CAN-test: {e}")}
            self.state.busy=false;self.publish();return result.map(|_|());
        }
        // Weighing actions without scale: refuse up front, keep reference and curve.
        let weighs=matches!(action,Action::Calibrate{..}|Action::Characterize{..}|Action::Dose{..}|Action::TestDose{..}|Action::Weigh|Action::Jog{weigh:true,..});
        if weighs && self.state.connected && !self.state.scale_ok{
            self.state.status=NO_SCALE.into();self.state.busy=false;self.publish();return Err(NO_SCALE.into());
        }
        if let Action::Configure{cfg}=action {
            let result=self.configure(cfg);
            if let Err(e)=&result{self.state.status=format!("Instellingen niet toegepast: {e}");}
            self.state.busy=false;self.publish();return result;
        }
        self.recording=false;
        let result=self.execute_inner(action);
        if let Err(e)=&result {
            self.state.reference_valid=false;self.state.curve=None;self.state.homing_status="Ongeldig na stop/fout".into();
            self.stop.stop();self.state.status=format!("Afgebroken: {e}");
        }
        self.state.busy=false;
        if self.recording{if let Err(e)=self.save(){self.state.reference_valid=false;self.state.curve=None;self.stop.stop();self.state.status=format!("Opslag mislukt: {e}");self.publish();return Err(e);}}
        self.publish();result
    }
    fn execute_inner(&mut self,action:Action)->Result<()> {
        match action {
            Action::Connect{cfg,simulation,python,repo,bridge}=>{
                cfg.validate()?;
                if self.device.is_some(){return Err("Eerst verbinding verbreken".into());}
                self.stop.reset();self.state=State::default();self.state.config=cfg.clone();self.state.simulation=simulation;
                let mut device:Box<dyn Device>=if simulation{Box::new(Mock::new(cfg.clone(),self.stop.clone()))}else{
                    Box::new(Bridge::spawn(&python,&bridge,&repo,&self.output,self.stop.clone())?)};
                device.call("connect",serde_json::to_value(&cfg).map_err(|e|e.to_string())?)?;
                self.device=Some(device);self.state.connected=true;self.state.scale_ok=cfg.use_scale;
                self.state.status=match(simulation,cfg.use_scale){(true,_)=>"Simulatie verbonden — voorbeeldcurve, geen machine",(false,true)=>"Hardware verbonden — kies startpositie",(false,false)=>"Pomp verbonden zonder weegschaal — alleen bewegen en instellen"}.into();
            }
            Action::Configure{cfg}=>self.configure(cfg)?,
            Action::CanScan{..}|Action::LiveWeight(_)=>{},
            Action::Disconnect|Action::Shutdown=>{
                self.stop.stop();if let Some(d)=&mut self.device{let _=d.call("disconnect",json!({}));}self.device=None;self.state.scale_ok=false;
                self.state.connected=false;self.state.reference_valid=false;self.state.curve=None;self.state.status="Verbinding gesloten".into();
            }
            Action::Zero|Action::Home=>{
                self.stop.reset();self.state.curve=None;self.state.reference_valid=false;
                let op=if matches!(action,Action::Home){"home"}else{"zero"};
                let response=self.call(op,json!({}))?;
                if response["reference_valid"]!=true || response["position"].as_i64()!=Some(0){return Err("Startpositie niet ingesteld".into());}
                self.state.position=0;self.state.reference_origin=0;self.state.reference_valid=true;self.state.controller_offset=0;self.state.rebases=0;
                self.state.homing_status=response["homing_status"].as_str().unwrap_or("software_origin").into();
                self.state.status="Startpositie ingesteld; meet curve vanaf huidige teller".into();
            }
            Action::Calibrate{revolutions,repeats,prime}=>{
                Self::validate_repeats(repeats)?;self.guard()?;
                if revolutions==0{return Err("Minimaal één gehele omwenteling".into());}
                let limit=self.state.config.controller_limit;
                if self.state.config.steps_rev.saturating_mul(revolutions.max(prime) as i64)>limit{return Err(format!("Maximaal {} omwentelingen per meting of prime (controllerbereik)",limit/self.state.config.steps_rev));}
                if prime>100{return Err("Prime: 0 tot 100 omwentelingen".into());}
                let delta=self.state.config.steps_rev.checked_mul(revolutions as i64).ok_or("Positieteller vol")?;
                let prime_steps=self.state.config.steps_rev as i128*prime as i128;
                if self.state.position as i128+prime_steps+delta as i128*repeats as i128>MAX_POSITION as i128{return Err("Positieteller vol".into());}
                self.state.curve=None;self.begin("kalibratie")?;let mut yields=vec![];
                if prime>0{
                    // Prime: whole forward revolutions without weighing, as in original pump_debug tool.
                    let start=self.state.position;let target=start+prime_steps as i64;
                    self.state.records.push(Record{kind:"Prime".into(),status:"running".into(),start_steps:start,target_steps:target,reference_valid:true,..Default::default()});
                    self.state.status=format!("Primen: {prime} omwentelingen");self.save()?;self.publish();
                    let moved=self.move_to(target);
                    let r=self.state.records.last_mut().unwrap();r.end_steps=Some(self.state.position);
                    match moved{Ok(v)=>{r.status="completed".into();r.motion_raw_json=v.to_string();},Err(e)=>{r.status="failed".into();r.error=e.clone();r.reference_valid=false;self.save()?;return Err(e);}}
                    self.save()?;self.publish();
                }
                for repeat in 1..=repeats {
                    let start=self.state.position;
                    let mass=self.measure_move(Record{kind:"Kalibratie".into(),repeat,start_steps:start,target_steps:start+delta,..Default::default()})?;
                    yields.push(mass/revolutions as f64);
                }
                let (mean,sd)=stats(&yields);if mean<=0.{return Err("Geen positieve kalibratiemassa".into());}
                self.state.calibration_g_rev=Some(mean);self.state.status=format!("Kalibratie: {mean:.5} g/omw; spreiding {sd:.5} g/omw");
            }
            Action::Characterize{interval,repeats}=>{
                Self::validate_repeats(repeats)?;self.guard()?;let cfg=self.state.config.clone();
                let targets=interval.targets(cfg.steps_rev)?;
                if targets.len()*repeats>100000 || self.state.position as i128+cfg.steps_rev as i128*repeats as i128>MAX_POSITION as i128{return Err("Meetreeks te groot".into());}
                self.state.curve=None;let origin=self.state.position;self.state.reference_origin=origin;self.begin("characterization")?;
                let mut runs=vec![];
                for repeat in 1..=repeats {
                    self.guard()?;let tare=self.call("tare",json!({}))?;let (baseline,raw)=self.weight()?;
                    let start=self.state.position;let mut run=vec![0.];let mut cumulative=0.;
                    self.state.records.push(Record{kind:"360°".into(),repeat,status:"completed".into(),start_steps:start,target_steps:start,end_steps:Some(start),angle_deg:Some(0.),mass_g:Some(0.),cumulative_g:Some(0.),reference_valid:true,scale_status:"stable".into(),before_raw_json:tare.to_string(),after_raw_json:raw.to_string(),..Default::default()});
                    for (angle,offset) in &targets{
                        let mass=self.measure_move(Record{kind:"360°".into(),repeat,start_steps:self.state.position,target_steps:start+offset,angle_deg:Some(*angle),..Default::default()})?;
                        // Cumulative from absolute baseline, not successive noisy differences.
                        let last=self.state.records.last_mut().unwrap();
                        let after:Value=serde_json::from_str(&last.after_raw_json).map_err(|e|e.to_string())?;
                        cumulative=after["grams"].as_f64().ok_or("Massa ontbreekt")?-baseline;
                        last.mass_g=Some(cumulative-*run.last().unwrap());last.cumulative_g=Some(cumulative);
                        let _=mass;run.push(cumulative);self.save()?;self.publish();
                    }
                    let _=cumulative;runs.push(run);
                }
                let steps=std::iter::once(0).chain(targets.iter().map(|(_,s)|*s)).collect();
                let curve=Curve::fit(cfg.steps_rev,origin,steps,&runs)?;
                self.state.status=format!("Meetcurve klaar: {:.5} g/omw; maximale monotone aanpassing {:.5} g. Ruwe metingen bewaard.",curve.yield_g(),curve.correction_g);
                self.state.curve=Some(curve);
            }
            Action::Jog{target,weigh}=>{
                self.guard()?;let start=self.state.position;
                let max=self.state.config.steps_rev.saturating_mul(MAX_JOG_REVS);
                if target<=start{return Err("Alleen vooruit: doel moet boven huidige teller liggen".into());}
                if target-start>max || target>MAX_POSITION{return Err(format!("Handmatig maximaal {MAX_JOG_REVS} omwentelingen per opdracht"));}
                self.state.status=format!("Handmatig: {start} -> {target}");self.publish();
                let mut m=ManualMove{time_ms:now(),kind:"Beweging".into(),start_steps:start,target_steps:target,..Default::default()};
                let result:Result<()>=(||{
                    let before=if weigh{Some(self.weight()?.0)}else{None};
                    self.move_to(target)?;
                    if let Some(b)=before{let (after,_)=self.weight()?;m.scale_g=Some(after);m.delta_g=Some(after-b);}
                    Ok(())
                })();
                m.end_steps=self.state.position;
                match &result{Ok(())=>m.status="completed".into(),Err(e)=>{m.status="failed".into();m.error=e.clone();}}
                let delta=m.delta_g;self.log_manual(m);result?;
                self.state.status=format!("Handmatig verplaatst {start} -> {target} (+{} st){}",target-start,delta.map(|d|format!("; gewogen {d:.5} g")).unwrap_or_default());
            }
            Action::Weigh=>{
                let pos=self.state.position;
                let mut m=ManualMove{time_ms:now(),kind:"Weging".into(),start_steps:pos,target_steps:pos,end_steps:pos,..Default::default()};
                let result=self.weight();
                match &result{Ok((g,_))=>{m.scale_g=Some(*g);m.status="completed".into();},Err(e)=>{m.status="failed".into();m.error=e.clone();}}
                self.log_manual(m);let (g,_)=result?;
                let c=&self.state.config;let active=c.measure_head as usize;
                if c.scale_tcp && c.scale_dual{
                    // Both heads connected: also read the other one (measurements keep using the active head).
                    let other=1-active;
                    let o=self.call("weight",json!({"head":other})).ok().and_then(|r|r["grams"].as_f64());
                    let mut w=[None,None];w[active]=Some(g);w[other]=o;
                    let f=|v:Option<f64>|v.map(|g|format!("{g:.5} g")).unwrap_or("niet gelezen".into());
                    self.state.status=format!("Weegschaal LH1: {} · LH2: {} (meetkop LH{})",f(w[0]),f(w[1]),active+1);
                }else{self.state.status=format!("Weegschaal: {g:.5} g (stabiel)");}
            }
            Action::TestDose{grams,correction}=>{
                self.no_files=true;let r=self.execute_inner(Action::Dose{grams:vec![grams],correction});self.no_files=false;r?;
            }
            Action::Dose{grams,correction}=>{
                if !(1..=MAX_DOSES).contains(&grams.len()){return Err(format!("Doses: 1 tot {MAX_DOSES}"));}
                let nominal:Vec<f64>=grams.iter().map(|g|corrected_mass(*g,correction)).collect::<Result<_>>()?;self.guard()?;
                let curve=self.state.curve.clone().ok_or("Eerst 360° meetcurve afronden")?;
                let mut planned=self.state.position;
                for n in &nominal{planned=curve.target(planned,*n)?;}
                let single=grams.iter().all(|g|*g==grams[0]);
                self.begin(if single{"dosering"}else{"doseertest"})?;
                for (i,(g,n)) in grams.iter().zip(&nominal).enumerate() {
                    self.guard()?;let start=self.state.position;let target=curve.target(start,*n)?;
                    self.state.status=format!("Dosis {} / {}: {g:.3} g",i+1,grams.len());self.publish();
                    self.measure_move(Record{kind:"Dosering".into(),repeat:i+1,start_steps:start,target_steps:target,requested_g:Some(*g),curve_target_g:Some(*n),correction_pct:Some(correction),..Default::default()})?;
                }
                if single{
                    let values:Vec<_>=self.state.records.iter().filter_map(|r|r.mass_g).collect();let(mean,sd)=stats(&values);
                    self.state.status=format!("Dosering klaar: gemiddeld {mean:.5} g; spreiding {sd:.5} g; correctie {correction:+0.2}%");
                }else{
                    let errors:Vec<_>=self.state.records.iter().filter_map(|r|r.error_pct).collect();let(mean,sd)=stats(&errors);
                    self.state.status=format!("Doseertest klaar: {} doses; gemiddelde fout {mean:+.3}%; spreiding {sd:.3}%; correctie {correction:+0.2}%",errors.len());
                }
                // Report failure never fails the measured run; it can be regenerated from the results page.
                // Single doses (one per action, as the Pomptest sends them) get no report of their own.
                if grams.len()>1{match crate::report::dose_report(&self.state){
                    Ok(p)=>self.state.status+=&format!("; rapport {}",p.file_name().and_then(|f|f.to_str()).unwrap_or("")),
                    Err(e)=>self.state.status+=&format!("; rapport mislukt: {e}"),
                }}
            }
        } Ok(())
    }
}

pub fn format_scan(r:&Value,seconds:f64,bitrate:u32)->Result<String>{Engine::scan_text(r,seconds,bitrate)}

/// Manual forward target of the ceramic pump. mode 0: value in unit (0 stappen, 1 omwentelingen, 2 graden);
/// 1: absolute counter; 2: next counter with phase `value`° from `origin`.
pub fn jog_target(pos:i64,n:i64,origin:i64,mode:usize,unit:usize,value:f64)->Result<i64>{
    let target=match mode{
        0=>{let per=[1.,n as f64,n as f64/360.][unit.min(2)];pos+(value*per).round() as i64}
        1=>value as i64,
        _=>{let want=(value.rem_euclid(360.)*n as f64/360.).round() as i64%n;let cur=(pos-origin).rem_euclid(n);
            let d=(want-cur).rem_euclid(n);if d==0{return Err("Pomp staat al op deze fase".into());}pos+d}
    };
    if target<=pos{return Err("Alleen vooruit: doel moet boven huidige teller liggen".into());}
    if target-pos>n.saturating_mul(MAX_JOG_REVS){return Err(format!("Maximaal {MAX_JOG_REVS} omwentelingen per opdracht"));}
    if target>MAX_POSITION{return Err("Positieteller vol".into());}
    Ok(target)
}

/// Extra advice for errors whose cause is outside the program.
pub fn explain(e:&str)->String{
    let l=e.to_lowercase();
    if l.contains("error warning limit") || l.contains("bus off") || l.contains("busoff") || l.contains("error passive"){
        " — CAN-busstoring: niemand bevestigt de berichten. Controleer bitrate (250 kbit/s), kabel en afsluitweerstand, en of de pompborden aanstaan.".into()
    }else if l.contains("invalid handle"){" — CAN-interface gesloten na eerdere fout; opnieuw verbinden.".into()
    }else if l.contains("access is denied") || l.contains("toegang geweigerd") || l.contains("permissionerror"){" — poort bezet door ander programma (Camelot, Omron-service).".into()
    }else{String::new()}
}
/// Short, readable description of an action for the log (no full pump lists).
pub fn describe(a:&Action)->String{
    match a{
        Action::Connect{cfg,simulation,..}=>format!("Pompbank verbinden ({}; {} node {} motor {}; weegschaal {})",if *simulation{"simulatie"}else{"hardware"},cfg.pump_type,cfg.node,cfg.motor,
            if !cfg.use_scale{"uit".to_string()}else if cfg.scale_tcp && cfg.scale_dual{format!("LH1 {}:{} + LH2 {}:{}, meten op LH{}",cfg.scale_host.trim(),cfg.scale_tcp_port,cfg.scale_host2.trim(),cfg.scale_tcp_port2,cfg.measure_head+1)}
            else if cfg.scale_tcp{format!("{}:{}",cfg.scale_host.trim(),cfg.scale_tcp_port)}else{format!("{} {} baud",cfg.port,cfg.baud)}),
        Action::Zero=>"Startpositie 0".into(),Action::Home=>"Home via sensor".into(),
        Action::Calibrate{revolutions,repeats,prime}=>format!("Kalibratie {revolutions} omw × {repeats}, prime {prime}"),
        Action::Characterize{interval,repeats}=>format!("Meetcurve {interval:?} × {repeats}"),
        Action::TestDose{grams,correction}=>format!("Testdosis {grams:.3} g, correctie {correction:+.2}%"),
        Action::Dose{grams,correction}=>format!("Doseren {} doses ({:.3}–{:.3} g), correctie {correction:+.2}%",grams.len(),grams.iter().cloned().fold(f64::MAX,f64::min),grams.iter().cloned().fold(f64::MIN,f64::max)),
        Action::Jog{target,weigh}=>format!("Handmatig naar {target}{}",if *weigh{" met weging"}else{""}),
        Action::Weigh=>"Weegschaal lezen".into(),
        Action::Configure{..}=>"Instellingen toepassen".into(),
        Action::CanScan{seconds,bitrate,..}=>format!("CAN-bus testen ({seconds:.0} s luisteren, {bitrate} bit/s)"),
        Action::LiveWeight(on)=>format!("Live wegen pompbank {}",if *on{"aan"}else{"uit"}),
        Action::Disconnect=>"Pompbank verbreken".into(),Action::Shutdown=>"Afsluiten".into(),
    }
}

pub const MAX_DOSES:usize=1000;
const NO_SCALE:&str="Geen weegschaal verbonden: zet Weegschaal gebruiken aan bij Verbinding en instellingen";
pub const MAX_JOG_REVS:i64=100;
const MAX_MANUAL:usize=200;

pub fn spawn_worker(output:PathBuf,stop:StopHandle)->(mpsc::Sender<Action>,mpsc::Receiver<Event>){
    let(tx,rx)=mpsc::channel();let(etx,erx)=mpsc::channel();
    std::thread::spawn(move||{
        let mut engine=Engine::new(output,stop);engine.events=Some(etx.clone());
        let mut last_idle=std::time::Instant::now();
        loop {
            // ~3 Hz for the live bench weight; the reference check stays at 1 Hz.
            match rx.recv_timeout(Duration::from_millis(330)){
                Ok(action)=>{let shutdown=matches!(&action,Action::Shutdown);engine.state.busy=true;engine.publish();let _=engine.execute(action);if shutdown{let _=etx.send(Event::Closed);break;}},
                Err(mpsc::RecvTimeoutError::Disconnected)=>{let _=engine.execute(Action::Shutdown);break;},
                Err(mpsc::RecvTimeoutError::Timeout)=>{
                    engine.bench_poll();
                    if last_idle.elapsed()<Duration::from_secs(1){continue;}
                    last_idle=std::time::Instant::now();
                    if engine.state.reference_valid {if let Err(e)=engine.guard(){crate::applog::error("REFERENTIE",&format!("Referentie verloren tijdens wachten: {e}{}",explain(&e)));engine.state.reference_valid=false;engine.state.curve=None;engine.state.status=e;engine.stop.stop();engine.publish();}}
                }
            }
        }
    });(tx,erx)
}

#[cfg(test)]mod tests {
    use super::*;
    /// Test engine whose output folder in %TEMP% is removed when the test ends (pass or fail).
    struct TestEngine(Engine);
    impl std::ops::Deref for TestEngine{type Target=Engine;fn deref(&self)->&Engine{&self.0}}
    impl std::ops::DerefMut for TestEngine{fn deref_mut(&mut self)->&mut Engine{&mut self.0}}
    impl Drop for TestEngine{fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0.output);}}
    fn engine(name:&str)->TestEngine{let path=std::env::temp_dir().join(format!("pump-rust-{name}-{}-{}",std::process::id(),now()));let mut e=TestEngine(Engine::new(path,StopHandle::default()));e.state.config.settle_s=0.;e.state.connected=true;e.state.scale_ok=true;let m=Mock::new(e.state.config.clone(),e.stop.clone());e.device=Some(Box::new(m));e.execute(Action::Zero).unwrap();e}
    #[test]fn complete_mock_workflow_and_manual_correction(){
        let mut e=engine("workflow");e.execute(Action::Calibrate{revolutions:1,repeats:2,prime:0}).unwrap();assert_eq!(e.state.position,51200);
        e.execute(Action::Characterize{interval:Interval::Degrees(10.),repeats:5}).unwrap();assert_eq!(e.state.position,179200);let c=e.state.curve.clone().unwrap();assert_eq!(c.origin,51200);assert!((c.yield_g()-1.345).abs()<1e-10);
        e.execute(Action::Dose{grams:vec![5.;2],correction:-1.}).unwrap();assert_eq!(e.state.records.len(),2);
        for r in &e.state.records {assert_eq!(r.requested_g,Some(5.));assert_eq!(r.curve_target_g,Some(4.95));assert!((r.mass_g.unwrap()-4.95).abs()<0.001);assert!((r.error_g.unwrap()+0.05).abs()<0.001);}
        assert_eq!(e.state.records[1].start_steps,e.state.records[0].end_steps.unwrap());
        let saved:State=serde_json::from_slice(&std::fs::read(&e.state.path).unwrap()).unwrap();assert!(!saved.busy);assert_eq!(saved.records[1].correction_pct,Some(-1.));
        assert!(PathBuf::from(&e.state.path).with_extension("csv").exists());assert_eq!(e.state.curve.as_ref().unwrap().masses,c.masses);
    }
    #[test]fn prime_moves_forward_without_weighing(){
        let mut e=engine("prime");e.execute(Action::Calibrate{revolutions:1,repeats:1,prime:2}).unwrap();
        assert_eq!(e.state.records[0].kind,"Prime");assert!(e.state.records[0].mass_g.is_none());assert_eq!(e.state.records[0].end_steps,Some(51200));
        assert_eq!(e.state.records[1].start_steps,51200);assert_eq!(e.state.position,76800);assert!((e.state.calibration_g_rev.unwrap()-1.345).abs()<1e-9);
    }
    #[test]fn step_grid_and_mixed_dose_series(){
        let mut e=engine("series");e.execute(Action::Characterize{interval:Interval::Steps(1000),repeats:2}).unwrap();
        let c=e.state.curve.clone().unwrap();assert_eq!(c.steps.len(),27);assert_eq!(*c.steps.last().unwrap(),25600);
        e.execute(Action::Dose{grams:vec![1.,2.5,0.7,4.],correction:0.}).unwrap();
        assert!(e.state.path.contains("doseertest"));assert_eq!(e.state.records.len(),4);assert!(crate::report::report_path(&e.state).unwrap().exists());
        for (r,g) in e.state.records.iter().zip([1.,2.5,0.7,4.]){assert_eq!(r.requested_g,Some(g));assert!((r.mass_g.unwrap()-g).abs()<0.001);}
        for w in e.state.records.windows(2){assert_eq!(w[1].start_steps,w[0].end_steps.unwrap());}
        assert!(e.execute(Action::Dose{grams:vec![],correction:0.}).is_err());
    }
    #[test]fn manual_jog_keeps_curve_and_test_records(){
        let mut e=engine("jog");e.execute(Action::Characterize{interval:Interval::Degrees(90.),repeats:1}).unwrap();
        e.execute(Action::Dose{grams:vec![1.],correction:0.}).unwrap();let path=e.state.path.clone();let pos=e.state.position;
        e.execute(Action::Jog{target:pos+12800,weigh:true}).unwrap();
        assert_eq!(e.state.position,pos+12800);assert!(e.state.curve.is_some());assert_eq!(e.state.path,path);assert_eq!(e.state.records.len(),1);
        let m=e.state.manual.last().unwrap();assert_eq!((m.start_steps,m.end_steps,m.status.as_str()),(pos,pos+12800,"completed"));assert!(m.delta_g.is_some());
        e.execute(Action::Weigh).unwrap();assert_eq!(e.state.manual.len(),2);assert!(e.output.join("handbediening.csv").exists());
        assert!(e.execute(Action::Jog{target:pos,weigh:false}).is_err());assert!(!e.state.reference_valid);
    }
    #[test]fn controller_counter_rebases_and_keeps_curve(){
        let mut e=engine("rebase");e.state.config.controller_limit=100_000;
        e.execute(Action::Characterize{interval:Interval::Degrees(90.),repeats:1}).unwrap();
        e.execute(Action::Dose{grams:vec![1.345;6],correction:0.}).unwrap();
        // 6 whole revolutions after the curve: app counter passes the limit several times, controller never does.
        assert!(e.state.rebases>=1);assert!(e.state.position>100_000);assert!(e.state.position-e.state.controller_offset<=100_000);
        assert!(e.state.curve.is_some());
        for r in &e.state.records{assert!((r.mass_g.unwrap()-1.345).abs()<0.002,"{:?}",r.mass_g);}
        assert!(e.state.manual.iter().any(|m|m.kind.starts_with("Tellerreset") && m.status=="completed"));
        let far=e.state.position+100_001;assert!(e.execute(Action::Jog{target:far,weigh:false}).is_err());
    }
    #[test]fn live_configure_keeps_or_drops_curve(){
        let mut e=engine("cfg");e.execute(Action::Characterize{interval:Interval::Degrees(90.),repeats:1}).unwrap();
        let mut c=e.state.config.clone();c.speed=30000;c.window_s=2.;
        e.execute(Action::Configure{cfg:c.clone()}).unwrap();
        assert!(e.state.curve.is_some() && e.state.reference_valid);assert_eq!(e.state.config.speed,30000);
        e.execute(Action::Dose{grams:vec![1.],correction:0.}).unwrap();
        let mut bad=c.clone();bad.channel=1;
        assert!(e.execute(Action::Configure{cfg:bad}).is_err());assert!(e.state.curve.is_some() && e.state.reference_valid);
        c.stroke_setting="2".into();e.execute(Action::Configure{cfg:c.clone()}).unwrap();
        assert!(e.state.curve.is_none() && e.state.reference_valid);
        c.steps_rev=51200;e.execute(Action::Configure{cfg:c.clone()}).unwrap();assert!(!e.state.reference_valid);
        e.execute(Action::Zero).unwrap();assert!(e.state.reference_valid);
        // Other pump on the same controller: motor 2 -> 8 without reconnect.
        c.motor=8;e.execute(Action::Configure{cfg:c}).unwrap();assert!(!e.state.reference_valid && e.state.connected);assert_eq!(e.state.config.motor,8);
        e.execute(Action::Zero).unwrap();e.execute(Action::Calibrate{revolutions:1,repeats:1,prime:0}).unwrap();
        assert!(e.state.manual.iter().any(|m|m.kind.starts_with("Instellingen")));
    }
    #[test]fn connect_without_scale_moves_but_never_weighs(){
        let path=std::env::temp_dir().join(format!("pump-rust-noscale-{}-{}",std::process::id(),now()));let mut e=TestEngine(Engine::new(path,StopHandle::default()));
        let mut cfg=Config::default();cfg.use_scale=false;cfg.port=String::new();cfg.settle_s=0.;
        e.execute(Action::Connect{cfg:cfg.clone(),simulation:true,python:PathBuf::new(),repo:PathBuf::new(),bridge:PathBuf::new()}).unwrap();
        assert!(e.state.connected && !e.state.scale_ok);e.execute(Action::Zero).unwrap();
        e.execute(Action::Jog{target:12800,weigh:false}).unwrap();assert_eq!(e.state.position,12800);
        for a in [Action::Weigh,Action::Calibrate{revolutions:1,repeats:1,prime:0},Action::Jog{target:25600,weigh:true}]{assert!(e.execute(a).is_err());}
        assert!(e.state.reference_valid);assert_eq!(e.state.position,12800);
        // Scale switched on live, no reconnect.
        cfg.use_scale=true;cfg.port="COM9".into();e.execute(Action::Configure{cfg}).unwrap();assert!(e.state.scale_ok && e.state.reference_valid);
        e.execute(Action::Calibrate{revolutions:1,repeats:1,prime:0}).unwrap();
    }
    #[test]fn failed_scale_invalidates_reference_and_preserves_failed_row(){
        let mut e=engine("scale");let mut mock=Mock::new(e.state.config.clone(),e.stop.clone());mock.reference=true;mock.fail_scale=true;e.device=Some(Box::new(mock));
        assert!(e.execute(Action::Calibrate{revolutions:1,repeats:1,prime:0}).is_err());assert!(!e.state.reference_valid);assert_eq!(e.state.records[0].status,"failed");assert!(e.state.records[0].mass_g.is_none());
    }
    #[test]fn jog_targets_forward_only(){
        assert_eq!(jog_target(1000,25600,0,0,0,500.),Ok(1500));assert_eq!(jog_target(1000,25600,0,0,1,0.5),Ok(13800));assert_eq!(jog_target(0,25600,0,0,2,90.),Ok(6400));
        assert_eq!(jog_target(1000,25600,0,1,0,30000.),Ok(30000));assert!(jog_target(1000,25600,0,1,0,1000.).is_err());assert!(jog_target(1000,25600,0,0,0,0.).is_err());
        // Phase from curve origin 51200: at phase 0, 180° is half a revolution ahead; past 180° it wraps to next revolution.
        assert_eq!(jog_target(51200,25600,51200,2,0,180.),Ok(64000));assert_eq!(jog_target(51200+20000,25600,51200,2,0,180.),Ok(51200+25600+12800));
        assert!(jog_target(64000,25600,51200,2,0,180.).is_err());assert!(jog_target(0,25600,0,0,1,101.).is_err());assert!(jog_target(MAX_POSITION-10,25600,0,0,0,100.).is_err());
    }
    #[test]fn live_bench_weight_polls_only_when_on(){
        let mut e=engine("live");e.bench_poll();assert!(e.state.live_g.is_none());
        e.execute(Action::LiveWeight(true)).unwrap();e.bench_poll();assert!(e.state.live_g.is_some() && e.state.live_stable);
        e.execute(Action::LiveWeight(false)).unwrap();assert!(e.state.live_g.is_none());
    }
    #[test]fn test_dose_writes_no_result_files(){
        let mut e=engine("testdose");e.execute(Action::Characterize{interval:Interval::Degrees(90.),repeats:1}).unwrap();
        let files=|e:&Engine|std::fs::read_dir(&e.output).map(|r|r.flatten().filter(|f|f.file_name().to_string_lossy().starts_with("dosering_")).count()).unwrap_or(0);
        e.execute(Action::TestDose{grams:1.,correction:-1.}).unwrap();
        assert_eq!(files(&e),0,"geen dosering_*.json/.csv");assert!(e.state.path.is_empty());
        let r=e.state.records.last().unwrap();assert_eq!((r.requested_g,r.curve_target_g),(Some(1.),Some(0.99)));assert!((r.mass_g.unwrap()-0.99).abs()<0.001);
        e.execute(Action::Dose{grams:vec![1.],correction:0.}).unwrap();assert_eq!(files(&e),2,"handmatige Dose schrijft nog wel");
    }
    #[test]fn cancelled_run_never_moves(){let mut e=engine("cancel");e.stop.stop();assert!(e.execute(Action::Calibrate{revolutions:1,repeats:1,prime:0}).is_err());assert_eq!(e.state.position,0);assert!(!e.state.reference_valid);}
    #[test]fn can_failure_invalidates_reference(){let mut e=engine("can");let mut mock=Mock::new(e.state.config.clone(),e.stop.clone());mock.reference=true;mock.fail_move=true;e.device=Some(Box::new(mock));assert!(e.execute(Action::Calibrate{revolutions:1,repeats:1,prime:0}).is_err());assert!(!e.state.reference_valid);assert_eq!(e.state.position,0);}
    #[test]fn reference_change_does_not_rewrite_completed_measurement(){
        let mut e=engine("history");e.execute(Action::Calibrate{revolutions:1,repeats:1,prime:0}).unwrap();
        let path=e.state.path.clone();let before=std::fs::read(&path).unwrap();
        e.execute(Action::Zero).unwrap();assert_eq!(std::fs::read(&path).unwrap(),before);
        e.execute(Action::Disconnect).unwrap();assert_eq!(std::fs::read(&path).unwrap(),before);
    }
}
